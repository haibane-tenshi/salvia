//! Query and input details, storage tasks (*nodes*).

use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;
use std::marker::PhantomData;
use std::ops::{BitOr, BitOrAssign};

use bitvec::vec::BitVec;
use tokio::sync::{mpsc, oneshot};
use tokio::sync::{RwLock, RwLockReadGuard};

use crate::runtime::{InputToken, QueryToken};
use crate::time::{GlobalInstant, LocalInstant};

pub mod input;
pub mod query;

/// Bit vec for tracking which inputs a cached value depends on.
///
/// Every input node is assigned a unique bit in this type by clock task.
///
/// Internal implementation defers to [`BitVec`], however implementation of `|` and `|=` is
/// semantically different:
/// in original implementation result assumes length of left argument which can lead
/// to lost bits, however this class makes sure to expand resulting `BitVec` to fit.
#[derive(Clone, Eq, PartialEq, Hash, Default)]
struct InputSelection(BitVec);

impl InputSelection {
    pub fn empty() -> Self {
        Default::default()
    }

    /// Create bitvector with `n`-th bit set
    pub fn new(n: usize) -> Self {
        let v = {
            use bitvec::bitvec;

            let mut v = bitvec![0; n];
            v.push(true);

            v
        };

        Self(v)
    }

    /// Check whether there is no inputs in selection.
    pub fn is_empty(&self) -> bool {
        // This function is semantically equivalent to `BitVec::not_any()`.
        // However, the way new snapshots can be constructed, we can guarantee that
        // every non-empty snapshot has a `1` in the end
        // and it is impossible to reset `1` bits within current (v0.1) API surface.
        // Emptiness check is way cheaper for non-empty bitvecs - and that is majority of cases.
        self.0.is_empty()
    }

    /// Iterate over individual bits.
    pub fn iter(&self) -> bitvec::slice::Iter<'_, bitvec::order::Lsb0, usize> {
        self.0.iter()
    }
}

impl BitOrAssign for InputSelection {
    /// Performs `|=` operation.
    ///
    /// `self` will expand to fit when `rhs` has greater length.
    fn bitor_assign(&mut self, mut rhs: Self) {
        // BitOrAssign on BitVec doesn't expand self when rhs is longer so we swap it in this case.
        if self.0.len() < rhs.0.len() {
            std::mem::swap(&mut self.0, &mut rhs.0);
        }

        self.0 |= rhs.0;
    }
}

impl BitOr for InputSelection {
    type Output = InputSelection;

    /// Performs `|` operation.
    ///
    /// Result will assume the longest length of either operand.
    fn bitor(mut self, rhs: Self) -> Self::Output {
        self |= rhs;

        self
    }
}

impl Debug for InputSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        struct Wrapper<'a>(&'a InputSelection);

        impl<'a> Debug for Wrapper<'a> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0 .0)
            }
        }

        f.debug_tuple(stringify!(InputSelection))
            .field(&Wrapper(self))
            .finish()
    }
}

/// Type for recording cache's upstream dependency information.
#[derive(Debug, Default)]
pub(crate) struct UpstreamEdges {
    validation: Vec<QueryValidationHandle>,
    dep_info: ValueDepInfo,
}

impl UpstreamEdges {
    /// Add dependency info of a node.
    pub(crate) fn add_edge(&mut self, validation: QueryValidationHandle, dep_info: ValueDepInfo) {
        self.validation.push(validation);
        self.dep_info |= dep_info;
    }
}

/// Node handle for sending validation requests.
pub(crate) type QueryValidationHandle = mpsc::UnboundedSender<ValidationRequest>;

/// Collection of channels to access input node.
#[derive(Debug, Clone)]
struct InputHandle<Value> {
    set_value: mpsc::UnboundedSender<SetValueRequest<Value>>,
    value: mpsc::UnboundedSender<ValueRequest<Value>>,
    validation: QueryValidationHandle,
}

/// Collection of channels to receive input node requests.
#[derive(Debug)]
struct InputReceiver<Value> {
    set_value: mpsc::UnboundedReceiver<SetValueRequest<Value>>,
    value: mpsc::UnboundedReceiver<ValueRequest<Value>>,
    validation: mpsc::UnboundedReceiver<ValidationRequest>,
}

/// Create channels pertaining to input node.
fn input_channels<Value>() -> (InputHandle<Value>, InputReceiver<Value>) {
    let (set_value_in, set_value_out) = mpsc::unbounded_channel();
    let (value_in, value_out) = mpsc::unbounded_channel();
    let (validation_in, validation_out) = mpsc::unbounded_channel();

    let handle = InputHandle {
        set_value: set_value_in,
        value: value_in,
        validation: validation_in,
    };

    let receiver = InputReceiver {
        set_value: set_value_out,
        value: value_out,
        validation: validation_out,
    };

    (handle, receiver)
}

/// Collection of channels to access query node.
#[derive(Debug, Clone)]
struct QueryHandle<Value> {
    value: mpsc::UnboundedSender<ValueRequest<Value>>,
    validation: mpsc::UnboundedSender<ValidationRequest>,
}

/// Collection of channels to receive query node requests.
#[derive(Debug)]
struct QueryReceiver<Value> {
    value: mpsc::UnboundedReceiver<ValueRequest<Value>>,
    validation: mpsc::UnboundedReceiver<ValidationRequest>,
}

/// Create channels pertaining to query node.
fn query_channels<Value>() -> (QueryHandle<Value>, QueryReceiver<Value>) {
    let (value_in, value_out) = mpsc::unbounded_channel();
    let (validation_in, validation_out) = mpsc::unbounded_channel();

    let handle = QueryHandle {
        value: value_in,
        validation: validation_in,
    };

    let receiver = QueryReceiver {
        value: value_out,
        validation: validation_out,
    };

    (handle, receiver)
}

/// Request a current `LocalInstant` from an input.
#[derive(Debug)]
pub(crate) struct LocalInstantRequest {
    pub callback: oneshot::Sender<LocalInstant>,
}

/// Record a new value into input node.
#[derive(Debug)]
struct SetValueRequest<Value> {
    token: InputToken,
    value: Value,
}

/// Request current value of a node.
#[derive(Debug)]
struct ValueRequest<Value> {
    token: QueryToken,
    callback: oneshot::Sender<ValuePayload<Value>>,
}

/// Response to value request.
#[derive(Debug)]
struct ValuePayload<Value> {
    value: Value,
    dep_info: ValueDepInfo,
}

/// Associated information about specific value.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Default)]
pub(crate) struct ValueDepInfo {
    /// Moment in time since when value is assumed to be valid.
    ///
    /// Generally value requests performed inside framework require a `QueryToken`
    /// which embeds information about current frame.
    /// For this reason the end point of valid range is not included, it is assumed to be always
    /// known to caller.
    /// So you can assume valid range to be `since..=token.current_frame()`.
    since: GlobalInstant,

    /// Bit markers for all transitive inputs which were used to calculate the value.
    ///
    /// Local times (which are complementary to bits) are not communicated because those are also
    /// embedded into token.
    ///
    /// **Note:** you should assume that those bits are only valid for *current frame*.
    /// It is totally possible for value to be recalculated with different inputs but stay the same.
    /// Nodes simply join valid intervals on such occasion which actively helps to remove
    /// extra recalculations.
    input_selection: InputSelection,
}

impl ValueDepInfo {
    /// Info for a value which stays valid for any inputs at any moment in time.
    pub fn trivial() -> Self {
        ValueDepInfo {
            since: Default::default(),
            input_selection: Default::default(),
        }
    }
}

impl BitOr for ValueDepInfo {
    type Output = ValueDepInfo;

    fn bitor(mut self, rhs: Self) -> Self::Output {
        self |= rhs;
        self
    }
}

impl BitOrAssign for ValueDepInfo {
    fn bitor_assign(&mut self, rhs: Self) {
        self.since = self.since.max(rhs.since);
        self.input_selection |= rhs.input_selection;
    }
}

/// Request node to update its cache.
#[derive(Debug)]
pub(crate) struct ValidationRequest {
    token: QueryToken,

    /// Time when requesting node have seen the value of target node.
    ///
    /// Target node is responsible for detecting whether value from specified frame is still
    /// actual or has changed and communicating that back in response.
    past_frame: GlobalInstant,

    callback: oneshot::Sender<ValidationStatus>,
}

/// Status of node value present during `ValidationRequest::past_frame`.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
enum ValidationStatus {
    /// Cache value was invalidated.
    Invalid,

    /// Cache value is still actual.
    ///
    /// We have to report value's dependency info back because we don't know how this information
    /// compares to caller.
    Valid(ValueDepInfo),
}

/// Errors returned by the executor.
///
/// All of them are induced by target node not responding in good way.
/// This can only happen when node task is already terminated.
#[doc(hidden)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum ExecutorError {
    QueryRequestRefused,
    QueryCallbackDropped,
}

/// Node manager using `HashMap` internally.
struct HashManager<Marker, Params, Handle> {
    marker: PhantomData<Marker>,
    storage: RwLock<HashMap<Params, Handle>>,
}

impl<Marker, Params, Handle> HashManager<Marker, Params, Handle>
where
    Params: Clone + Eq + Hash,
{
    fn new() -> Self {
        Self {
            marker: PhantomData,
            storage: Default::default(),
        }
    }

    async fn get_or_insert_with_key<F>(&self, params: Params, f: F) -> RwLockReadGuard<'_, Handle>
    where
        F: FnOnce(&Params) -> Handle,
    {
        let guard = self.storage.read().await;
        if let Ok(guard) = RwLockReadGuard::try_map(guard, |t| t.get(&params)) {
            return guard;
        }

        // This is some unfortunate API interaction here.
        // We cannot downgrade a mapped mutable guard because of potential soundness issue.
        // This means input have to live long enough to index into map after node is produced.
        // However this also means we need to keep an extra copy of `input` around:
        // both key and node already consumed one at that point.
        let mut guard = self.storage.write().await;
        guard.entry(params.clone()).or_insert_with_key(f);
        RwLockReadGuard::map(guard.downgrade(), |t| t.get(&params).unwrap())
    }
}

#[cfg(test)]
mod test {
    mod input_selection {
        use super::super::InputSelection;

        #[test]
        fn test_empty() {
            {
                let selection = InputSelection::empty();

                assert!(selection.is_empty());
                assert!(selection.0.is_empty());
            }
            {
                let selection = InputSelection::default();

                assert!(selection.is_empty());
                assert!(selection.0.is_empty());
            }
            {
                let selection = InputSelection::new(0);

                assert!(!selection.is_empty());
                assert!(!selection.0.is_empty());
            }
        }

        #[test]
        fn test_new() {
            {
                let selection = InputSelection::new(0);

                assert_eq!(selection.0.get(0).as_deref(), Some(&true));
                assert_eq!(selection.0.count_ones(), 1);
            }
            {
                let selection = InputSelection::new(1000);

                assert_eq!(selection.0.get(1000).as_deref(), Some(&true));
                assert_eq!(selection.0.count_ones(), 1);
            }
        }

        #[test]
        fn test_bitor() {
            {
                let selection = InputSelection::empty() | InputSelection::new(1);

                assert_eq!(selection.0.get(1).as_deref(), Some(&true));
                assert_eq!(selection.0.count_ones(), 1);
            }
            {
                let selection = InputSelection::new(1) | InputSelection::new(1);

                assert_eq!(selection.0.get(1).as_deref(), Some(&true));
                assert_eq!(selection.0.count_ones(), 1);
            }
            {
                let selection = InputSelection::new(2) | InputSelection::new(1);

                assert_eq!(selection.0.get(1).as_deref(), Some(&true));
                assert_eq!(selection.0.get(2).as_deref(), Some(&true));
                assert_eq!(selection.0.count_ones(), 2);
            }
        }
    }

    mod value_dep_info {
        use super::super::{GlobalInstant, InputSelection, ValueDepInfo};

        fn make_selection<I>(bits: I) -> InputSelection
        where
            I: IntoIterator<Item = usize>,
        {
            bits.into_iter().fold(InputSelection::empty(), |acc, i| {
                acc | InputSelection::new(i)
            })
        }

        fn make_value_dep_info<I>(since: GlobalInstant, bits: I) -> ValueDepInfo
        where
            I: IntoIterator<Item = usize>,
        {
            let input_selection = make_selection(bits);

            ValueDepInfo {
                since,
                input_selection,
            }
        }

        #[test]
        fn test_bitor_latest_instant() {
            {
                let dep1 = make_value_dep_info(GlobalInstant::new(3), []);
                let dep2 = make_value_dep_info(GlobalInstant::new(3), []);

                let r = dep1 | dep2;
                assert_eq!(r.since, GlobalInstant::new(3));
            }

            {
                let dep1 = make_value_dep_info(GlobalInstant::initial(), []);
                let dep2 = make_value_dep_info(GlobalInstant::new(1), []);

                let r = dep1 | dep2;
                assert_eq!(r.since, GlobalInstant::new(1));
            }

            {
                let dep1 = make_value_dep_info(GlobalInstant::new(256), []);
                let dep2 = make_value_dep_info(GlobalInstant::new(3), []);

                let r = dep1 | dep2;
                assert_eq!(r.since, GlobalInstant::new(256));
            }
        }

        #[test]
        fn test_bitor_join_selections() {
            let since = GlobalInstant::initial();

            {
                let dep1 = make_value_dep_info(since, [1]);
                let dep2 = make_value_dep_info(since, []);

                let r = dep1 | dep2;
                assert_eq!(r.input_selection, make_selection([1]));
            }

            {
                let dep1 = make_value_dep_info(since, [1]);
                let dep2 = make_value_dep_info(since, [2, 3]);

                let r = dep1 | dep2;
                assert_eq!(r.input_selection, make_selection([1, 2, 3]));
            }

            {
                let dep1 = make_value_dep_info(since, [1]);
                let dep2 = make_value_dep_info(since, [1]);

                let r = dep1 | dep2;
                assert_eq!(r.input_selection, make_selection([1]));
            }

            {
                let dep1 = make_value_dep_info(since, [1]);
                let dep2 = make_value_dep_info(since, [1, 2]);

                let r = dep1 | dep2;
                assert_eq!(r.input_selection, make_selection([1, 2]));
            }
        }
    }
}
