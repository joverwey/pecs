//! Core [`Promise`] functionality.
use bevy::{
    ecs::system::{Command, SystemParam, SystemParamItem},
    prelude::*,
    utils::HashMap,
};
use pecs_macro::{asyn, impl_all_promises, impl_any_promises};
use std::{
    any::type_name,
    cell::RefCell,
    marker::PhantomData,
    mem,
    sync::{Arc, RwLock},
    thread::{self, ThreadId},
};
pub mod timer;
pub mod app;

pub struct AsyncOps<T>(pub T);

pub fn promise_resolve<S: 'static, R: 'static>(world: &mut World, id: PromiseId, state: S, result: R) {
    // info!(
    //     "resolving {id}<{}, {}, {}>",
    //     type_name::<R>(),
    //     type_name::<E>(),
    //     type_name::<S>(),
    // );
    let registry = world
        .get_resource_or_insert_with(PromiseRegistry::<S, R>::default)
        .clone();
    if let Some(resolve) = {
        let mut write = registry.0.write().unwrap();
        let prom = write.get_mut(&id).unwrap();
        mem::take(&mut prom.resolve)
    } {
        resolve(world, state, result)
    }
    registry.0.write().unwrap().remove(&id);
    // info!(
    //     "resolved {id}<{}, {}, {}> ({} left)",
    //     type_name::<R>(),
    //     type_name::<E>(),
    //     type_name::<S>(),
    //     registry.0.read().unwrap().len()
    // );
}


pub fn promise_register<S: 'static, R: 'static>(world: &mut World, mut promise: Promise<S, R>) {
    let id = promise.id;
    // info!("registering {id}");
    let register = promise.register;
    promise.register = None;
    let registry = world
        .get_resource_or_insert_with(PromiseRegistry::<S, R>::default)
        .clone();
    registry.0.write().unwrap().insert(id, promise);
    if let Some(register) = register {
        register(world, id)
    }
    // info!(
    //     "registered {id}<{}, {}, {}> ({} left)",
    //     type_name::<R>(),
    //     type_name::<E>(),
    //     type_name::<S>(),
    //     registry.0.read().unwrap().len()
    // );
}

pub fn promise_discard<S: 'static, R: 'static>(world: &mut World, id: PromiseId) {
    // info!("discarding {id}");
    let registry = world
        .get_resource_or_insert_with(PromiseRegistry::<S, R>::default)
        .clone();
    if let Some(discard) = {
        let mut write = registry.0.write().unwrap();
        if let Some(prom) = write.get_mut(&id) {
            mem::take(&mut prom.discard)
        } else {
            error!(
                "Internal promise error: trying to discard complete {id}<{}, {}>",
                type_name::<S>(),
                type_name::<R>(),
            );
            None
        }
    } {
        discard(world, id);
    }
    registry.0.write().unwrap().remove(&id);
    // info!(
    //     "discarded {id}<{}, {}, {}> ({} left)",
    //     type_name::<R>(),
    //     type_name::<E>(),
    //     type_name::<S>(),
    //     registry.0.read().unwrap().len()
    // );
}


pub trait PromiseParams: 'static + SystemParam + Send + Sync {}
impl<T: 'static + SystemParam + Send + Sync> PromiseParams for T {}

pub struct AsynFunction<Input, Output, Params: PromiseParams> {
    pub marker: PhantomData<Params>,
    pub body: fn(In<Input>, SystemParamItem<Params>) -> Output,
}
impl<Input, Output, Params: PromiseParams> AsynFunction<Input, Output, Params> {
    fn new(body: fn(In<Input>, SystemParamItem<Params>) -> Output) -> Self {
        AsynFunction {
            body,
            marker: PhantomData,
        }
    }
}

thread_local!(static PROMISE_LOCAL_ID: std::cell::RefCell<usize>  = RefCell::new(0));
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PromiseId {
    thread: ThreadId,
    local: usize,
}
impl PromiseId {
    pub fn new() -> PromiseId {
        PROMISE_LOCAL_ID.with(|id| {
            let mut new_id = id.borrow_mut();
            *new_id += 1;
            PromiseId {
                thread: thread::current().id(),
                local: *new_id,
            }
        })
    }
}

impl std::fmt::Display for PromiseId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let t = format!("{:?}", self.thread);
        write!(
            f,
            "Promise({}:{})",
            t.strip_prefix("ThreadId(").unwrap().strip_suffix(")").unwrap(),
            self.local
        )
    }
}

impl std::fmt::Debug for PromiseId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

pub enum PromiseResult<S, R> {
    Resolve(S, R),
    Await(Promise<S, R>),
}

impl<S, R> From<Promise<S, R>> for PromiseResult<S, R> {
    fn from(value: Promise<S, R>) -> Self {
        PromiseResult::Await(value)
    }
}

#[derive(Resource)]
struct PromiseRegistry<S, R>(Arc<RwLock<HashMap<PromiseId, Promise<S, R>>>>);
impl<S, R> Default for PromiseRegistry<S, R> {
    fn default() -> Self {
        PromiseRegistry(Arc::new(RwLock::new(HashMap::new())))
    }
}
impl<S, R> Clone for PromiseRegistry<S, R> {
    fn clone(&self) -> Self {
        PromiseRegistry(self.0.clone())
    }
}

pub struct Promise<S, R> {
    id: PromiseId,
    register: Option<Box<dyn FnOnce(&mut World, PromiseId)>>,
    discard: Option<Box<dyn FnOnce(&mut World, PromiseId)>>,
    resolve: Option<Box<dyn FnOnce(&mut World, S, R)>>,
}
unsafe impl<S, R> Send for Promise<S, R> {}
unsafe impl<S, R> Sync for Promise<S, R> {}

impl<S: 'static, R: 'static> Promise<S, R> {
    /// Create new [`Promise`] with empty [state][PromiseState]
    /// ```
    /// # use bevy::prelude::*
    /// fn setup(mut commands: Commands) {
    ///     commands.add(
    ///         // type of the `state` is infered as `PromiseState<()>`
    ///         Promise::start(asyn!(state => {
    ///             info!("I'm new Promise with empty state");
    ///             state.pass()
    ///         }))
    ///     );
    /// }
    /// ```
    pub fn start<Params: PromiseParams, P: 'static + Into<PromiseResult<S, R>>>(
        func: AsynFunction<PromiseState<()>, P, Params>,
    ) -> Promise<S, R> {
        Promise::new((), func)
    }
    /// Create new [`Promise`] with [`PromiseState<D>`] state.
    /// ```
    /// # use bevy::prelude::*
    /// fn setup(mut commands: Commands) {
    ///     let entity = commands.spawn_empty().id();
    ///     commands.add(
    ///         // type of the `state` is infered as `PromiseState<Entity>`
    ///         Promise::new(entity, asyn!(state => {
    ///             info!("I'm started with some entity {:?}", state.value);
    ///             state.pass()
    ///         }))
    ///     );
    /// }
    /// ```
    pub fn new<D: 'static, Params: PromiseParams, P: 'static + Into<PromiseResult<S, R>>>(
        default_state: D,
        func: AsynFunction<PromiseState<D>, P, Params>,
    ) -> Promise<S, R> {
        let id = PromiseId::new();
        // let default = OnceValue::new(default_state);
        Promise {
            id,
            resolve: None,
            discard: None,
            register: Some(Box::new(move |world, id| {
                let mut system = IntoSystem::into_system(func.body);
                system.initialize(world);
                let pr = system.run(PromiseState::new(default_state), world).into();
                system.apply_buffers(world);
                match pr {
                    PromiseResult::Resolve(s, r) => promise_resolve::<S, R>(world, id, s, r),
                    PromiseResult::Await(mut p) => {
                        if p.resolve.is_some() {
                            error!(
                                "Misconfigured {}<{}, {}>, resolve already defined",
                                p.id,
                                type_name::<S>(),
                                type_name::<R>(),
                            );
                            return;
                        }
                        p.resolve = Some(Box::new(move |world, s, r| promise_resolve::<S, R>(world, id, s, r)));
                        promise_register::<S, R>(world, p);
                    }
                }
            })),
        }
    }

    /// Create new [`Promise`] with resolve/reject behaviour controlled by user.
    /// It takes two closures as arguments: `on_invoke` and `on_discard`. The
    /// `invoke` will be executed when the promise's turn comes. The discard
    /// will be called in the case of promise cancellation. Both closures takes
    /// [`&mut World`][bevy::prelude::World] and [`PromiseId`] as argiments.
    /// ```rust
    /// #[derive(Component)]
    /// /// Holds PromiseId and the time when the timer should time out.
    /// pub struct MyTimer(PromiseId, f32);
    /// 
    /// /// creates promise that will resolve after [`duration`] seconds
    /// pub fn delay(duration: f32) -> Promise<(), ()> {
    ///     Promise::register(
    ///         // this will be invoked when promise's turn comes
    ///         move |world, id| {
    ///             let now = world.resource::<Time>().elapsed_seconds();
    ///             // store timer
    ///             world.spawn(MyTimer(id, now + duration));
    ///         },
    ///         // this will be invoked when promise got discarded
    ///         move |world, id| {
    ///             let entity = {
    ///                 let mut timers = world.query::<(Entity, &MyTimer)>();
    ///                 timers
    ///                     .iter(world)
    ///                     .filter(|(_entity, timer)| timer.0 == id)
    ///                     .map(|(entity, _timer)| entity)
    ///                     .next()
    ///             };
    ///             if let Some(entity) = entity {
    ///                 world.despawn(entity);
    ///             }
    ///         },
    ///     )
    /// }
    /// 
    /// /// iterate ofver all timers and resolves completed
    /// pub fn process_timers_system(timers: Query<(Entity, &MyTimer)>, mut commands: Commands, time: Res<Time>) {
    ///     let now = time.elapsed_seconds();
    ///     for (entity, timer) in timers.iter().filter(|(_, t)| t.1 < now) {
    ///         let promise_id = timer.0;
    ///         commands.promise(promise_id).resolve(());
    ///         commands.entity(entity).despawn();
    ///     }
    /// }
    /// 
    /// fn setup(mut commands: Commands) {
    ///     // `delay()` can be called from inside promise
    ///     commands.add(
    ///         Promise::start(asyn!(_state => {
    ///             info!("Starting");
    ///             delay(1.)
    ///         }))
    ///         .then(asyn!(s, _ => {
    ///             info!("Completing");
    ///             s.pass()
    ///         })),
    ///     );
    /// 
    ///     // or queued directly to Commands
    ///     commands.add(delay(2.).then(asyn!(s, _ => {
    ///         info!("I'm another timer");
    ///         s.pass()
    ///     })));
    /// }
    /// ```
    pub fn register<F: 'static + FnOnce(&mut World, PromiseId), D: 'static + FnOnce(&mut World, PromiseId)>(
        on_invoke: F,
        on_discard: D,
    ) -> Promise<S, R> {
        Promise {
            id: PromiseId::new(),
            resolve: None,
            register: Some(Box::new(on_invoke)),
            discard: Some(Box::new(on_discard)),
        }
    }

    pub fn then<
        S2: 'static,
        R2: 'static,
        Params: PromiseParams,
        P: 'static + Into<PromiseResult<S2, R2>>,
    >(
        mut self,
        func: AsynFunction<(PromiseState<S>, R), P, Params>,
    ) -> Promise<S2, R2> {
        let id = PromiseId::new();
        let discard = mem::take(&mut self.discard);
        let self_id = self.id;
        self.discard = Some(Box::new(move |world, _id| {
            promise_discard::<S2, R2>(world, id);
        }));
        self.resolve = Some(Box::new(move |world, state, result| {
            let mut system = IntoSystem::into_system(func.body);
            system.initialize(world);
            let pr = system.run((PromiseState::new(state), result), world).into();
            system.apply_buffers(world);
            match pr {
                PromiseResult::Resolve(s, r) => promise_resolve::<S2, R2>(world, id, s, r),
                PromiseResult::Await(mut p) => {
                    if p.resolve.is_some() {
                        error!(
                            "Misconfigured {}<{}, {}>, resolve already defined",
                            p.id,
                            type_name::<S2>(),
                            type_name::<R2>(),
                        );
                        return;
                    }
                    p.resolve = Some(Box::new(move |world, s, r| {
                        promise_resolve::<S2, R2>(world, id, s, r);
                    }));
                    promise_register::<S2, R2>(world, p);
                }
            }
        }));
        Promise {
            id,
            register: Some(Box::new(move |world, _id| {
                promise_register::<S, R>(world, self);
            })),
            discard: Some(Box::new(move |world, _id| {
                if let Some(discard) = discard {
                    discard(world, self_id);
                }
            })),
            resolve: None,
        }
    }

    pub fn with_result<R2: 'static>(self, value: R2) -> Promise<S, R2> {
        self.map_result(|_| value)
    }

    pub fn map_result<R2: 'static, F: 'static + FnOnce(R) -> R2>(mut self, map: F) -> Promise<S, R2> {
        let id = PromiseId::new();
        let discard = mem::take(&mut self.discard);
        let self_id = self.id;
        self.discard = Some(Box::new(move |world, _id| {
            promise_discard::<S, R2>(world, id);
        }));
        self.resolve = Some(Box::new(move |world, state, result| {
            let result = map(result);
            promise_resolve::<S, R2>(world, id, state, result);
        }));
        Promise {
            id,
            register: Some(Box::new(move |world, _id| {
                promise_register::<S, R>(world, self);
            })),
            discard: Some(Box::new(move |world, _id| {
                if let Some(discard) = discard {
                    discard(world, self_id);
                }
            })),
            resolve: None,
        }
    }

    pub fn with<S2: 'static>(self, state: S2) -> Promise<S2, R> {
        self.map(|_| state)
    }

    pub fn map<S2: 'static, F: 'static + FnOnce(S) -> S2>(mut self, map: F) -> Promise<S2, R> {
        let id = PromiseId::new();
        let discard = mem::take(&mut self.discard);
        let self_id = self.id;
        self.discard = Some(Box::new(move |world, _id| {
            promise_discard::<S2, R>(world, id);
        }));
        self.resolve = Some(Box::new(move |world, state, result| {
            let state = map(state);
            promise_resolve::<S2, R>(world, id, state, result);
        }));
        Promise {
            id,
            register: Some(Box::new(move |world, _id| {
                promise_register::<S, R>(world, self);
            })),
            discard: Some(Box::new(move |world, _id| {
                if let Some(discard) = discard {
                    discard(world, self_id);
                }
            })),
            resolve: None,
        }
    }
}

impl<R: 'static> Promise<(), R> {
    /// Create stateless [resolve][PromiseResult::Resolve] with `R` result.
    pub fn resolve(result: R) -> PromiseResult<(), R> {
        PromiseResult::Resolve((), result)
    }
}

impl Promise<(), ()> {
    pub fn pass() -> PromiseResult<(), ()> {
        PromiseResult::Resolve((), ())
    }
    pub fn any<T: AnyPromises>(any: T) -> Promise<(), T::Result> {
        any.register()
    }
    pub fn all<T: AllPromises>(any: T) -> Promise<(), T::Result> {
        any.register()
    }
}

pub struct PromiseCommand<R> {
    id: PromiseId,
    result: R
}

impl<R> PromiseCommand<R> {
    pub fn resolve(id: PromiseId, result: R) -> Self {
        PromiseCommand { id, result }
    }
}

impl<R: 'static + Send + Sync> Command for PromiseCommand<R> {
    fn write(self, world: &mut World) {
        promise_resolve::<(), R>(world, self.id, (), self.result);
    }
}

impl<R: 'static, S: 'static> Command for Promise<S, R> {
    fn write(self, world: &mut World) {
        promise_register::<S, R>(world, self)
    }
}

pub struct PromiseCommands<'w, 's, 'a> {
    id: PromiseId,
    commands: &'a mut Commands<'w, 's>,
}
impl<'w, 's, 'a> PromiseCommands<'w, 's, 'a> {
    pub fn resolve<R: 'static + Send + Sync>(&mut self, value: R) -> &mut Self {
        self.commands.add(PromiseCommand::<R>::resolve(self.id, value));
        self
    }
}

pub trait PromiseCommandsExtension<'w, 's> {
    fn promise<'a>(&'a mut self, id: PromiseId) -> PromiseCommands<'w, 's, 'a>;
}

impl<'w, 's> PromiseCommandsExtension<'w, 's> for Commands<'w, 's> {
    fn promise<'a>(&'a mut self, id: PromiseId) -> PromiseCommands<'w, 's, 'a> {
        PromiseCommands { id, commands: self }
    }
}

impl<T: Clone> Clone for AsyncOps<T> {
    fn clone(&self) -> Self {
        AsyncOps(self.0.clone())
    }
}
impl<T: Copy> Copy for AsyncOps<T> {}
pub struct PromiseState<S> {
    pub value: S,
}
impl<S: 'static> PromiseState<S> {
    pub fn new(value: S) -> PromiseState<S> {
        PromiseState { value }
    }
    pub fn asyn(self) -> AsyncOps<S> {
        AsyncOps(self.value)
    }
    pub fn resolve<R>(self, result: R) -> PromiseResult<S, R> {
        PromiseResult::Resolve(self.value, result)
    }
    pub fn pass(self) -> PromiseResult<S, ()> {
        PromiseResult::Resolve(self.value, ())
    }
    pub fn map<T, F: FnOnce(S) -> T>(self, map: F) -> PromiseState<T> {
        PromiseState { value: map(self.value) }
    }

    pub fn with<T: 'static>(self, value: T) -> PromiseState<T> {
        PromiseState { value }
    }

    pub fn then<R: 'static, S2: 'static>(self, promise: Promise<S2, R>) -> Promise<S, R> {
        promise.with(self.value)
    }

    pub fn any<A: AnyPromises>(self, any: A) -> Promise<S, A::Result> {
        any.register().with(self.value)
    }

    pub fn all<A: AllPromises>(self, all: A) -> Promise<S, A::Result> {
        all.register().with(self.value)
    }
}

impl<T: std::fmt::Display> std::fmt::Display for PromiseState<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PromiseState({})", self.value)
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for PromiseState<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PromiseState({:?})", self.value)
    }
}

pub struct MutPtr<T>(*mut T);
unsafe impl<T> Send for MutPtr<T> {}
unsafe impl<T> Sync for MutPtr<T> {}
impl<T> Clone for MutPtr<T> {
    fn clone(&self) -> Self {
        MutPtr(self.0)
    }
}

impl<T> MutPtr<T> {
    pub fn new(value: T) -> MutPtr<T> {
        let b = Box::new(value);
        MutPtr(Box::leak(b) as *mut T)
    }
    pub fn get(&mut self) -> T {
        if self.0.is_null() {
            panic!("Ups.")
        }
        let b = unsafe { Box::from_raw(self.0) };
        self.0 = std::ptr::null_mut();
        *b
    }
    pub fn get_ref(&self) -> &T {
        if self.0.is_null() {
            panic!("Ups.");
        }
        unsafe { self.0.as_ref().unwrap() }
    }
    pub fn get_mut(&mut self) -> &mut T {
        if self.0.is_null() {
            panic!("Ups.");
        }
        unsafe { self.0.as_mut().unwrap() }
    }
    pub fn is_valid(&self) -> bool {
        !self.0.is_null()
    }
}

pub trait AnyPromises {
    type Result: 'static;
    fn register(self) -> Promise<(), Self::Result>;
}
pub trait AllPromises {
    type Result: 'static;
    fn register(self) -> Promise<(), Self::Result>;
}

impl<S: 'static, R: 'static> AnyPromises for Vec<Promise<S, R>> {
    type Result = (S, R);
    fn register(self) -> Promise<(), Self::Result> {
        let ids: Vec<PromiseId> = self.iter().map(|p| p.id).collect();
        let discard_ids = ids.clone();
        Promise::register(
            move |world, any_id| {
                let mut idx = 0usize;
                for promise in self {
                    let ids = ids.clone();
                    promise_register(
                        world,
                        promise.map(move |s| (s, any_id, idx, ids)).then(asyn!(|s, r| {
                            let (state, any_id, idx, ids) = s.value;
                            Promise::<(), ()>::register(
                                move |world, _id| {
                                    for (i, id) in ids.iter().enumerate() {
                                        if i != idx {
                                            promise_discard::<S, R>(world, *id);
                                        }
                                    }
                                    promise_resolve::<(), (S, R)>(world, any_id, (), (state, r))
                                },
                                |_, _| {},
                            )
                        })),
                    );
                    idx += 1;
                }
            },
            move |world, _| {
                for id in discard_ids {
                    promise_discard::<S, R>(world, id);
                }
            },
        )
    }
}

impl<S: 'static, R: 'static> AllPromises for Vec<Promise<S, R>> {
    type Result = Vec<(S, R)>;
    fn register(self) -> Promise<(), Self::Result> {
        let ids: Vec<PromiseId> = self.iter().map(|p| p.id).collect();
        let size = ids.len();
        Promise::register(
            move |world, any_id| {
                let value: Vec<Option<(S, R)>> = (0..size).map(|_| None).collect();
                let value = MutPtr::new(value);
                let mut idx = 0usize;
                for promise in self {
                    let value = value.clone();
                    promise_register(
                        world,
                        promise.map(move |s| (s, any_id, idx, value)).then(asyn!(|s, r| {
                            let (s, any_id, idx, mut value) = s.value;
                            Promise::<(), ()>::register(
                                move |world, _id| {
                                    value.get_mut()[idx] = Some((s, r));
                                    if value.get_ref().iter().all(|v| v.is_some()) {
                                        let value = value.get().into_iter().map(|v| v.unwrap()).collect();
                                        promise_resolve::<(), Vec<(S, R)>>(world, any_id, (), value)
                                    }
                                },
                                |_, _| {},
                            )
                        })),
                    );
                    idx += 1;
                }
            },
            move |world, _| {
                for id in ids {
                    promise_discard::<S, R>(world, id);
                }
            },
        )
    }
}

impl_any_promises! { 8 }
impl_all_promises! { 8 }

pub struct Promises<S: 'static, R: 'static>(Vec<Promise<S, R>>);
impl<S: 'static, R: 'static> Promises<S, R> {
    pub fn any(self) -> Promise<(), (S, R)> {
        PromiseState::new(()).any(self.0)
    }
    pub fn all(self) -> Promise<(), Vec<(S, R)>> {
        PromiseState::new(()).all(self.0)
    }
}

pub trait PromisesExtension<S: 'static, R: 'static> {
    fn promise(self) -> Promises<S, R>;
}

impl<S: 'static, R: 'static, I: Iterator<Item = Promise<S, R>>> PromisesExtension<S, R> for I {
    fn promise(self) -> Promises<S, R> {
        Promises(self.collect())
    }
}
