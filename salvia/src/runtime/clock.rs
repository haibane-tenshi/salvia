//! Implementation of internal synchronization tasks.

use std::fmt::Debug;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore};

use crate::node::LocalInstantRequest;
use crate::time::{GlobalInstant, InputSnapshot, LocalInstant};

/// Token proving that clock is in *mutation* phase.
#[derive(Clone)]
pub struct InputToken {
    current_frame: GlobalInstant,
    guard: Arc<OwnedSemaphorePermit>,
}

impl InputToken {
    /// Acquire time token associated with next *query* phase.
    pub fn current_frame(&self) -> GlobalInstant {
        self.current_frame
    }
}

impl Debug for InputToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(InputToken))
            .field("current_frame", &self.current_frame)
            .finish_non_exhaustive()
    }
}

/// Token proving that clock is in *query* phase.
#[derive(Clone)]
pub struct QueryToken {
    current_frame: GlobalInstant,
    input_snapshot: InputSnapshot,
    guard: Arc<OwnedSemaphorePermit>,
}

impl QueryToken {
    /// Special constructor for testing purpose.
    ///
    /// Query internals are strongly tied to token and it is impossible to test them without it.
    #[cfg(any(test, doc))]
    #[doc(hidden)]
    pub fn new(
        current_frame: GlobalInstant,
        input_snapshot: InputSnapshot,
        guard: Arc<OwnedSemaphorePermit>,
    ) -> Self {
        QueryToken {
            current_frame,
            input_snapshot,
            guard,
        }
    }

    /// Acquire time token associated with current *query* phase.
    pub fn current_frame(&self) -> GlobalInstant {
        self.current_frame
    }

    /// Acquire time tokens associated with inputs within current phase.
    pub fn input_snapshot(&self) -> &InputSnapshot {
        &self.input_snapshot
    }
}

impl Debug for QueryToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(QueryToken))
            .field("current_frame", &self.current_frame)
            .field("input_snapshot", &self.input_snapshot)
            .finish_non_exhaustive()
    }
}

/// Register a bound input inside clock.
///
/// Upon registration the input will be given a unique index which identifies its respective bit
/// in `InputSelection` (see [`node`](crate::node) module).
///
/// At the beginning of every frame clock requests current `LocalInstant`
/// through registered `handle`.
/// This time token then gets embedded into [`QueryToken`] to allow queries to check if their
/// input dependencies has changed as per [first step](crate::runtime#update-routine)
/// of update routine.
#[derive(Debug)]
pub(super) struct RegisterInputRequest {
    /// Channel to request `LocalTime` token from input.
    pub handle: InputTimeHandle,

    /// Channel to return assigned index to input.
    pub callback: oneshot::Sender<usize>,
}

/// Request all newly registered inputs since the last time.
struct AcquireInputsRequest {
    pub callback: oneshot::Sender<AcquireInputsResponse>,
}

/// Respond an ordered vec or handles to registered inputs.
struct AcquireInputsResponse {
    pub handles: Vec<mpsc::Sender<LocalInstantRequest>>,
}

/// Handle to input node for requesting local instants.
type InputTimeHandle = mpsc::Sender<LocalInstantRequest>;

/// Handle to clock for sending phase instructions.
type InstructionsHandle = mpsc::UnboundedSender<ClockInstr>;

/// Receiver of clock phase instructions.
type InstructionsReceiver = mpsc::UnboundedReceiver<ClockInstr>;

/// Collection of channels to access clock.
pub(super) struct ClockHandle {
    /// Send phase change instructions.
    pub instructions: InstructionsHandle,

    /// Send input registration requests.
    pub register_input: mpsc::UnboundedSender<RegisterInputRequest>,
}

/// Handle to indexer for asking newly registered inputs.
type AcquireInputsHandle = mpsc::UnboundedSender<AcquireInputsRequest>;

/// Collections of channels to access indexer.
struct IndexerHandle {
    pub register_handle: mpsc::UnboundedSender<RegisterInputRequest>,
    pub acquire_inputs: AcquireInputsHandle,
}

/// Receiver for indexer channels.
struct IndexerReceiver {
    pub register_handle: mpsc::UnboundedReceiver<RegisterInputRequest>,
    pub acquire_inputs: mpsc::UnboundedReceiver<AcquireInputsRequest>,
}

/// Create indexer channels.
fn indexer_channels() -> (IndexerHandle, IndexerReceiver) {
    let (register_in, register_out) = mpsc::unbounded_channel();
    let (acquire_in, acquire_out) = mpsc::unbounded_channel();

    let handle = IndexerHandle {
        register_handle: register_in,
        acquire_inputs: acquire_in,
    };

    let receiver = IndexerReceiver {
        register_handle: register_out,
        acquire_inputs: acquire_out,
    };

    (handle, receiver)
}

/// Clock phase change command.
///
/// Token proving that clock entered requested phase will be returned through supplied channel.
pub enum ClockInstr {
    /// Request transition to next *query* phase.
    StartQuery(oneshot::Sender<QueryToken>),

    /// Request transition to next *mutation* phase.
    StartMutation(oneshot::Sender<InputToken>),
}

impl Debug for ClockInstr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use ClockInstr::*;

        struct Blank;

        impl Debug for Blank {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "_")
            }
        }

        match self {
            StartQuery(_) => f.debug_tuple(stringify!(StartQuery)).field(&Blank).finish(),
            StartMutation(_) => f
                .debug_tuple(stringify!(StartMutation))
                .field(&Blank)
                .finish(),
        }
    }
}

/// Phase of the clock/runtime.
///
/// See [`runtime`](crate::runtime#runtime-phases) module for more details.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
enum ClockPhase {
    Mutation,
    Query(InputSnapshot),
}

impl Default for ClockPhase {
    fn default() -> Self {
        let snapshot = Arc::from(Vec::new());

        ClockPhase::Query(snapshot)
    }
}

/// Internal state of clock task.
struct ClockState {
    phase: ClockPhase,
    current: GlobalInstant,
    input_handles: Vec<mpsc::Sender<LocalInstantRequest>>,
    acquire_handles: AcquireInputsHandle,
    guard: ClockGuard,
}

impl ClockState {
    /// Create new state.
    fn new(acquire_handles: AcquireInputsHandle) -> Self {
        ClockState {
            phase: Default::default(),
            current: Default::default(),
            input_handles: Default::default(),
            acquire_handles,
            guard: ClockGuard::new(),
        }
    }

    async fn acquire_new_handles(&mut self) -> Vec<InputTimeHandle> {
        let (callback, receiver) = oneshot::channel();

        let request = AcquireInputsRequest { callback };

        self.acquire_handles
            .send(request)
            .ok()
            .expect("indexer task should outlive clock task");

        receiver
            .await
            .expect("indexer task should outlive clock task")
            .handles
    }

    /// Pull newly registered input nodes from indexer task.
    async fn update_handles(&mut self) {
        let new_handles = self.acquire_new_handles().await;
        self.input_handles.extend(new_handles);
    }

    /// Create snapshot of inputs' local instants.
    ///
    /// This will cause inputs to actualize their values and progress local time if necessary.
    async fn create_snapshot(&self) -> InputSnapshot {
        use futures::stream::{FuturesOrdered, StreamExt};

        let t: FuturesOrdered<_> = self
            .input_handles
            .iter()
            .map(|handle| async move {
                let fut = async move {
                    let (request, time_out) = {
                        let (time_in, time_out) = oneshot::channel();

                        let request = LocalInstantRequest { callback: time_in };

                        (request, time_out)
                    };
                    handle.send(request).await.ok()?;

                    time_out.await.ok()
                };

                // This will return None in case input died somehow.
                // This shouldn't happen under normal circumstances.
                // We can substitute it with initial instant if that happens.
                // This is completely harmless: that input cannot be queried directly anymore.
                // Even if it is reinstantiated later, it will reside in a different task and with
                // different index.
                fut.await.unwrap_or_else(LocalInstant::initial)
            })
            .collect();

        let v: Vec<_> = t.collect().await;

        Arc::from(v)
    }

    /// Force clock to progress into *query* phase and provide token as proof.
    async fn force_query(&mut self) -> QueryToken {
        use crate::tracing;

        let input_snapshot = match &self.phase {
            ClockPhase::Query(input_snapshot) => input_snapshot,
            ClockPhase::Mutation => {
                self.guard.release_all().await;

                tracing::record_in_span("state", &self);

                self.update_handles().await;
                self.phase = ClockPhase::Query(self.create_snapshot().await);

                info!(current_frame = ?self.current, "start a query phase");

                if let ClockPhase::Query(input_snapshot) = &self.phase {
                    input_snapshot
                } else {
                    unreachable!()
                }
            }
        };

        QueryToken {
            current_frame: self.current,
            input_snapshot: input_snapshot.clone(),
            guard: Arc::new(self.guard.acquire()),
        }
    }

    /// Force clock to progress into *mutation* phase and provide token as proof.
    async fn force_mutation(&mut self) -> InputToken {
        use crate::tracing;

        match &self.phase {
            ClockPhase::Mutation => {
                // We need to wait for all previous tokens to expire to ensure atomicity of writes.
                self.guard.release_all().await;

                tracing::record_in_span("state", &self);
            }
            ClockPhase::Query(_) => {
                self.guard.release_all().await;

                self.update_handles().await;
                self.phase = ClockPhase::Mutation;
                self.current = self.current.next();

                tracing::record_in_span("state", &self);
                info!(current_frame = ?self.current, "start a mutation phase");
            }
        };

        InputToken {
            current_frame: self.current,
            guard: Arc::new(self.guard.acquire()),
        }
    }
}

impl Debug for ClockState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        struct Wrapper(usize);

        impl Debug for Wrapper {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "[..; {}]", self.0)
            }
        }

        f.debug_struct(stringify!(ClockState))
            .field("phase", &self.phase)
            .field("current_frame", &self.current)
            .field("input_handles", &Wrapper(self.input_handles.len()))
            .field("guard", &self.guard)
            .finish()
    }
}

/// Helper type for tracking existence of generated tokens.
struct ClockGuard {
    total_count: u32,
    semaphore: Arc<Semaphore>,
}

impl ClockGuard {
    /// Create a new guard.
    fn new() -> Self {
        let total_count = 10;

        Self {
            total_count,
            semaphore: Arc::new(Semaphore::new(total_count as usize)),
        }
    }

    /// Acquire one permit from internal semaphore.
    ///
    /// This function will add more permits to semaphore if necessary so
    /// it is synchronous and always succeeds.
    fn acquire(&mut self) -> OwnedSemaphorePermit {
        if self.semaphore.available_permits() == 0 {
            let new_permits = 10;

            self.total_count += new_permits;
            self.semaphore.add_permits(new_permits as usize);
        }

        self.semaphore.clone().try_acquire_owned().unwrap()
    }

    /// Await until all permits are released.
    async fn release_all(&self) {
        let _ = self.semaphore.acquire_many(self.total_count).await.unwrap();
    }
}

impl Debug for ClockGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(ClockGuard))
            .field(
                "active_permits",
                &(self.total_count - self.semaphore.available_permits() as u32),
            )
            .field("total_permits", &self.total_count)
            .finish()
    }
}

/// Clock task.
///
/// Clock manages transitions between query and mutation phases.
/// It is also responsible for generation of tokens - proofs of enacting specific phase.
/// Tokens are required for communication between nodes and also embedded in corresponding contexts
/// to provide entry points for user code.
async fn clock(mut state: ClockState, mut instructions: InstructionsReceiver) {
    use crate::tracing::{self, Instrument};

    debug!(?state, "spawn clock");

    while let Some(instr) = instructions.recv().await {
        let span = debug_span!("request: instruction", ?instr, ?state);

        let state = &mut state;

        async move {
            match instr {
                ClockInstr::StartQuery(callback) => {
                    let token = state.force_query().await;

                    let _ = callback.send(token);
                }
                ClockInstr::StartMutation(callback) => {
                    let token = state.force_mutation().await;

                    let _ = callback.send(token);
                }
            }

            tracing::record_in_span("state", state);
            debug!("processed instruction");
        }
        .instrument(span)
        .await
    }
}

/// Indexer task.
///
/// Indexer manages newly created inputs on behalf of clock task:
/// it is responsible for assigning a unique index into `InputSelection` and `InputSnapshot` vectors
/// as well as forwarding input handles to clock task.
async fn indexer(receiver: IndexerReceiver) {
    use tokio::select;

    let IndexerReceiver {
        mut register_handle,
        mut acquire_inputs,
    } = receiver;

    let mut queue = Vec::new();
    let mut index = 0;

    loop {
        select! {
            biased;

            Some(request) = register_handle.recv() => {
                let RegisterInputRequest {
                    handle,
                    callback,
                } = request;

                queue.push(handle);
                let i = index;
                index += 1;

                let _ = callback.send(i);
                debug!(index = ?i, "serve input registration");
            }
            Some(request) = acquire_inputs.recv() => {
                let AcquireInputsRequest {
                    callback,
                } = request;

                let response = AcquireInputsResponse {
                    handles: std::mem::take(&mut queue),
                };

                let _ = callback.send(response);
            }
            else => break,
        }
    }
}

/// Spawn new clock task.
pub(super) fn spawn() -> ClockHandle {
    let (indexer_handle, indexer_receiver) = indexer_channels();

    tokio::spawn(indexer(indexer_receiver));

    let IndexerHandle {
        register_handle,
        acquire_inputs,
    } = indexer_handle;

    let state = ClockState::new(acquire_inputs);
    let (instructions_in, instructions_out) = mpsc::unbounded_channel();

    tokio::spawn(clock(state, instructions_out));

    ClockHandle {
        instructions: instructions_in,
        register_input: register_handle,
    }
}

#[cfg(test)]
mod test {
    use super::{indexer, indexer_channels, ClockPhase, ClockState, GlobalInstant};
    use std::sync::Arc;

    #[tokio::test]
    async fn test_force_mutation() {
        let (indexer_in, indexer_out) = indexer_channels();
        tokio::spawn(indexer(indexer_out));

        let mut state = ClockState {
            phase: ClockPhase::Query(Arc::from([])),
            current: GlobalInstant::new(3),
            ..ClockState::new(indexer_in.acquire_inputs)
        };

        state.force_mutation().await;

        assert_eq!(state.phase, ClockPhase::Mutation);
        assert_eq!(state.current, GlobalInstant::new(4));

        state.force_mutation().await;

        assert_eq!(state.phase, ClockPhase::Mutation);
        assert_eq!(state.current, GlobalInstant::new(4));
    }

    #[tokio::test]
    async fn test_force_query() {
        let (indexer_in, indexer_out) = indexer_channels();
        tokio::spawn(indexer(indexer_out));

        let mut state = ClockState {
            phase: ClockPhase::Mutation,
            current: GlobalInstant::new(5),
            ..ClockState::new(indexer_in.acquire_inputs)
        };

        state.force_query().await;

        assert_eq!(state.phase, ClockPhase::Query(Arc::from([])));
        assert_eq!(state.current, GlobalInstant::new(5));

        state.force_query().await;

        assert_eq!(state.phase, ClockPhase::Query(Arc::from([])));
        assert_eq!(state.current, GlobalInstant::new(5));
    }
}
