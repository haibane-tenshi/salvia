//! Runtime and contexts.
//!
//! # Storage
//!
//! `salvia` delegates cache storage to special infinitely looped tasks (*nodes*)
//! which hang inside executor.
//! One such node is spawned per *bound query* or *bound input*.
//! The storage is lazy: it is created when query/input is used for the first time
//! and task wakes only when it is (potentially transitively) polled by the user.
//!
//! Runtime stores handles to all nodes it spawned and communicates that through contexts
//! to queries and inputs.
//!
//! The structure hinges on being accessible through the runtime instance so dropping it
//! will cause all associated *nodes* to (eventually) terminate.
//!
//! # Synchronization
//!
//! ## The need for synchronization
//!
//! The following snippet illustrates the issue:
//!
//! ```
//! # use salvia::{InputAccess, Input, QueryContext, query};
//! # use futures::join;
//! #[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, InputAccess)]
//! struct A;
//!
//! impl Input<usize> for A {
//!     fn initial(&self) -> usize { 0 }
//! }
//!
//! #[query]
//! async fn left(cx: &QueryContext) -> usize {
//!     A.get(cx).await
//! }
//!
//! #[query]
//! async fn right(cx: &QueryContext) -> usize {
//!     A.get(cx).await
//! }
//!
//! #[query]
//! async fn bad(cx: &QueryContext) {
//!     let (left, right) = join!(left(cx), right(cx));
//!
//!     assert_eq!(left, right);
//! }
//! ```
//!
//! Without synchronization this is basically identical to how normal async code works,
//! so call to `bad` may panic, for example:
//! *  `left` is polled first, asks a value from `A`, `bad` suspends on `right`
//! *  some other task sets a different `usize` in `A`
//! *  `right` is awaken, takes a new value from `A`
//! *  `bad` panics as `left` and `right` disagreed on which `usize` value in `A` they have seen
//!
//! Calling the same source twice is not guaranteed to return the same value.
//! But in our case it becomes a major issue: query which can observe inconsistent input values
//! is no longer pure, it may yield different results when executed at different times!
//! This is a nasty side-effect baked into the system itself.
//!
//! This problem is unique to asynchronous frameworks,
//! sync variants can rely on aliasing rules and normal Rust's synchronization mechanisms to prevent
//! situations like this.
//!
//! ## Runtime phases
//!
//! `salvia` solves the problem by introducing two distinct runtime phases:
//!
//! *   During *query* phase input values are frozen and queries are allowed to be executed.
//!     All queries are required to finalize before rolling into *mutation*.
//!
//!     This is indicated to the user by access to `QueryContext`.
//!
//! *   During *mutation* phase input values can be mutated, however no queries can be executed.
//!
//!     This is indicated to the user by access to `InputContext`.
//!
//! Change between phases can happen when user calls [`Runtime::mutate`] or [`Runtime::query`].
//! It will request phase transition from internal clock task and suspend until
//! the phase is entered.
//! There is no need to transition if runtime is already in required phase, so multiple calls
//! to `mutate` or `query` can be executed concurrently within their respective phase.
//!
//! `QueryContext` and `InputContext` in fact are RAII guards which keep runtime from rolling
//! into next phase.
//! This guarantees that as long as you keep hold of one interacting with queries/inputs is always
//! sound.
//!
//! ## Scheduling
//!
//! However holding onto `QueryContext`/`InputContext` does not *necessarily* guarantee
//! that other `query`s/`mutate`s can execute.
//! This happens because phase changes are queued internally and performed in request order,
//! in other words transitions are *fair*.
//!
//! It means that for example a call to `query` can be awaiting on a `mutate`
//! which in turn is awaiting on current *query* phase to end.
//!
//! This in particular implies that requesting a new context without releasing old one on the same
//! task may deadlock.
//! Normally this shouldn't be an issue as contexts are scoped to closures you pass into
//! `query`/`mutate`, however attempting to call these functions recursively will likely
//! cause issues.
//!
//! ## Mutation sequencing
//!
//! Calls to [`Runtime::mutate`] are atomic: any changes performed within a single call
//! are guaranteed to happen uninterrupted.
//! For this reason calls to `mutation` are executed sequentially.
//!
//! # Update routine
//!
//! When a query *node* is requested to update its cache (and it is not up-to-date already)
//! it does so in three steps:
//!
//! 1.  Check if any (transitive) input dependencies has been modified.
//!
//!     This step is quite efficient: all necessary information is embedded into context,
//!     so no communication with other nodes is necessary.
//!
//!     The goal here is to avoid "update cascade" (next step) in following cases:
//!     *   Query is constant, i. e. doesn't depend on any input.
//!         There is no point to ever recalculate this one.
//!     *   Query is frequently called, but its inputs doesn't necessarily change even if other
//!         inputs do.
//!     *   During second step prevent touching branches of call tree unaffected
//!         by changes in inputs.
//!
//! 2.  Check if any (direct) query/input dependencies has been modified.
//!
//!     This recursively asks upstream nodes to update and awaits until they finish.
//!
//! 3.  Recalculate the query and compare it to cached value.
//!
//! If any step yields positive result, update routine is cut short and signals its requester that
//! cached value is still valid.
//! Otherwise the new value is cached and recalculation is propagated downstream.

mod clock;

use std::fmt::Debug;
use std::future::Future;
use std::sync::Arc;

use anymap2::SendSyncAnyMap;
use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot, RwLock, RwLockReadGuard};

use crate::node::{LocalInstantRequest, QueryValidationHandle, UpstreamEdges, ValueDepInfo};
use clock::ClockHandle;
pub(crate) use clock::{InputToken, QueryToken};

/// Map type used internally by [`Runtime`] to store node managers.
type AnyMap = SendSyncAnyMap;

/// Context indicating *mutation* phase.
///
/// This context contains all information necessary for input setters to be executed.
///
/// See [module-level](crate::runtime#runtime-phases) documentation for more details.
#[derive(Debug)]
pub struct InputContext {
    rt: Arc<Runtime>,
    token: InputToken,
}

impl InputContext {
    /// Create new context.
    pub(crate) fn new(rt: Arc<Runtime>, token: InputToken) -> Self {
        Self { rt, token }
    }

    /// Acquire reference to associated runtime.
    pub(crate) fn runtime(&self) -> &Arc<Runtime> {
        &self.rt
    }

    /// Acquire reference to associated token.
    pub(crate) fn input_token(&self) -> &InputToken {
        &self.token
    }
}

/// Context indicating *query* phase.
///
/// This context contains all information necessary for queries and input getters to be executed.
///
/// See [module-level](crate::runtime#runtime-phases) documentation for more details.
#[derive(Debug)]
pub struct QueryContext {
    rt: Arc<Runtime>,
    token: QueryToken,
    upstream: Mutex<UpstreamEdges>,
}

impl QueryContext {
    /// Create new context.
    pub(crate) fn new(rt: Arc<Runtime>, token: QueryToken) -> Self {
        Self {
            rt,
            token,
            upstream: Default::default(),
        }
    }

    /// Acquire reference to associated runtime.
    pub(crate) fn runtime(&self) -> &Arc<Runtime> {
        &self.rt
    }

    /// Acquire reference to associated token.
    pub(crate) fn query_token(&self) -> &QueryToken {
        &self.token
    }

    /// Record new upstream edge dependency information.
    pub(crate) fn add_edge(&self, validation: QueryValidationHandle, dep_info: ValueDepInfo) {
        let mut guard = self.upstream.lock();
        guard.add_edge(validation, dep_info);
    }

    /// Consume context and produce recorded upstream dependencies.
    pub(crate) fn into_edges(self) -> UpstreamEdges {
        self.upstream.into_inner()
    }
}

/// `salvia` runtime.
///
/// Runtime serves as central point of access to queries and inputs.
/// It also oversees internal synchronization task and issues execution contexts.
///
/// When runtime is dropped it will cause all associated tasks to terminate.
pub struct Runtime {
    managers: RwLock<AnyMap>,
    clock_handle: ClockHandle,
}

impl Runtime {
    /// Create a new runtime.
    ///
    /// This function is guaranteed to return `Poll::Ready`, `async` is just a reminder
    /// that you need to call it from within `tokio` context.
    ///
    /// See [`Runtime::with_tokio`] if you need to create a runtime from within synchronous context.
    pub async fn new() -> Arc<Runtime> {
        Arc::new(Runtime::create())
    }

    /// Synchronously create a new runtime.
    ///
    /// See [`Runtime::new`] if you need to create a runtime from within asynchronous context.
    pub fn with_tokio(tokio: &tokio::runtime::Runtime) -> Arc<Runtime> {
        let _guard = tokio.enter();
        Arc::new(Runtime::create())
    }

    /// Create a new unwrapped runtime.
    ///
    /// # Panics
    ///
    /// This function will panic if there is no `tokio` runtime attached to the current thread.
    fn create() -> Runtime {
        let clock_handle = clock::spawn();

        Runtime {
            managers: RwLock::new(AnyMap::new()),
            clock_handle,
        }
    }

    /// Get existing node manager or insert with specified function.
    pub(crate) async fn manager_or_insert_with<Manager, F>(
        &self,
        f: F,
    ) -> RwLockReadGuard<'_, Manager>
    where
        F: FnOnce() -> Manager,
        Manager: Send + Sync + 'static,
    {
        let guard = self.managers.read().await;
        if let Ok(guard) = RwLockReadGuard::try_map(guard, |t| t.get()) {
            return guard;
        }

        let mut guard = self.managers.write().await;
        guard.entry().or_insert_with(f);
        RwLockReadGuard::map(guard.downgrade(), |t| t.get().unwrap())
    }

    /// Register an input node within clock task.
    ///
    /// This function simply forwards registration request to clock task.
    ///
    /// See [`RegisterInputRequest`](clock::RegisterInputRequest) for details.
    pub(crate) async fn register_input(&self, handle: mpsc::Sender<LocalInstantRequest>) -> usize {
        use clock::RegisterInputRequest;

        let (callback, index_out) = oneshot::channel();
        let payload = RegisterInputRequest { handle, callback };

        self.clock_handle
            .register_input
            .send(payload)
            .expect("clock should be alive as long as runtime");

        index_out
            .await
            .expect("clock should properly respond to input registration requests")
    }

    /// Request a new `QueryToken`.
    pub(crate) async fn query_token(self: &Arc<Self>) -> QueryToken {
        use clock::ClockInstr;

        let (data_in, data_out) = oneshot::channel();
        self.clock_handle
            .instructions
            .send(ClockInstr::StartQuery(data_in))
            .expect("clock should be alive as long as runtime");

        data_out
            .await
            .expect("clock should properly respond to instructions")
    }

    /// Request a new `InputToken`.
    pub(crate) async fn input_token(self: &Arc<Self>) -> InputToken {
        use clock::ClockInstr;

        let (data_in, data_out) = oneshot::channel();
        self.clock_handle
            .instructions
            .send(ClockInstr::StartMutation(data_in))
            .expect("clock should be alive as long as runtime");

        data_out
            .await
            .expect("clock should properly respond to instructions")
    }

    /// Initiate *query* phase.
    ///
    /// `query` serves as initial point for any interaction with queries.
    ///
    /// This function does two jobs:
    /// *   Transition runtime to *query* phase.
    ///
    ///     This plays an important role in internal synchronization.
    ///     Any queries executed with the same `QueryContext` are guaranteed to agree on which
    ///     input values they have seen.
    ///
    ///     Refer to [module-level](crate::runtime#synchronization) documentation for more details.
    ///
    /// *   Generate a `QueryContext`.
    ///
    ///     Context will be made available to passed closure and then can be used to run queries.
    pub async fn query<F, Ret, T>(self: &Arc<Self>, root_query: F) -> T
    where
        F: FnOnce(QueryContext) -> Ret,
        Ret: Future<Output = T>,
    {
        let token = self.query_token().await;

        let cx = QueryContext::new(self.clone(), token);

        root_query(cx).await
    }

    /// Initiate *mutation* phase.
    ///
    /// `mutate` serves as the only point for mutating inputs.
    ///
    /// This function does two jobs:
    /// *   Transition runtime to *mutation* phase.
    ///
    ///     This plays an important role in internal synchronization.
    ///     No queries are allowed to be executed during *mutation* phase which prevents them
    ///     from observing inconsistent input values.
    ///
    ///     Refer to [module-level](crate::runtime#synchronization) documentation for more details.
    ///
    /// *   Generate an `InputContext`.
    ///
    ///     Context will be made available to passed closure and then can be used
    ///     to call input setters.
    pub async fn mutate<F, Ret, T>(self: &Arc<Self>, f: F) -> T
    where
        F: FnOnce(InputContext) -> Ret,
        Ret: Future<Output = T>,
    {
        let token = self.input_token().await;

        let cx = InputContext::new(self.clone(), token);

        f(cx).await
    }
}

impl Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(Runtime)).finish_non_exhaustive()
    }
}
