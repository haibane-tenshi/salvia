//! Input details.
//!
//! Inputs are special "functions" which can store a user-provided value and indicate to dependent
//! queries when its value was changed.
//!
//! # Design: input as a trait
//!
//! Here's a loose collection of thoughts which led to this design.
//!
//! ## `initial` fn
//!
//! Input nodes have an awkward problem: if values are explicitly set by user, what happens when
//! input is queried *before* it is set for the first time?
//!
//! It is tempting to just honestly panic however this is a *really* bad choice: any query from
//! any point of your program theoretically can access any input, this can happen in transitive
//! dependencies too, but it is the user who shoulder responsibility for properly setting inputs up.
//! This leads to a situation where global reasoning throughout all dependencies is required
//! to be sure that things don't fall over randomly.
//!
//! `salvia` takes more strict approach: it guarantees that it is *always* sound to run getter
//! on an input and to achieve that it asks you to provide initialization function.
//!
//! ## Two access points
//!
//! Unlike queries inputs have two ways to access them: to read the value and
//! to put the new value in.
//! This makes it awkward to generate implementation with an attribute macro (like queries)
//! as it have to occupy extra names in the scope and those names need to be made discoverable.
//!
//! ## Solution: trait
//!
//! What input is trying to achieve is to map `initial()` function onto a predefined set of
//! functionality (`set` and `get` pair).
//! The typical solution for this in Rust is to use traits: it clearly indicates override points
//! and gives overview of provided API.
//! In fact there are some incredibly successful cases of such usage,
//! for example [`Iterator`] trait.
//!
//! However, the approach does impose limitations.
//! First, a *host* type is required for trait to attach to.
//! Second, it is currently impossible support variadic functions inside traits,
//! so inputs cannot take arbitrary arguments like queries.
//!
//! `salvia` solves variadic argument by stating that *the only* allowed parameter is `self`.
//! So when you need to parametrize input you have to pack the data into host type.
//!
//! This approach results in sort of inverted function calls: normally you call function by passing
//! arguments in, but here you need to instantiate arguments first and call function on them.
//!
//! # Idealized implementation
//!
//! Conceptually input can be represented as the following trait:
//!
//! ```ignore
//! trait Input<Value>: Stashable + Hash
//!     where Value: Stashable
//! {
//!     fn initial(&self) -> Value;
//!
//!     async fn get(self, cx: &QueryContext) -> Value {
//!         // ...
//!     }
//!
//!     async fn set(self, value: Value, cx: &InputContext) {
//!         // ...
//!     }
//! }
//! ```
//!
//! Here:
//!
//! *   `Self` indicates the host type and also type of the only parameter input has
//! *   `Value` generic param indicates type of values stored
//! *   `initial` function is called once per `self` value to initialize the storage
//! *   `get` and `set` provide a way to access/mutate value respectively.
//!
//! Trait only requires you to provide `initial` function.
//!
//! Unfortunately this design has a problem: `async fn` is not allowed in traits (yet).
//! This can be alleviated by [`async_trait`] crate but pinboxing every call might get expensive.
//!
//! [docs.rs:async_trait]: https://docs.rs/async-trait/latest/async_trait/
//!
//! # Current implementation
//!
//! To avoid mentioned issue inputs implement a more complex structure:
//!
//! *   [`Input`] trait specifies `Value` param and `initial` function.
//!     It is used to indicate that type is hosting an input but doesn't provide any `set`/`get`
//!     implementations.
//! *   [`InputAccess`](macro@InputAccess) macro, which implements generic `set`/`get` directly
//!     on the host.
//!     Unlike with traits, implementing `async fn` on type is perfectly legal and doesn't
//!     impose extra overhead.
//! *   [`InputAccess`](trait@InputAccess) trait (enabled by `async-trait` feature)
//!     provides `set` and `get` implementation in form of a trait.
//!     It relies on `async_trait` crate and
//!     is automatically derived for all types implementing `Input`,
//!     so only consumers which need it have to opt-in.
//!
//! You should apply derive macro to your input host type
//! and let user decide which of two APIs to use.

use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::node::{
    ExecutorError, InputHandle, InputReceiver, SetValueRequest, ValidationRequest, ValueRequest,
};
use crate::time::{GlobalInstant, LocalInstant};
use crate::Stashable;
use crate::{InputContext, QueryContext, Runtime};

pub use salvia_macro::InputAccess;

/// Mark type as a host to an input.
pub trait Input<Value>: Stashable + Hash
where
    Value: Stashable,
{
    /// Provide initial value.
    ///
    /// This function will be called to produce a value in case a query tried to access unset input.
    ///
    /// There is no other guarantees about how this functions is used.
    /// It is possible that the function is called multiple times per bound input,
    /// called before setter or even after input was explicitly set.
    fn initial(&self) -> Value;
}

/// Provide setter and getter for an input.
///
/// This trait is disabled by default and enabled by `async-trait` feature.
///
/// This trait is automatically implemented for all types implementing [`Input`] trait.
#[cfg(any(feature = "async-trait", doc))]
#[cfg_attr(feature = "nightly", doc(cfg(feature = "async-trait")))]
#[async_trait::async_trait]
pub trait InputAccess<Value>: Input<Value>
where
    Value: Stashable,
{
    /// Set a new value.
    async fn set(self, value: Value, cx: &InputContext) {
        set_executor(self, value, cx).await.unwrap()
    }

    /// Acquire currently stored value.
    async fn get(self, cx: &QueryContext) -> Value {
        get_executor(self, cx).await.unwrap()
    }
}

#[cfg(any(feature = "async-trait", doc))]
impl<T, Value> InputAccess<Value> for T
where
    T: Input<Value>,
    Value: Stashable,
{
}

/// Value marked with time since when it is actual.
#[derive(Debug)]
struct TimedValue<Value> {
    value: Value,
    valid_since: GlobalInstant,
}

/// Cache of input node.
#[derive(Debug)]
struct Cache<Host, Value> {
    host: Host,
    timed_value: TimedValue<Value>,
    next_value: Option<TimedValue<Value>>,
    instant: LocalInstant,
}

impl<Host, Value> Cache<Host, Value>
where
    Host: Input<Value>,
    Value: Stashable,
{
    /// Create new cache.
    ///
    /// It is assumed that value is valid since `GlobalInstant::initial()` moment.
    pub fn new(host: &Host) -> Self {
        let timed_value = TimedValue {
            value: host.initial(),
            valid_since: GlobalInstant::initial(),
        };

        Self {
            host: host.clone(),
            timed_value,
            next_value: None,
            instant: LocalInstant::initial(),
        }
    }

    /// Queue a change in value.
    ///
    /// This function does not modify current value.
    /// Previously queued value is discarded.
    /// Queued change will be performed during `actualize()`.
    pub fn set_next(&mut self, value: Value, valid_since: GlobalInstant) {
        self.next_value = Some(TimedValue { value, valid_since });
    }

    /// Apply queued value change.
    pub fn actualize(&mut self) {
        if let Some(timed_value) = self.next_value.take() {
            if self.timed_value.value != timed_value.value {
                info!(host = ?self.host, from = ?self.timed_value, to = ?timed_value, "persist new value");

                self.timed_value = timed_value;
                self.instant = self.instant.next();
            }
        }
    }

    /// Moment in time since currently persisting value is valid.
    pub fn valid_since(&self) -> GlobalInstant {
        self.timed_value.valid_since
    }
}

/// Node task for inputs.
async fn node<Host, Value>(
    rt: Arc<Runtime>,
    mut cache: Cache<Host, Value>,
    receiver: InputReceiver<Value>,
) where
    Host: Input<Value>,
    Value: Stashable,
{
    use crate::node::{ValueDepInfo, ValuePayload};
    use tokio::select;

    let (self_deps, mut clock_out) = {
        use crate::node::InputSelection;
        use crate::tracing::Instrument;

        let span = debug_span!("requesting index from clock", ?cache);

        async move {
            let (input_handle, clock_out) = mpsc::channel(1);
            let index = rt.register_input(input_handle).await;

            let self_deps = InputSelection::new(index);

            debug!(?index, "acquire index");

            (self_deps, clock_out)
        }
        .instrument(span)
        .await
    };

    info!(?cache, "spawn new input node");

    let InputReceiver {
        mut set_value,
        mut value,
        mut validation,
    } = receiver;

    loop {
        select! {
            biased;

            Some(request) = set_value.recv() => {
                let SetValueRequest {
                    value,
                    token,
                } = request;

                debug!(?cache, new_value = ?value, "set new value");

                cache.set_next(value, token.current_frame());
            }
            Some(request) = validation.recv() => {
                use crate::node::{ValidationStatus, ValueDepInfo};

                let ValidationRequest {
                    past_frame,
                    callback,
                    ..
                } = request;

                debug_assert!(cache.next_value.is_none());

                let since = cache.timed_value.valid_since;
                let status = if past_frame < since {
                    ValidationStatus::Invalid
                } else {
                    let dep_info = ValueDepInfo {
                        since,
                        input_selection: self_deps.clone()
                    };

                    ValidationStatus::Valid(dep_info)
                };

                debug!(?cache, ?past_frame, ?status, "serve validation");

                let _ = callback.send(status);
            }
            Some(payload) = value.recv() => {
                let dep_info = ValueDepInfo {
                    since: cache.valid_since(),
                    input_selection: self_deps.clone(),
                };

                let value_payload = ValuePayload {
                    value: cache.timed_value.value.clone(),
                    dep_info,
                };

                debug_assert!(cache.next_value.is_none());

                debug!(?cache, "serve value");

                let _ = payload.callback.send(value_payload);
            }
            Some(request) = clock_out.recv() => {
                use crate::node::LocalInstantRequest;

                let LocalInstantRequest {
                    callback,
                } = request;

                debug!(?cache, "actualize");

                // Clock request happens right before a frame starts, so we only need to actualize
                // the value at this point.
                cache.actualize();

                let _ = callback.send(cache.instant);
            }
            else => break,
        }
    }
}

/// Spawn a new node.
fn spawn_node<Host, Value>(rt: Arc<Runtime>, host: &Host) -> InputHandle<Value>
where
    Host: Input<Value>,
    Value: Stashable,
{
    let (handle, receiver) = super::input_channels();

    let cache = Cache::new(host);

    tokio::spawn(node(rt, cache, receiver));

    handle
}

#[doc(hidden)]
pub async fn set_executor<Host, Value>(
    host: Host,
    value: Value,
    cx: &InputContext,
) -> Result<(), ExecutorError>
where
    Host: Input<Value>,
    Value: Stashable,
{
    use crate::node::HashManager;

    let manager_handle = cx
        .runtime()
        .manager_or_insert_with(HashManager::<Host, _, _>::new)
        .await;
    let query_handle = manager_handle
        .get_or_insert_with_key(host, |host| spawn_node(cx.runtime().clone(), host))
        .await;

    let request = SetValueRequest {
        token: cx.input_token().clone(),
        value,
    };
    query_handle
        .set_value
        .send(request)
        .map_err(|_| ExecutorError::QueryRequestRefused)?;

    Ok(())
}

#[doc(hidden)]
pub async fn get_executor<Host, Value>(
    host: Host,
    cx: &QueryContext,
) -> Result<Value, ExecutorError>
where
    Host: Input<Value>,
    Value: Stashable,
{
    use crate::node::{HashManager, ValuePayload};
    use tokio::sync::oneshot;

    let (out, validation_handle) = {
        let manager_handle = cx
            .runtime()
            .manager_or_insert_with(HashManager::<Host, _, _>::new)
            .await;
        let query_handle = manager_handle
            .get_or_insert_with_key(host, |host| spawn_node(cx.runtime().clone(), host))
            .await;

        let (callback, out) = oneshot::channel();

        let request = ValueRequest {
            token: cx.query_token().clone(),
            callback,
        };
        query_handle
            .value
            .send(request)
            .map_err(|_| ExecutorError::QueryRequestRefused)?;

        (out, query_handle.validation.clone())
    };

    let ValuePayload { value, dep_info } =
        out.await.map_err(|_| ExecutorError::QueryCallbackDropped)?;

    cx.add_edge(validation_handle, dep_info);

    Ok(value)
}

#[cfg(test)]
mod test {
    use salvia::{Input, InputAccess, Runtime};

    // Unset inputs should be initialized using provided function.
    #[tokio::test]
    async fn test_initial_value() {
        #[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, InputAccess)]
        struct Tree(u32);

        impl Input<String> for Tree {
            fn initial(&self) -> String {
                format!("i'm a tree #{}!", self.0)
            }
        }

        let rt = Runtime::new().await;

        rt.query(|cx| async move {
            assert_eq!(Tree(3).get(&cx).await.as_str(), "i'm a tree #3!");
            assert_eq!(Tree(125).get(&cx).await.as_str(), "i'm a tree #125!");
        })
        .await;
    }

    // Mutated inputs should change its value.
    #[tokio::test]
    async fn test_setter() {
        #[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, InputAccess)]
        struct Tree(u32);

        impl Input<String> for Tree {
            fn initial(&self) -> String {
                format!("i'm a tree #{}!", self.0)
            }
        }

        let rt = Runtime::new().await;

        rt.mutate(|cx| async move {
            Tree(3).set("a stump".to_string(), &cx).await;
            Tree(178).set("a cristmas tree".to_string(), &cx).await;
        })
        .await;

        rt.query(|cx| async move {
            assert_eq!(Tree(3).get(&cx).await.as_str(), "a stump");
            assert_eq!(Tree(5).get(&cx).await, "i'm a tree #5!");
            assert_eq!(Tree(178).get(&cx).await.as_str(), "a cristmas tree");
        })
        .await;
    }

    mod cache {
        use super::super::{Cache, Input};
        use crate::time::{GlobalInstant, LocalInstant};

        #[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
        struct Integer;

        impl Input<u64> for Integer {
            fn initial(&self) -> u64 {
                3
            }
        }

        #[test]
        fn test_no_change_roll() {
            let mut cache = Cache::new(&Integer);

            cache.actualize();

            assert_eq!(cache.timed_value.value, 3);
            assert_eq!(cache.instant, LocalInstant::initial());
            assert!(cache.next_value.is_none());
        }

        #[test]
        fn test_roll_with_change() {
            let mut cache = Cache::new(&Integer);

            let next_time = GlobalInstant::initial().next().next();
            cache.set_next(10, next_time);

            assert_eq!(cache.timed_value.value, 3);
            assert_eq!(cache.timed_value.valid_since, GlobalInstant::initial());
            assert_eq!(cache.instant, LocalInstant::initial());

            cache.actualize();

            assert_eq!(cache.timed_value.value, 10);
            assert_eq!(cache.timed_value.valid_since, next_time);
            assert_eq!(cache.instant, LocalInstant::initial().next());
            assert!(cache.next_value.is_none());
        }

        #[test]
        fn test_roll_with_change_to_equal() {
            let mut cache = Cache::new(&Integer);

            let next_time = GlobalInstant::initial().next().next();
            cache.set_next(3, next_time);

            assert_eq!(cache.timed_value.value, 3);
            assert_eq!(cache.timed_value.valid_since, GlobalInstant::initial());
            assert_eq!(cache.instant, LocalInstant::initial());

            cache.actualize();

            assert_eq!(cache.instant, LocalInstant::initial());
            assert_eq!(cache.timed_value.value, 3);
            assert_eq!(cache.timed_value.valid_since, GlobalInstant::initial());
            assert!(cache.next_value.is_none());
        }
    }
}
