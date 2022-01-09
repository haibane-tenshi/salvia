//! Stubs for `tracing` crate functionality.
//!
//! Items in this module serve as proxies to `tracing` crate functionality
//! (as well as `info!`, `debug!` and `debug_span!` macros from crate root).
//! When `tracing` feature is not enabled they turn into no-op.

#[cfg(feature = "tracing")]
pub use tracing::Instrument;

#[cfg(not(feature = "tracing"))]
pub trait Instrument: Sized {
    fn instrument(self, (): ()) -> Self {
        self
    }
}

#[cfg(not(feature = "tracing"))]
impl<T> Instrument for T where T: std::future::Future {}

#[cfg(feature = "tracing")]
pub fn record_in_span<V>(field: &str, value: &V)
where
    V: std::fmt::Debug,
{
    tracing::span::Span::current().record(field, &tracing::field::debug(value));
}

#[cfg(not(feature = "tracing"))]
pub fn record_in_span<V>(_field: &str, _value: &V) {}
