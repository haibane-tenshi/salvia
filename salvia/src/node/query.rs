//! Query node.
//!
//! Queries are pure functions which can cache their value and recalculate only when other
//! queries or inputs it depends on change.
//! Queries can be created with the help of [`#[query]`](query) attribute.

use std::fmt::Debug;
use std::future::Future;
use std::hash::Hash;
use std::ops::RangeInclusive;
use std::sync::{Arc, Weak};

use crate::node::{
    ExecutorError, InputSelection, QueryHandle, QueryReceiver, QueryValidationHandle,
    UpstreamEdges, ValidationRequest, ValidationStatus, ValueDepInfo, ValueRequest,
};
use crate::runtime::{QueryToken, Runtime};
use crate::time::{GlobalInstant, InputSnapshot, LocalInstant};
use crate::{QueryContext, Stashable};

pub use salvia_macro::query;

/// Extend `base` range into the past if possible.
///
/// Note that even if extension goes past `base.end()` it is ignored.
fn extend_valid_range(
    base: RangeInclusive<GlobalInstant>,
    extension: RangeInclusive<GlobalInstant>,
) -> RangeInclusive<GlobalInstant> {
    use crate::time::Duration;

    let start = if *base.start() <= *extension.end() + Duration::from_frames(1) {
        *base.start().min(extension.start())
    } else {
        *base.start()
    };

    let end = *base.end();

    start..=end
}

/// Error indicating that `Runtime` is no longer accessible.
#[derive(Debug)]
struct DroppedRuntimeError;

/// Aggregate status of input dependencies.
#[derive(Debug, Eq, PartialEq)]
enum InputStatus {
    Unchanged,
    Changed,
}

/// Aggregate status of upstream nodes.
#[derive(Debug, Eq, PartialEq)]
enum UpstreamStatus {
    Valid(ValueDepInfo),
    Invalid,
}

/// Status of node cache with respect to current `FrameToken`.
#[derive(Debug, Eq, PartialEq)]
enum CacheStatus {
    UpToDate,
    RangeChanged {
        new_range: RangeInclusive<GlobalInstant>,
        new_input_selection: Option<InputSelection>,
    },
    NeedRecalculation,
}

/// Result of cache update.
#[derive(Debug, Eq, PartialEq)]
enum CacheUpdate {
    Unchanged,
    Changed,
}

/// Representation of bound query.
#[derive(Clone)]
struct Core<Params, F> {
    /// User function defining how to generate values.
    pub tenant: F,

    /// Set of parameter values `tenant` is to be evaluated on.
    pub params: Params,
}

impl<Params, F> Debug for Core<Params, F>
where
    Params: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(CacheCore))
            .field("fn", &std::any::type_name::<F>())
            .field("params", &self.params)
            .finish()
    }
}

impl<Value, Params, F, Ret> Core<Params, F>
where
    Value: Stashable,
    Params: Stashable,
    F: FnOnce(Params, QueryContext) -> Ret + Clone,
    Ret: Future<Output = (Value, QueryContext)>,
{
    /// Evaluate user function producing new value with respect to `FrameToken`.
    #[cfg_attr(feature = "tracing", tracing::instrument(level = "debug", skip_all, fields(cache = ?self)))]
    async fn evaluate(
        &self,
        rt: Arc<Runtime>,
        token: QueryToken,
    ) -> (Value, Vec<QueryValidationHandle>, ValueDepInfo) {
        let cx = QueryContext::new(rt, token);
        let (value, cx) = (self.tenant.clone())(self.params.clone(), cx).await;
        let UpstreamEdges {
            validation,
            dep_info,
        } = cx.into_edges();

        (value, validation, dep_info)
    }
}

/// Input dependency information.
#[derive(Debug)]
struct InputDeps {
    /// Selection of (transitive) input node dependencies.
    pub selection: InputSelection,

    /// Snapshot of local input times on which cached value was calculated.
    pub snapshot: InputSnapshot,
}

impl InputDeps {
    /// Validate if any (transitive) inputs has changed.
    ///
    /// The function will return `None` in case cache is too much outdated and there is possibility
    /// of input's local time wrapping around, i.e. it's no longer impossible to determine.
    fn status(&self, valid_until: GlobalInstant, token: &QueryToken) -> Option<InputStatus> {
        // If we don't depend on any input we are a constant and can never change.
        if self.selection.is_empty() {
            return Some(InputStatus::Unchanged);
        }

        // Make sure LocalTime didn't wrap since last time we saw it.
        // Otherwise we cannot reason on input dependencies.
        let time_diff = token.current_frame() - valid_until;
        if time_diff >= LocalInstant::MAX_GLOBAL_FRAMES {
            return None;
        }

        if Arc::ptr_eq(&self.snapshot, token.input_snapshot()) {
            return Some(InputStatus::Unchanged);
        }

        // We cannot just zip here: input_selection is always of correct length,
        // however *either* local time vec can be shorter than input_selection.
        // This can happen when input node was spawned in response to evaluation of
        // current node.
        // We have to accommodate for potentially missing LocalTime's by providing initial
        // value; comparison cannot just be omitted - it is possible for input to have
        // changed since then.
        let changed = self.selection.iter().enumerate().any(|(i, is_relevant)| {
            *is_relevant && {
                let node_time = self
                    .snapshot
                    .get(i)
                    .cloned()
                    .unwrap_or_else(LocalInstant::initial);
                let frame_time = token
                    .input_snapshot()
                    .get(i)
                    .cloned()
                    .unwrap_or_else(LocalInstant::initial);

                node_time != frame_time
            }
        });

        let status = if changed {
            InputStatus::Changed
        } else {
            InputStatus::Unchanged
        };

        Some(status)
    }
}

/// Upstream dependencies information.
struct UpstreamDeps {
    /// Validation channels of (direct) upstream node dependencies.
    pub handles: Vec<QueryValidationHandle>,
}

impl UpstreamDeps {
    /// Validate upstream dependencies.
    async fn status(&self, valid_until: GlobalInstant, token: &QueryToken) -> UpstreamStatus {
        use futures::stream::{FuturesUnordered, TryStreamExt};

        // Unfortunately `TryStream` is only implemented for futures returning `Result`,
        // so we use `Result<_, ()>` instead of `Option`.
        // We also need this to be a function as closures + lifetimes + async doesn't (yet) work.
        async fn f(
            handle: &QueryValidationHandle,
            token: QueryToken,
            valid_until: GlobalInstant,
        ) -> Result<ValidationStatus, ()> {
            use tokio::sync::oneshot;

            let (status_in, status_out) = oneshot::channel();
            let request = ValidationRequest {
                token,
                past_frame: valid_until,
                callback: status_in,
            };

            handle.send(request).map_err(|_| ())?;

            status_out.await.map_err(|_| ())
        }

        let futures: FuturesUnordered<_> = self
            .handles
            .iter()
            .map(|handle| f(handle, token.clone(), valid_until))
            .collect();

        let upstream_dep_info = futures
            .try_fold(ValueDepInfo::trivial(), |acc, status| async move {
                let dep_info = match status {
                    ValidationStatus::Valid(t) => t,
                    ValidationStatus::Invalid => return Err(()),
                };

                Ok(acc | dep_info)
            })
            .await;

        match upstream_dep_info {
            Ok(t) => UpstreamStatus::Valid(t),
            Err(()) => UpstreamStatus::Invalid,
        }
    }
}

impl Debug for UpstreamDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        struct Wrapper(usize);

        impl Debug for Wrapper {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "[..; {}]", self.0)
            }
        }

        f.debug_struct(stringify!(UpstreamDeps))
            .field("handles", &Wrapper(self.handles.len()))
            .finish()
    }
}

/// Node cache definition.
struct Cache<Value, Params, F> {
    /// Reference to associated runtime.
    ///
    /// It is a weak ref to allow `Runtime` to be dropped.
    pub rt: Weak<Runtime>,

    /// Node identity.
    pub core: Core<Params, F>,

    /// Currently persisting value.
    pub value: Value,

    /// Range of global time during which cached value is actual.
    ///
    /// Because nodes are lazy it is possible for global time to progress without task noticing.
    /// This is why the latest known-to-be-valid moment is explicitly stated as range end.
    pub valid_range: RangeInclusive<GlobalInstant>,

    /// Information about all (direct) upstream node dependencies.
    pub upstream_deps: UpstreamDeps,

    /// Information about all (transitive) input dependencies.
    pub input_deps: InputDeps,
}

impl<Value, Params, F, Ret> Cache<Value, Params, F>
where
    Value: Stashable,
    Params: Stashable,
    F: FnOnce(Params, QueryContext) -> Ret + Clone,
    Ret: Future<Output = (Value, QueryContext)>,
{
    /// Populate a new cache.
    async fn new(rt: Arc<Runtime>, core: Core<Params, F>, token: QueryToken) -> Self {
        let current_frame = token.current_frame();

        let snapshot = token.input_snapshot().clone();
        let weak_rt = Arc::downgrade(&rt);
        let (value, handles, dep_info) = core.evaluate(rt, token).await;

        let ValueDepInfo {
            since,
            input_selection: selection,
        } = dep_info;

        let upstream_deps = UpstreamDeps { handles };

        let input_deps = InputDeps {
            selection,
            snapshot,
        };

        let valid_range = since..=current_frame;

        Self {
            rt: weak_rt,
            core,
            value,
            valid_range,
            upstream_deps,
            input_deps,
        }
    }

    /// Oldest moment in time where cached value is known to be valid.
    fn valid_since(&self) -> GlobalInstant {
        *self.valid_range.start()
    }

    /// Latest moment in time where cached value is known to be valid.
    fn valid_until(&self) -> GlobalInstant {
        *self.valid_range.end()
    }

    /// Calculate status of currently cached value.
    async fn status(&self, token: &QueryToken) -> CacheStatus {
        if self.valid_until() == token.current_frame() {
            return CacheStatus::UpToDate;
        }

        debug_assert!(
            self.valid_until() < token.current_frame(),
            "value cannot be valid for future moments in time"
        );

        let input_status = self.input_deps.status(self.valid_until(), token);

        if let Some(InputStatus::Unchanged) = input_status {
            // When there is no changes to inputs we can simply extend valid range
            // up to current instant.
            let new_range = self.valid_since()..=token.current_frame();

            return CacheStatus::RangeChanged {
                new_range,
                new_input_selection: None,
            };
        }

        let upstream_status = self.upstream_deps.status(self.valid_until(), token).await;

        match upstream_status {
            UpstreamStatus::Valid(dep_info) => {
                let ValueDepInfo {
                    since,
                    input_selection,
                } = dep_info;

                let new_range =
                    extend_valid_range(since..=token.current_frame(), self.valid_range.clone());

                CacheStatus::RangeChanged {
                    new_range,
                    new_input_selection: Some(input_selection),
                }
            }
            UpstreamStatus::Invalid => CacheStatus::NeedRecalculation,
        }
    }

    /// Update cache to current moment in time.
    async fn update(&mut self, token: QueryToken) -> Result<CacheUpdate, DroppedRuntimeError> {
        let status = self.status(&token).await;

        match status {
            CacheStatus::UpToDate => (),
            CacheStatus::RangeChanged {
                new_range: valid_range,
                new_input_selection,
            } => {
                // self.value and self.upstream persist
                self.valid_range = valid_range;
                self.input_deps.snapshot = token.input_snapshot().clone();
                if let Some(selection) = new_input_selection {
                    self.input_deps.selection = selection;
                }
            }
            CacheStatus::NeedRecalculation => {
                let current_frame = token.current_frame();
                let snapshot = token.input_snapshot().clone();
                let rt = self.rt.upgrade().ok_or(DroppedRuntimeError)?;
                let (value, handles, dep_info) = self.core.evaluate(rt, token).await;

                let ValueDepInfo {
                    since,
                    input_selection: selection,
                } = dep_info;

                // Those fields must always be updated regardless of what happens to value.
                self.upstream_deps = UpstreamDeps { handles };
                self.input_deps = InputDeps {
                    snapshot,
                    selection,
                };

                // It is possible (and perfectly valid) that `since != current_frame`.
                // Input values that were used to produce the value could be set some time ago,
                // but we didn't observe it due to laziness.
                // Because upstream values stay the same in that period of time our calculated
                // value will still be valid in those past frames.
                if self.value == value {
                    self.valid_range =
                        extend_valid_range(since..=current_frame, self.valid_range.clone());
                    // self.value persist.
                } else {
                    info!(core = ?self.core, from = ?self.value, to = ?value, "persist new value");

                    self.valid_range = since..=current_frame;
                    self.value = value;

                    return Ok(CacheUpdate::Changed);
                }
            }
        };

        Ok(CacheUpdate::Unchanged)
    }

    /// Check actuality of value persisted during `past_frame` compared to current frame.
    async fn validate(
        &mut self,
        past_frame: GlobalInstant,
        token: QueryToken,
    ) -> Result<ValidationStatus, DroppedRuntimeError> {
        let old_contains = self.valid_range.contains(&past_frame);
        let updated = self.update(token).await?;
        let new_contains = self.valid_range.contains(&past_frame);

        let r = if new_contains {
            ValidationStatus::Valid(self.dep_info())
        } else if old_contains {
            match updated {
                CacheUpdate::Changed => ValidationStatus::Invalid,
                CacheUpdate::Unchanged => ValidationStatus::Valid(self.dep_info()),
            }
        } else {
            ValidationStatus::Invalid
        };

        Ok(r)
    }

    /// Generate dependency info for currently cached value.
    fn dep_info(&self) -> ValueDepInfo {
        ValueDepInfo {
            since: self.valid_since(),
            input_selection: self.input_deps.selection.clone(),
        }
    }
}

impl<Value, Params, F> Debug for Cache<Value, Params, F>
where
    Value: Debug,
    Params: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(QueryCache))
            .field("core", &self.core)
            .field("value", &self.value)
            .field("valid_range", &self.valid_range)
            .field("input_deps", &self.input_deps)
            .field("upstream_deps", &self.upstream_deps)
            .finish_non_exhaustive()
    }
}

/// Query node task.
async fn node<Params, Value, F, Ret>(
    mut cache: Cache<Value, Params, F>,
    receiver: QueryReceiver<Value>,
) -> Result<(), DroppedRuntimeError>
where
    Params: Stashable,
    Value: Stashable,
    F: FnOnce(Params, QueryContext) -> Ret + Clone,
    Ret: Future<Output = (Value, QueryContext)>,
{
    use crate::node::ValuePayload;
    use crate::tracing::{self, Instrument};
    use tokio::select;

    info!(core = ?cache.core, value = ?cache.value, "spawn new query node");

    let QueryReceiver {
        mut value,
        mut validation,
    } = receiver;

    loop {
        select! {
            biased;

            Some(request) = value.recv() => {
                let ValueRequest {
                    token,
                    callback,
                } = request;

                let cache = &mut cache;

                let span = debug_span!("request: value", ?cache);

                async move {
                    cache.update(token).await?;

                    tracing::record_in_span("cache", &cache);

                    let value = ValuePayload {
                        value: cache.value.clone(),
                        dep_info: cache.dep_info(),
                    };

                    let _ = callback.send(value);

                    debug!("serve value");

                    Ok::<(), _>(())
                }
                .instrument(span)
                .await?;
            }
            Some(request) = validation.recv()  => {
                let ValidationRequest {
                    token,
                    past_frame,
                    callback,
                } = request;

                let cache = &mut cache;

                let span = debug_span!("request: validation", ?past_frame, ?cache);

                async move {
                    let status = cache.validate(past_frame, token).await?;

                    tracing::record_in_span("cache", &cache);
                    debug!(?status, "serve validation");

                    let _ = callback.send(status);

                    Ok::<(), _>(())
                }
                .instrument(span)
                .await?;
            }
            else => break
        }
    }

    Ok(())
}

/// Spawn query node.
fn spawn_node<Params, Value, F, Ret>(
    rt: Arc<Runtime>,
    core: Core<Params, F>,
    token: QueryToken,
) -> QueryHandle<Value>
where
    Params: Stashable,
    Value: Stashable,
    F: FnOnce(Params, QueryContext) -> Ret + Clone + Send + Sync + 'static,
    Ret: Future<Output = (Value, QueryContext)> + Send + 'static,
{
    let (handle, receiver) = super::query_channels();

    tokio::spawn(async move {
        let cache = Cache::new(rt, core, token).await;

        node(cache, receiver).await
    });

    handle
}

#[doc(hidden)]
pub async fn executor<Params, Value, F, Ret>(
    tenant: F,
    params: Params,
    cx: &QueryContext,
) -> Result<Value, ExecutorError>
where
    Params: Stashable + Hash,
    Value: Stashable,
    F: FnOnce(Params, QueryContext) -> Ret + Clone + Send + Sync + 'static,
    Ret: Future<Output = (Value, QueryContext)> + Send + 'static,
{
    use crate::node::{HashManager, ValuePayload};
    use tokio::sync::oneshot;

    let (value_out, validation_handle) = {
        let manager_handle = cx
            .runtime()
            .manager_or_insert_with(HashManager::<F, _, _>::new)
            .await;
        let query_handle = manager_handle
            .get_or_insert_with_key(params, |params| {
                let core = Core {
                    tenant,
                    params: params.clone(),
                };

                spawn_node(cx.runtime().clone(), core, cx.query_token().clone())
            })
            .await;

        let (value_in, value_out) = oneshot::channel();
        let request = ValueRequest {
            token: cx.query_token().clone(),
            callback: value_in,
        };
        query_handle
            .value
            .send(request)
            .map_err(|_| ExecutorError::QueryRequestRefused)?;

        (value_out, query_handle.validation.clone())
    };

    let ValuePayload { value, dep_info } = value_out
        .await
        .map_err(|_| ExecutorError::QueryCallbackDropped)?;

    cx.add_edge(validation_handle, dep_info);

    Ok(value)
}

#[cfg(test)]
mod test {
    use super::{
        GlobalInstant, InputDeps, InputSelection, LocalInstant, QueryToken, UpstreamDeps,
        ValidationStatus,
    };

    fn make_token<I>(current_frame: GlobalInstant, local_instants: I) -> QueryToken
    where
        I: IntoIterator<Item = LocalInstant>,
    {
        use std::sync::Arc;
        use tokio::sync::Semaphore;

        let input_snapshot = Arc::from(local_instants.into_iter().collect::<Vec<_>>());
        let semaphore = Arc::new(Semaphore::new(1));
        let guard = Arc::new(semaphore.try_acquire_owned().unwrap());

        QueryToken::new(current_frame, input_snapshot, guard)
    }

    fn make_selection<I>(bits: I) -> InputSelection
    where
        I: IntoIterator<Item = usize>,
    {
        bits.into_iter()
            .fold(InputSelection::empty(), |acc, value| {
                acc | InputSelection::new(value)
            })
    }

    fn make_input_deps<I>(bits: I, snapshot: &[LocalInstant]) -> InputDeps
    where
        I: IntoIterator<Item = usize>,
    {
        use std::sync::Arc;

        let selection = make_selection(bits);

        let snapshot = Arc::from(snapshot);

        InputDeps {
            selection,
            snapshot,
        }
    }

    fn make_upstream_deps<I>(iter: I) -> UpstreamDeps
    where
        I: IntoIterator<Item = ValidationStatus>,
    {
        let handles = iter
            .into_iter()
            .map(|status| {
                let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();

                tokio::spawn(async move {
                    while let Some(request) = receiver.recv().await {
                        use super::super::ValidationRequest;

                        let ValidationRequest {
                            token: _,
                            past_frame: _,
                            callback,
                        } = request;

                        let _ = callback.send(status.clone());
                    }
                });

                sender
            })
            .collect();

        UpstreamDeps { handles }
    }

    #[test]
    fn test_extend_range() {
        use super::extend_valid_range;
        use GlobalInstant as GI;

        {
            let range = extend_valid_range(GI::new(1)..=GI::new(3), GI::new(4)..=GI::new(10));
            let expected = GI::new(1)..=GI::new(3);

            assert_eq!(range, expected)
        }

        {
            let range = extend_valid_range(GI::new(1)..=GI::new(3), GI::new(1)..=GI::new(2));
            let expected = GI::new(1)..=GI::new(3);

            assert_eq!(range, expected)
        }

        {
            let range = extend_valid_range(GI::new(5)..=GI::new(6), GI::new(5)..=GI::new(10));
            let expected = GI::new(5)..=GI::new(6);

            assert_eq!(range, expected)
        }

        {
            let range = extend_valid_range(GI::new(5)..=GI::new(6), GI::new(2)..=GI::new(6));
            let expected = GI::new(2)..=GI::new(6);

            assert_eq!(range, expected)
        }

        {
            let range = extend_valid_range(GI::new(5)..=GI::new(6), GI::new(2)..=GI::new(5));
            let expected = GI::new(2)..=GI::new(6);

            assert_eq!(range, expected)
        }

        {
            let range = extend_valid_range(GI::new(5)..=GI::new(6), GI::new(2)..=GI::new(4));
            let expected = GI::new(2)..=GI::new(6);

            assert_eq!(range, expected)
        }

        {
            let range = extend_valid_range(GI::new(5)..=GI::new(6), GI::new(2)..=GI::new(3));
            let expected = GI::new(5)..=GI::new(6);

            assert_eq!(range, expected)
        }
    }

    mod input_deps {
        use super::super::{GlobalInstant as GI, InputStatus, LocalInstant as LI};
        use super::{make_input_deps, make_token};
        use InputStatus::*;

        // Test that constant queries (i.e. with no input dependencies) are always considered valid.
        #[test]
        fn test_constant() {
            {
                // No inputs.

                let deps = make_input_deps([], &[]);
                let token = make_token(GI::new(1), []);

                assert_eq!(deps.status(token.current_frame(), &token), Some(Unchanged));
                assert_eq!(deps.status(GI::new(1), &token), Some(Unchanged));
                assert_eq!(deps.status(GI::initial(), &token), Some(Unchanged));
            }

            {
                // Same snapshot.

                let local_instants = [LI::initial(), LI::new(2)];

                let deps = make_input_deps([], &local_instants);
                let token = make_token(GI::new(2), local_instants);

                assert_eq!(deps.status(token.current_frame(), &token), Some(Unchanged));
                assert_eq!(deps.status(GI::new(1), &token), Some(Unchanged));
                assert_eq!(deps.status(GI::initial(), &token), Some(Unchanged));
            }

            {
                // Different snapshot.

                let instants1 = [LI::initial(), LI::new(2)];
                let instants2 = [LI::new(1), LI::new(2), LI::initial()];

                let deps = make_input_deps([], &instants1);
                let token = make_token(GI::new(2), instants2);

                assert_eq!(deps.status(token.current_frame(), &token), Some(Unchanged));
                assert_eq!(deps.status(GI::new(1), &token), Some(Unchanged));
                assert_eq!(deps.status(GI::initial(), &token), Some(Unchanged));
            }
        }

        // Test behavior on outdated queries.
        #[test]
        fn test_local_instant_wrapping() {
            {
                // Constants are not affected.

                let instants = [LI::new(1), LI::new(2)];

                let deps = make_input_deps([], &instants);
                let token = make_token(GI::new(3) + LI::MAX_GLOBAL_FRAMES, instants);

                assert_eq!(deps.status(GI::initial(), &token), Some(Unchanged));
                assert_eq!(deps.status(GI::new(3), &token), Some(Unchanged));
                assert_eq!(deps.status(GI::new(4), &token), Some(Unchanged));
            }

            {
                // Queries outdated by more than `LI::MAX_GLOBAL_FRAMES` cannot be verified.

                let instants = [LI::new(10), LI::new(11), LI::new(100)];

                let deps = make_input_deps([1], &instants);
                let token = make_token(GI::new(4) + LI::MAX_GLOBAL_FRAMES, instants);

                assert_eq!(deps.status(GI::initial(), &token), None);
                assert_eq!(deps.status(GI::new(4), &token), None);
                assert_eq!(deps.status(GI::new(5), &token), Some(Unchanged));
            }
        }

        // Test non-constant queries.
        #[test]
        fn test_normal() {
            let cache_instants = [LI::new(1), LI::new(2), LI::new(3)];

            let deps = make_input_deps([1, 2], &cache_instants);

            assert!(!deps.selection.is_empty());

            {
                // Same snapshot.

                let token = make_token(GI::new(3), cache_instants);

                assert_eq!(deps.status(token.current_frame(), &token), Some(Unchanged));
                assert_eq!(deps.status(GI::new(2), &token), Some(Unchanged));
                assert_eq!(deps.status(GI::initial(), &token), Some(Unchanged));
            }

            {
                // Different snapshot, dependencies preserved.

                let instants = [LI::new(2), LI::new(2), LI::new(3), LI::initial()];
                let token = make_token(GI::new(3), instants);

                assert_eq!(deps.status(token.current_frame(), &token), Some(Unchanged));
                assert_eq!(deps.status(GI::new(2), &token), Some(Unchanged));
                assert_eq!(deps.status(GI::initial(), &token), Some(Unchanged));
            }

            {
                // Different snapshot, dependencies changed.

                let instants = [LI::new(1), LI::new(2), LI::new(4)];
                let token = make_token(GI::new(3), instants);

                assert_eq!(deps.status(token.current_frame(), &token), Some(Changed));
                assert_eq!(deps.status(GI::new(2), &token), Some(Changed));
                assert_eq!(deps.status(GI::initial(), &token), Some(Changed));
            }

            {
                // Empty snapshot, dependencies changed.

                let token = make_token(GI::new(3), []);

                assert_eq!(deps.status(token.current_frame(), &token), Some(Changed));
                assert_eq!(deps.status(GI::new(2), &token), Some(Changed));
                assert_eq!(deps.status(GI::initial(), &token), Some(Changed));
            }
        }
    }

    mod upstream_deps {
        use super::super::{
            GlobalInstant, QueryToken, UpstreamStatus, ValidationStatus, ValueDepInfo,
        };
        use super::make_upstream_deps;
        use UpstreamStatus::*;

        fn make_token() -> QueryToken {
            super::make_token(GlobalInstant::new(10), [])
        }

        #[tokio::test]
        async fn test_empty() {
            let deps = make_upstream_deps([]);
            let token = make_token();

            assert_eq!(
                deps.status(token.current_frame(), &token).await,
                Valid(ValueDepInfo::trivial())
            );
        }

        #[tokio::test]
        async fn test_invalid() {
            let token = make_token();

            {
                let deps = make_upstream_deps([ValidationStatus::Invalid]);
                assert_eq!(deps.status(GlobalInstant::initial(), &token).await, Invalid);
            }

            {
                let deps = make_upstream_deps([
                    ValidationStatus::Valid(ValueDepInfo::trivial()),
                    ValidationStatus::Invalid,
                ]);
                assert_eq!(deps.status(GlobalInstant::initial(), &token).await, Invalid);
            }
        }

        #[tokio::test]
        async fn test_info_folding() {
            use super::super::InputSelection;

            let token = make_token();

            let deps = make_upstream_deps([
                ValidationStatus::Valid(ValueDepInfo {
                    since: GlobalInstant::new(1),
                    input_selection: InputSelection::new(2),
                }),
                ValidationStatus::Valid(ValueDepInfo {
                    since: GlobalInstant::new(100),
                    input_selection: InputSelection::new(3),
                }),
                ValidationStatus::Valid(ValueDepInfo {
                    since: GlobalInstant::new(3),
                    input_selection: InputSelection::new(2),
                }),
                ValidationStatus::Valid(ValueDepInfo {
                    since: GlobalInstant::new(10),
                    input_selection: InputSelection::new(8),
                }),
            ]);

            let status = deps.status(GlobalInstant::new(101), &token).await;

            let expected = ValueDepInfo {
                since: GlobalInstant::new(100),
                input_selection: InputSelection::new(2)
                    | InputSelection::new(3)
                    | InputSelection::new(8),
            };

            assert_eq!(status, Valid(expected));
        }
    }

    mod cache_status {
        use super::super::{Cache, CacheStatus, Core, GlobalInstant as GI, LocalInstant as LI};
        use super::{make_input_deps, make_selection, make_token, make_upstream_deps};
        use std::sync::Weak;

        #[tokio::test]
        async fn test_same_instant() {
            let cache = Cache {
                rt: Weak::new(),
                core: Core {
                    tenant: |_, cx| async move { (3_usize, cx) },
                    params: (),
                },
                value: 1_usize,
                valid_range: GI::new(1)..=GI::new(3),
                upstream_deps: make_upstream_deps([]),
                input_deps: make_input_deps([], &[]),
            };

            let token = make_token(GI::new(3), []);

            assert_eq!(cache.status(&token).await, CacheStatus::UpToDate);
        }

        #[tokio::test]
        #[should_panic]
        async fn test_past_instant() {
            let cache = Cache {
                rt: Weak::new(),
                core: Core {
                    tenant: |_, cx| async move { (3_usize, cx) },
                    params: (),
                },
                value: 1_usize,
                valid_range: GI::new(1)..=GI::new(3),
                upstream_deps: make_upstream_deps([]),
                input_deps: make_input_deps([], &[]),
            };

            let token = make_token(GI::new(2), []);

            let _ = cache.status(&token).await;
        }

        #[tokio::test]
        async fn test_unchanged_inputs() {
            let cache = Cache {
                rt: Weak::new(),
                core: Core {
                    tenant: |_, cx| async move { (3_usize, cx) },
                    params: (),
                },
                value: 1_usize,
                valid_range: GI::new(1)..=GI::new(3),
                upstream_deps: make_upstream_deps([]),
                input_deps: make_input_deps([1], &[LI::new(1), LI::new(2)]),
            };

            let token = make_token(GI::new(5), [LI::new(3), LI::new(2), LI::initial()]);

            let expected = CacheStatus::RangeChanged {
                new_range: GI::new(1)..=GI::new(5),
                new_input_selection: None,
            };

            assert_eq!(cache.status(&token).await, expected);
        }

        #[tokio::test]
        async fn test_valid_upstream_touching() {
            use super::super::{ValidationStatus::*, ValueDepInfo};

            let cache = Cache {
                rt: Weak::new(),
                core: Core {
                    tenant: |_, cx| async move { (3_usize, cx) },
                    params: (),
                },
                value: 1_usize,
                valid_range: GI::new(1)..=GI::new(3),
                upstream_deps: make_upstream_deps([
                    Valid(ValueDepInfo {
                        since: GI::new(4),
                        input_selection: make_selection([0]),
                    }),
                    Valid(ValueDepInfo {
                        since: GI::new(3),
                        input_selection: make_selection([]),
                    }),
                ]),
                input_deps: make_input_deps([0], &[LI::new(1)]),
            };

            let token = make_token(GI::new(5), [LI::new(3)]);

            let expected = CacheStatus::RangeChanged {
                new_range: GI::new(1)..=GI::new(5),
                new_input_selection: Some(make_selection([0])),
            };

            assert_eq!(cache.status(&token).await, expected);
        }

        #[tokio::test]
        async fn test_valid_upstream_discontinuous() {
            use super::super::{ValidationStatus::*, ValueDepInfo};

            let cache = Cache {
                rt: Weak::new(),
                core: Core {
                    tenant: |_, cx| async move { (3_usize, cx) },
                    params: (),
                },
                value: 1_usize,
                valid_range: GI::new(1)..=GI::new(3),
                upstream_deps: make_upstream_deps([
                    Valid(ValueDepInfo {
                        since: GI::new(5),
                        input_selection: make_selection([1]),
                    }),
                    Valid(ValueDepInfo {
                        since: GI::new(4),
                        input_selection: make_selection([]),
                    }),
                ]),
                input_deps: make_input_deps([0], &[LI::new(1)]),
            };

            let token = make_token(GI::new(6), [LI::new(3)]);

            let expected = CacheStatus::RangeChanged {
                new_range: GI::new(5)..=GI::new(6),
                new_input_selection: Some(make_selection([1])),
            };

            assert_eq!(cache.status(&token).await, expected);
        }

        #[tokio::test]
        async fn test_invalid_upstream() {
            use super::super::ValidationStatus::*;

            let cache = Cache {
                rt: Weak::new(),
                core: Core {
                    tenant: |_, cx| async move { (3_usize, cx) },
                    params: (),
                },
                value: 1_usize,
                valid_range: GI::new(1)..=GI::new(3),
                upstream_deps: make_upstream_deps([Invalid]),
                input_deps: make_input_deps([0], &[LI::new(1)]),
            };

            let token = make_token(GI::new(6), [LI::new(3)]);

            assert_eq!(cache.status(&token).await, CacheStatus::NeedRecalculation);
        }
    }
}
