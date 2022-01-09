//! Types to track and version changes in inputs.
//!
//! This module contains types used to track changes in inputs both globally
//! (i. e. without respecting which input changed, represented by [`GlobalInstant`]) and locally
//! (i. e. specific for every input, represented by [`LocalInstant`]).
//!
//! `GlobalTime` also has a companion `Duration` type which is used to implement simple
//! arithmetic.

use std::ops::{Add, AddAssign, Sub, SubAssign};
use std::sync::Arc;

/// Time token for tracking temporal changes in individual input.
///
/// An input is required to track changes in its stored value by assigning a new instant
/// whenever its observed value changes.
/// Every token is required to be unique for at least `LocalTime::MAX_GLOBAL_FRAMES` frames.
/// This setup powers [first step](crate::runtime#update-routine) in query update routine.
///
/// The type is implemented as a newtype around `u8`, so it can potentially wrap given enough time
/// (which is indicated by `MAX_GLOBAL_FRAMES` constant).
/// Use it in combination with [`GlobalInstant`] to avoid this issue.
///
/// This choice of representation is done in order to save memory and speed up comparisons.
/// The point of local versioning is to avoid update cascade
/// ([step two](crate::runtime#update-routine) in update routine)
/// on frequent queries - in case their inputs are untouched.
/// But it doesn't necessarily need to work on infrequent queries (when local time can wrap
/// between polls), after all the longer cache persists the more likely it is to be out of date.
/// In that case just paying the price of cascade is reasonable enough.
#[derive(Debug, Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash)]
pub struct LocalInstant(u8);

impl LocalInstant {
    /// Maximal number of frames for which given `LocalTime` is unique.
    pub const MAX_FRAMES: u64 = 2u64.pow(8);

    /// Maximal number of frames for which given `LocalTime` is unique expressed as `Duration`.
    pub const MAX_GLOBAL_FRAMES: Duration = Duration::from_frames(Self::MAX_FRAMES);

    /// Generate initial instant.
    pub fn initial() -> Self {
        LocalInstant(0)
    }

    /// Create a new instant.
    pub fn new(n: u8) -> Self {
        LocalInstant(n)
    }

    /// Generate next token.
    pub fn next(self) -> Self {
        LocalInstant(self.0.wrapping_add(1))
    }
}

/// Snapshot of inputs' local time.
pub(crate) type InputSnapshot = Arc<[LocalInstant]>;

/// Time token for tracking temporal changes across all inputs.
///
/// Runtime generates a new unique instant per frame.
/// This is done in order for outdated queries to become aware that they are behind.
///
/// The type is implemented as newtype around `u64`.
/// It is not expected to wrap in any practical circumstance, however it will panic if that happens.
#[derive(Debug, Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Default)]
pub struct GlobalInstant(u64);

impl GlobalInstant {
    /// Generate initial instant.
    pub fn initial() -> GlobalInstant {
        GlobalInstant(0)
    }

    /// Create a new instant.
    pub fn new(n: u64) -> GlobalInstant {
        GlobalInstant(n)
    }

    /// Generate next token.
    pub fn next(self) -> GlobalInstant {
        GlobalInstant(self.0.checked_add(1).unwrap())
    }
}

impl Add<Duration> for GlobalInstant {
    type Output = GlobalInstant;

    fn add(mut self, rhs: Duration) -> Self::Output {
        self += rhs;
        self
    }
}

impl AddAssign<Duration> for GlobalInstant {
    fn add_assign(&mut self, rhs: Duration) {
        self.0 = self
            .0
            .checked_add(rhs.0)
            .expect("time arithmetics should not overflow")
    }
}

impl Sub<Duration> for GlobalInstant {
    type Output = GlobalInstant;

    fn sub(mut self, rhs: Duration) -> Self::Output {
        self -= rhs;
        self
    }
}

impl SubAssign<Duration> for GlobalInstant {
    fn sub_assign(&mut self, rhs: Duration) {
        self.0 = self
            .0
            .checked_sub(rhs.0)
            .expect("time arithmetics should not underflow")
    }
}

impl Sub for GlobalInstant {
    type Output = Duration;

    fn sub(self, rhs: Self) -> Self::Output {
        let value = self
            .0
            .checked_sub(rhs.0)
            .expect("time arithmetics should not underflow");

        Duration(value)
    }
}

/// Represent a positive distance between two `GlobalFrame`s.
#[derive(Debug, Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash)]
pub struct Duration(u64);

impl Duration {
    /// Construct a new duration.
    pub const fn from_frames(n: u64) -> Duration {
        Duration(n)
    }
}
