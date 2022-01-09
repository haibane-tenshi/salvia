//! Incremental computing brought to async Rust.
//!
//! **Obligatory warning**
//!
//! This crate is highly experimental.
//! Unless you absolutely need `async` consider using [`salsa`][salsa] instead.
//!
//! # Contents
//!
//! * [Primer](#primer)
//! * [Quick start](#quick-start)
//! * [Required trait bounds](#required-trait-bounds)
//! * [How it works](#how-it-works)
//! * [Features](#features)
//! * [Known limitations](#known-limitations)
//! * [Similar projects](#similar-projects)
//!
//! # Primer
//!
//! * [Async in Rust](https://rust-lang.github.io/async-book/)
//! * [Incremental computing](https://en.wikipedia.org/wiki/Incremental_computing)
//! * [Adapton][adapton] - research initiative for incremental computing
//! * [`salsa`][salsa] - incremental computation framework for sync Rust
//!
//! `salvia` allows you to define *queries* (functions which values will be cached) and *inputs*
//! ("functions" which value is set directly by user).
//! Upon execution, queries record which other queries or inputs it called and can avoid
//! recalculation when none of those values change.
//!
//! `salvia` was inspired by `salsa` library.
//! As a major difference from it, this crate was designed to work in async Rust from the start
//! and had to make some concessions to achieve that.
//!
//! Currently, only [`tokio`][tokio] runtime is supported.
//!
//! [tokio]: https://github.com/tokio-rs/tokio
//! [salsa]: https://github.com/salsa-rs/salsa
//! [adapton]: http://adapton.org/
//!
//! # Quick start
//!
//! 1.  *Define inputs.*
//!
//!     Input acts as *handle to storage* for values of specific type.
//!     Inputs can be explicitly set, so they serve as a primary entry point for user data.
//!
//!     Conceptually inputs are implemented as traits, so we will need a *host* type
//!     to attach input to.
//!
//!     ```
//!     #[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, salvia::InputAccess)]
//!     //                                                        ^^^^^^^^^^^
//!     // derive `InputAccess` to implement `get` and `set` functions on type
//!     enum Team {
//!         Blue,
//!         Red,
//!     }
//!
//!     #[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
//!     struct Score(u32);
//!
//!     // implement `Input` trait to associate type with input
//!     impl salvia::Input<Score> for Team {
//!     //                 ^^^^^
//!     // generic param indicates the type of stored value
//!
//!         fn initial(&self) -> Score {
//!         // ^^^^^^^
//!         // `initial` function will be called to initialize the storage if needed
//!
//!             Score(0)
//!         }
//!     }
//!     ```
//!
//!     You can implement `Input` multiple times using different `Value` types.
//!
//!     Deriving [`InputAccess`](macro@InputAccess) implements two (generic) functions
//!     `get` and `set` on `Host`.
//!     Unlike normal derive macros `InputAccess` does not implement a trait,
//!     it rather implements functions **directly on the type**.
//!     For reasons behind this design (and alternatives) see [`input`][node::input] module.
//!
//! 2.  *Define queries.*
//!
//!     Queries are *pure functions* which can call other queries or inputs.
//!     Do not forget to make them async!
//!
//!     ```
//!     # #[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, salvia::InputAccess)]
//!     # enum Team {
//!     #     Blue,
//!     #     Red,
//!     # }
//!     #
//!     # #[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
//!     # struct Score(u32);
//!     #
//!     # impl salvia::Input<Score> for Team {
//!     #     fn initial(&self) -> Score {
//!     #         Score(0)
//!     #     }
//!     # }
//!     #
//!     #[salvia::query]
//!     async fn leader(cx: &salvia::QueryContext) -> Option<Team> {
//!         //          ^^
//!         // query should take `&QueryContext` as last parameter...
//!         use std::cmp::Ordering;
//!
//!         let red: Score = Team::Red.get(cx).await;
//!         //                             ^^
//!         // ... and just pass it into any other queries it calls
//!
//!         let blue: Score = Team::Blue.get(cx).await;
//!         //                           ^^^
//!         // `get` is a query which allows you to access value of an input
//!         // it has signature of `async fn get<T>(self, &QueryContext) -> T`
//!         // you can use it similarly to `Iterator::collect()` by specifying expected type
//!
//!         match red.cmp(&blue) {
//!             Ordering::Less => Some(Team::Blue),
//!             Ordering::Equal => None,
//!             Ordering::Greater => Some(Team::Red),
//!         }
//!     }
//!     ```
//!
//!     The macro is not picky and supports transformation on all kinds of functions.
//!     Check [`#[query]`](`query`) attribute documentation to learn more
//!     about its usage.
//!
//! 3.  *Create runtime*
//!
//!     Runtime holds references to internal storage and mediates synchronization.
//!
//!     You can create one by calling to [`Runtime::new`] from inside a task:
//!
//!     ```
//!     #[tokio::main]
//!     async fn main() {
//!         let rt = salvia::Runtime::new().await;
//!     }
//!     ```
//!
//!     `new` is guaranteed to return `Poll::Ready`, `async` is just a reminder that you have
//!     to call from within tokio context,
//!     runtime needs access to it to set up internal tasks.
//!
//!     Alternatively, in synchronous context you can use [`Runtime::with_tokio`]:
//!
//!     ```
//!     # fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let tokio = tokio::runtime::Runtime::new()?;
//!     let rt = salvia::Runtime::with_tokio(&tokio);
//!     # Ok(())
//!     # }
//!     ```
//!
//! 4. *Run*
//!
//!     Now we can update inputs
//!
//!     ```
//!     # #[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, salvia::InputAccess)]
//!     # enum Team {
//!     #     Blue,
//!     #     Red,
//!     # }
//!     #
//!     # #[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
//!     # struct Score(u32);
//!     #
//!     # impl salvia::Input<Score> for Team {
//!     #     fn initial(&self) -> Score {
//!     #         Score(0)
//!     #     }
//!     # }
//!     #
//!     # #[salvia::query]
//!     # async fn leader(cx: &salvia::QueryContext) -> Option<Team> {
//!     #     use std::cmp::Ordering;
//!     #
//!     #     let red: Score = Team::Red.get(cx).await;
//!     #     let blue: Score = Team::Blue.get(cx).await;
//!     #
//!     #     match red.cmp(&blue) {
//!     #         Ordering::Less => Some(Team::Blue),
//!     #         Ordering::Equal => None,
//!     #         Ordering::Greater => Some(Team::Red),
//!     #     }
//!     # }
//!     #
//!     # #[tokio::main]
//!     # async fn main() {
//!     #     let rt = salvia::Runtime::new().await;
//!     #
//!     rt.mutate(|cx| async move {
//!         //     ^^
//!         // `Runtime::mutate` gives you access to `InputContext`
//!
//!         Team::Red.set(Score(1), &cx).await;
//!         //        ^^^
//!         // `set` allows you to mutate an input
//!         // it has signature of `async fn set<T>(self, value: T, cx: &InputContext)`
//!         //                                                           ^^^^^^^^^^^^
//!         // note that setters take `&InputContext`, a different context type from queries
//!     }).await;
//!     # }
//!     ```
//!
//!     or run queries
//!
//!     ```
//!     # #[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, salvia::InputAccess)]
//!     # enum Team {
//!     #     Blue,
//!     #     Red,
//!     # }
//!     #
//!     # #[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
//!     # struct Score(u32);
//!     #
//!     # impl salvia::Input<Score> for Team {
//!     #     fn initial(&self) -> Score {
//!     #         Score(0)
//!     #     }
//!     # }
//!     #
//!     # #[salvia::query]
//!     # async fn leader(cx: &salvia::QueryContext) -> Option<Team> {
//!     #     use std::cmp::Ordering;
//!     #
//!     #     let red: Score = Team::Red.get(cx).await;
//!     #     let blue: Score = Team::Blue.get(cx).await;
//!     #
//!     #     match red.cmp(&blue) {
//!     #         Ordering::Less => Some(Team::Blue),
//!     #         Ordering::Equal => None,
//!     #         Ordering::Greater => Some(Team::Red),
//!     #     }
//!     # }
//!     #
//!     # #[tokio::main]
//!     # async fn main() {
//!     #     let rt = salvia::Runtime::new().await;
//!     #
//!     #     rt.mutate(|cx| async move {
//!     #         Team::Red.set(Score(1), &cx).await;
//!     #     }).await;
//!     let leader = rt.query(|cx| async move {
//!         //                 ^^
//!         // `Runtime::query` gives you access to `QueryContext`
//!
//!         leader(&cx).await
//!         //     ^
//!         // both `query` and `mutate` provide context by value,
//!         // so you need to reference it
//!     }).await;
//!
//!     assert_eq!(leader, Some(Team::Red));
//!     # }
//!     ```
//!
//!     Note that both [`Runtime::query`] and [`Runtime::mutate`] provide appropriate context
//!     *by value*, which is inconsistent with normal queries.
//!     Unfortunately this is caused by a long-standing issue between closures, async and lifetimes:
//!     currently there is no way to pass a reference into async closure and properly constrain
//!     lifetimes so it can be consumed by other functions :(
//!
//! # Required trait bounds
//!
//! ## Core traits
//!
//! All involved value types are required to implement selection of traits:
//!
//! *   `Clone` - required to duplicate values to rerun queries and to clone cache contents
//! *   `Eq` - required to determine if queries are outdated and need to be rerun
//! *   `Send` - required to send between tasks (which potentially can send them between threads)
//! *   `Sync` - interestingly, `Sync` is not strictly required (in fact early prototypes didn't
//!     use it at all), however it helps to avoid excess cloning in multiple cases
//! *   `'static` - required to persist values and to send them between tasks
//!
//! `Clone + Eq + Send + Sync + 'static` can be quite a mouthful to type,
//! so `salvia` provides a special [`Stashable`] trait which can be used as a convenient shortcut.
//!
//! ## Additional traits
//!
//! Query parameters and input hosts are also required to implement `Hash`.
//!
//! `salvia`'s unit of operation is *bound query*.
//! *Bound query* is a combination of query with its parameter values.
//! To draw an analogy if normal query is like a function
//! then *bound query* is a closure which captures all parameters and forwards them to the function:
//!
//! ```
//! fn query(a: u32, b: f64, c: &str) {}
//!
//! let a = 2;
//! let b = 3.5;
//! let c = "beautiful tree";
//!
//! let bound_query = move || {
//!     // a, b, c is captured here
//!     query(a, b, c)
//! };
//! ```
//!
//! Why is this important?
//! *Bound queries* are handled independently from each other and have separate caches.
//! Which brings forth a question: how does framework finds its way back to a specific cache?
//! Queries are represented as different functions so we can statically distinguish between them,
//! but then we also need to do the same for parameter values.
//!
//! `salvia` opts for simple solution: `HashMap`.
//! The observation here is that types you can put into framework are "data-like",
//! so they almost never have a reason to not implement `Hash`.
//!
//! It is possible to avoid this extra bound at the cost of linear lookup,
//! but that hurts every other use case.
//!
//! # How it works
//!
//! On important implementation details and how it affects runtime characteristics see
//! [`runtime`][runtime] module-level documentation.
//!
//! # Features
//!
//! This crate provides following features:
//!
//! *   `async-trait` - enable [`InputAccess`](trait@InputAccess) trait,
//!     pulls in [`async-trait`][docs.rs:async_trait] crate.
//! *   `tracing` - enable internal logging via [`tracing`][docs.rs:tracing] crate.
//!     It gets quite verbose, intended for debugging of `salvia` itself.
//!     Log contents are not part of SemVer guarantees.
//!     See [CONTRIBUTING][repo:contributing] doc for more details on logging levels.
//!
//!     You **should not** use it in production or enable in your lib:
//!     it silently modifies definition of [`Stashable`] by adding [`Debug`] trait.
//! *   `nightly` - enable some unstable features.
//!     At the moment it just enables [`doc_cfg`][rustdoc:doc_cfg],
//!     so it is only useful when compiling documentation.
//!
//! [repo:contributing]: https://github.com/haibane-tenshi/salvia/blob/main/CONTRIBUTING.md#using-tracing
//! [docs.rs:async_trait]: https://docs.rs/async-trait/latest/async_trait/
//! [docs.rs:tracing]: https://docs.rs/tracing/latest/tracing/
//! [rustdoc:doc_cfg]: https://doc.rust-lang.org/rustdoc/unstable-features.html#doccfg-recording-what-platforms-or-features-are-required-for-code-to-be-present
//!
//! # Known limitations
//!
//! *   Only `tokio` runtime is supported.
//!
//!     But it should be possible to support other runtimes in the future as long as there are ways
//!     to abstract over them.
//!
//! *   No proper `Input` trait.
//!
//!     This is caused by lack of certain features in Rust type system, so it can be lifted in
//!     distant future.
//!     See [`input`][node::input] module documentation for details.
//!
//! *   No serialization/deserialization capabilities.
//!
//!     [`anymap2`] crate is used internally which makes it unclear to how even approach
//!     this topic.
//!
//!     See [DESIGN_DOC][repo:design_doc] for list of possible approaches.
//!
//! *   No node deallocation.
//!
//!     Currently nodes once started terminate only when runtime is dropped.
//!     That is a valid strategy, but can cause memory issues when too many queries with diverse
//!     sets of arguments are called.
//!
//!     It should possible to implement some kind of cache invalidation system, but within
//!     the current architecture this will require periodically waking nodes at masse.
//!     This is against the core idea of laziness, so it is not implemented at the moment.
//!
//! *   No *external* synchronization mechanisms.
//!
//!     `salvia` provides only two *internal* synchronization mechanisms:
//!
//!     *   `Runtime::query` guarantees that all queries executed within it agree on which input
//!         values they see.
//!     *   `Runtime::mutate` guarantees that passed closure is executed atomically w.r.t. to other
//!         calls to `mutate`.
//!
//!     However there is no synchronization means between individual calls
//!     to `query`/`mutate`.
//!
//!     You can sequence them by just polling one to completion before other:
//!
//!     ```
//!     # #[tokio::main]
//!     # async fn main() {
//!     # let rt = salvia::Runtime::new().await;
//!     rt.mutate(|cx| async move {
//!         // change inputs...
//!     }).await; // this await makes sure that mutation went through
//!
//!     rt.query(|cx| async move {
//!         // query values...
//!     }).await; // this await makes sure that query has finished
//!     # }
//!     ```
//!
//!     However this does not guarantee that query will see exact same values that you just set:
//!     indeed it is possible for some other task to change an input value after `mutate`
//!     finishes but before `query` starts!
//!     Similarly, if two tasks try to write to the same input concurrently there are no guarantees
//!     about if and which value will be observed by queries.
//!     You can be sure that one value is going to persist (the one that arrived last),
//!     but it can be either of the two.
//!
//!     If you are in need for more sophisticated dataflow control you should lean into normal
//!     async synchronization methods (manager tasks, semaphores, etc).
//!
//! [repo:design_doc]: https://github.com/haibane-tenshi/salvia/blob/main/DESIGN_DOC.md#deserialization
//!
//! # Similar projects
//!
//! * [`salsa`][docs.rs:salsa]
//!
//!     Similarities:
//!     * Both are based on pure functions
//!     * Both use query/input structure
//!
//!     Key differences:
//!     * `salsa` is sync, `salvia` is async
//!     *   `salsa`'s queries are resolved statically via traits,
//!         i.e. given a runtime you can only run queries implemented on it.
//!
//!         `salvia`'s queries are resolved dynamically, i.e. you can run any query in any runtime.
//!
//! * [`futures_signals`][docs.rs:futures_signals]
//!
//!     Similarities:
//!     * Both deal with async
//!
//!     Key differences:
//!     *   `futures_signals` is a "push-based" framework, i.e.
//!         * computations are eager (introducing change causes computations),
//!         * computations are effectfull (operate with side-effects).
//!
//!         `salvia` is "pull-based" framework, i.e.
//!         *   computations are lazy (introducing change nas no extra consequences;
//!             performed only on request),
//!         * computations are pure (don't produce side-effects).
//!
//!     *   `salvia` cares about result consistency: putting same inputs into computation
//!         necessarily produces the same outcome.
//!
//!
//! [docs.rs:salsa]: https://docs.rs/salsa/latest/salsa/
//! [docs.rs:futures_signals]: https://docs.rs/futures-signals/latest/futures_signals/
#![warn(missing_docs)]
#![allow(dead_code)]
#![cfg_attr(feature = "nightly", feature(doc_cfg))]

#[allow(unused_imports)]
use std::fmt::Debug;

pub use node::input::{Input, InputAccess};
pub use node::query::query;
pub use runtime::{InputContext, QueryContext, Runtime};

macro_rules! info {
    ($($t:tt)*) => {
        #[cfg(feature = "tracing")]
        ::tracing::info!($($t)*)
    }
}

macro_rules! debug {
    ($($t:tt)*) => {
        #[cfg(feature = "tracing")]
        ::tracing::debug!($($t)*)
    }
}

macro_rules! debug_span {
    ($($t:tt)*) => {{
        #[cfg(feature = "tracing")]
        ::tracing::debug_span!($($t)*)
    }}
}

pub mod node;
pub mod runtime;

pub(crate) mod time;
pub(crate) mod tracing;

#[doc(hidden)]
pub mod macro_support;

/// A shortcut for `Clone + Eq + Send + Sync + 'static`.
///
/// This trait is an umbrella for all trait bounds required for value types
/// flowing in or out of the framework.
/// See [crate level](self#core-traits) documentation for more details.
///
/// The purpose of this trait is to reduce boilerplate and chances of accidentally forgetting some
/// in generic context.
/// It is recommended to use the trait over specifying every bound individually.
#[cfg(not(feature = "tracing"))]
pub trait Stashable: Clone + Eq + Send + Sync + 'static {}

/// A shortcut for `Debug + Clone + Eq + Send + Sync + 'static`.
///
/// This trait is an umbrella for all trait bounds required for value types
/// flowing in or out of the framework.
/// See [crate level](self#core-traits) documentation for more details.
///
/// The purpose of this trait is to reduce boilerplate and chances of accidentally forgetting some
/// in generic context.
/// It is recommended to use the trait over specifying every bound individually.
///
/// **Warning**: this variant of trait is defined under `tracing` feature and is different
/// from normal definition (+ `Debug`).
#[cfg(feature = "tracing")]
pub trait Stashable: Debug + Clone + Eq + Send + Sync + 'static {}

#[cfg(not(feature = "tracing"))]
impl<T> Stashable for T where T: Clone + Eq + Send + Sync + 'static {}

#[cfg(feature = "tracing")]
impl<T> Stashable for T where T: Debug + Clone + Eq + Send + Sync + 'static {}

// Test examples from README.
#[doc = include_str!("../README.md")]
#[doc(hidden)]
const _: () = ();
