//! Macro implementations for `salvia` framework.
//!
//! Don't use this crate directly, use `salvia` crate instead.

use proc_macro::TokenStream;

mod input;
mod query;

/// Turn `async fn` into `salvia`'s query.
///
/// Queries cache their value when executed and able to avoid recalculation when queries/inputs
/// it depends on don't change.
///
/// # Contents
///
/// * [How it works](#how-it-works)
/// * [Basic usage](#basic-usage)
/// * [Advanced usage](#advanced-usage)
///     * [`&self`](#self)
///     * [`&'static self`](#static-self)
///     * [Recursion](#recursion)
///     * [Traits](#traits)
/// * [Unsupported features](#not-supported)
/// * [Configuration](#configuration)
///
/// # How it works
///
/// Macro hoists provided function and replaces it with query *executor*.
///
/// *Executor* is responsible for locating/spawning correct *node* (which is a looped task serving
/// as cache storage) and communicating to it.
/// It also registers discovered node as dependency to parent query.
///
/// Attribute will expand the following
///
/// ```
/// # use salvia::{QueryContext, query};
/// #[query]
/// async fn my_query(n: i32, cx: &QueryContext) -> bool {
///     // do some work
/// #     unimplemented!()
/// }
/// ```
///
/// roughly into this:
///
/// ```ignore
/// # use salvia::{QueryContext, query};
/// async fn my_query(n: i32, cx: &QueryContext) -> bool {
///     async fn inner(n: i32, cx: &QueryContext) -> bool {
///         // do some work
/// #     unimplemented!()
///     };
///
///     // conjure correct executor by magic
///     query_executor(inner, n, cx).await
/// }
/// ```
///
/// # Basic usage
///
/// `#[query]` supports nearly every function feature you may desire.
///
/// *   In its most basic form attribute can be applied to a **freestanding** async function:
///
///     ```no_run
///     # use std::sync::Arc;
///     # use salvia::{QueryContext, query};
///     #[query]
///     async fn freestanding(n: i32, strings: Arc<Vec<&'static str>>, cx: &QueryContext) -> bool {
///         unimplemented!()
///     }
///     ```
///
///     It can accept any number of arguments, but the last one should always be `&QueryContext`.
///
/// *   Function can be **generic** too:
///
///     ```no_run
///     # use std::hash::Hash;
///     # use salvia::{QueryContext, query, Stashable};
///     #[query]
///     async fn generic<T, U>(t: T, cx: &QueryContext) -> U
///         where T: Stashable + Hash, U: Stashable
///     {
///         unimplemented!()
///     }
///     ```
///
///     You will need to specify trait bounds, especially `Stashable`.
///
/// *   You can use it **in function scope**:
///
///     ```no_run
///     # use salvia::{QueryContext, query};
///     async fn outer(n: i32, cx: &QueryContext) -> bool {
///         #[query]
///         async fn inner(n: i32, cx: &QueryContext) -> bool {
///             unimplemented!()
///         }
///
///         inner(n, cx).await
///     }
///     ```
///
///     This specific arrangement is also referred to as "inner function trick" in this documentation.
///
/// *   Query can be in **associated function** position:
///
///     ```no_run
///     # use salvia::{QueryContext, query};
///     # struct Widget;
///     #
///     impl Widget {
///         #[query]
///         async fn associated(n: i32, cx: &QueryContext) -> bool {
///             unimplemented!()
///         }
///     }
///     ```
///
/// *   Also it can appear as **associated function in generic impl** block:
///
///     ```no_run
///     # use std::marker::PhantomData;
///     # use std::hash::Hash;
///     # use salvia::{QueryContext, query, Stashable};
///     # struct GenericWidget<T, U>(PhantomData<T>, PhantomData<U>);
///     #
///     impl<T, U> GenericWidget<T, U>
///         where T: Stashable + Hash, U: Stashable
///     {
///         #[query]
///         async fn generic_associated(t: T, cx: &QueryContext) -> U {
///             unimplemented!()
///         }
///     }
///     ```
///
/// *   Before you ask, yes, **`self` argument** is supported:
///
///     ```no_run
///     # use salvia::{QueryContext, query};
///     # #[derive(Clone, Eq, PartialEq, Hash)]
///     # struct Widget;
///     #
///     impl Widget {
///         #[query]
///         async fn take_and_calculate(self, n: i32, cx: &QueryContext) -> bool {
///             unimplemented!()
///         }
///     }
///     ```
///
///     Note that you *have* to pass self by value: it is treated just like any other argument
///     and must satisfy `Stashable`!
///     This in particular implies `'static`, so trying to pass in a reference will fail to compile.
///
///     See [advanced usage](#self) techniques for ways to conjure desirable API.
///
/// # Advanced usage
///
/// Recipes which work but require some boilerplate or external dependencies.
///
/// ## `&self`
///
/// Sadly there is no way to directly support references in queries:
///
/// ```compile_fail
/// # use salvia::{QueryContext, query};
/// # #[derive(Clone, Eq, PartialEq, Hash)]
/// # struct Widget;
/// #
/// impl Widget {
///     #[query] // references are not allowed :(
///     async fn calculate(&self, n: i32, cx: &QueryContext) -> bool {
///         unimplemented!()
///     }
/// }
/// ```
///
/// This is especially needed for non-`Copy` types where `self`-by-value can lead
/// to cumbersome API.
/// However it is trivial to produce desired functions by delegating to consuming variant:
///
/// ```no_run
/// # use salvia::{QueryContext, query};
/// # #[derive(Clone, Eq, PartialEq, Hash)]
/// # struct Widget;
/// #
/// impl Widget {
/// #     #[query]
/// #     async fn take_and_calculate(self, n: i32, cx: &QueryContext) -> bool {
/// #         unimplemented!()
/// #     }
/// #
///     async fn calculate(&self, n: i32, cx: &QueryContext) -> bool {
///         self.clone().take_and_calculate(n, cx).await
///     }
/// }
/// ```
///
/// Or in case you want to hide consuming variant inner function trick works too:
///
/// ```no_run
/// # use salvia::{QueryContext, query};
/// # #[derive(Clone, Eq, PartialEq, Hash)]
/// # struct Widget;
/// #
/// impl Widget {
///     async fn calculate(&self, n: i32, cx: &QueryContext) -> bool {
///         #[query]
///         async fn inner(this: Widget, n: i32, cx: &QueryContext) -> bool {
///             unimplemented!()
///         }
///
///         inner(self.clone(), n, cx).await
///     }
/// }
/// ```
///
/// ## `&'static self`
///
/// One case where accepting `self` by reference is valid is when that reference is alive
/// for `'static`:
///
/// ```no_run
/// # use salvia::{QueryContext, query};
/// # #[derive(Clone, Eq, PartialEq, Hash)]
/// # struct Widget;
/// impl Widget {
///     #[query]
///     async fn calculate(&'static self, cx: &QueryContext) -> bool {
///         unimplemented!()
///     }
/// }
/// ```
///
/// ## Recursion
///
/// Recursion is possible, but you will need `async_recursion` crate:
///
/// ```
/// # use tokio::join;
/// # use async_recursion::async_recursion;
/// # use salvia::{QueryContext, query, Runtime};
/// // notice the order of attributes
/// #[query]           // <- comes first
/// #[async_recursion] // <- comes last
/// async fn fibonacci(n: u64, cx: &QueryContext) -> u64 {
///     match n {
///         0 => 0,
///         1 => 1,
///         n => {
///             let (p1, p2) = join!(fibonacci(n - 1, cx), fibonacci(n - 2, cx));
///
///             p1 + p2
///         }
///     }
/// }
///
/// # #[tokio::main]
/// # async fn main() {
/// # let rt = Runtime::new().await;
/// #
/// # rt.query(|cx| async move {
/// assert_eq!(fibonacci(10, &cx).await, 55);
/// # }).await;
/// # }
/// ```
///
/// *Theoretically* it shouldn't be needed: the executor part communicates to node through a channel
/// so even though node calls executor (and must embed its future into itself) there is no
/// dependency the other way - no cycle.
/// However `rustc` refuses to recognize it, so here we are.
///
/// This works because each node corresponds to query + set of arguments, so multiple calls to the
/// same query but with different arguments correspond to different nodes which run concurrently.
/// From perspective of a node other nodes are completely opaque: all it really cares about is
/// whether their values are valid for its current calculation.
///
/// Beware that infinite recursion will deadlock as node ends up awaiting on itself.
///
/// ## Traits
///
/// Async in traits is hard, but it can be done with the help of `async_trait` crate.
///
/// ### Defaulted functions
///
/// Unfortunately naive attempt to use the two together doesn't work:
///
/// ```compile_fail
/// # use salvia::{QueryContext, query};
/// # use async_trait::async_trait;
/// #[async_trait]
/// trait Rendered {
///     #[query] // :(
///     async fn my_query(self, n: i32, cx: &QueryContext) -> bool {
///         unimplemented!()
///     }
/// }
/// ```
///
/// The reason behind it is order in which macros are expanded:
/// `#[async_trait]` starts first and transforms `my_query` into normal `fn` which returns boxed
/// future, but that is unsupported by `#[query]` for good reasons.
///
/// The way to work around the issue is to use inner function trick:
///
/// ```no_run
/// # use salvia::{QueryContext, query};
/// # use async_trait::async_trait;
/// #[async_trait]
/// trait Rendered {
///     async fn my_query(n: i32, cx: &QueryContext) -> bool {
///         #[query]
///         async fn inner(n: i32, cx: &QueryContext) -> bool {
///             unimplemented!()
///         }
///
///         inner(n, cx).await
///     }
/// }
/// ```
///
/// This gives a different target for each macro to expand on and lets them work together.
///
/// It gets a little bit more complicated if you want to operate on `self`:
///
/// *   You will need `Stashable + Hash` as supertraits.
/// *   Inner query needs to accept `Self` type as generic parameter because it cannot implicitly
///     capture it.
/// *   See also note on dyn trait/object safety in `async_trait` crate.
///
/// ```no_run
/// # use std::hash::Hash;
/// # use salvia::{QueryContext, Stashable, query};
/// # use async_trait::async_trait;
/// #[async_trait]
/// trait Rendered: Stashable + Hash {
///     //          ^^^^^^^^^   ^^^^
///     async fn my_query(self, n: i32, cx: &QueryContext) -> bool {
///         #[query]
///         async fn inner<SelfT>(this: SelfT, n: i32, cx: &QueryContext) -> bool
///         //             ^^^^^
///             where SelfT: Stashable + Hash
///         {
///             unimplemented!()
///         }
///
///         inner(self, n, cx).await
///     }
/// }
/// ```
///
/// ### Non-defaulted functions
///
/// Applying `#[query]` to non-defaulted item is conceptually faulty.
/// On the one hand attribute wants to replace the function with a query executor, but there is
/// only a signature.
/// On the other hand this is the override point for implementors so when a function is provided
/// it will shadow query executor.
///
/// The way to solve the conundrum is to separate the concerns and make *two* functions:
/// one as override point and another as query executor.
/// This leads to a somewhat awkwardly looking "sandwich":
///
/// ```no_run
/// # use std::hash::Hash;
/// # use async_trait::async_trait;
/// # use salvia::{QueryContext, Stashable, query};
/// #[async_trait]
/// trait Rendered: Stashable + Hash {
///     async fn override_me(self, n: i32, cx: &QueryContext) -> bool;
///
///     async fn call_me(self, n: i32, cx: &QueryContext) -> bool {
///         #[query]
///         async fn inner<SelfT>(this: SelfT, n: i32, cx: &QueryContext) -> bool
///             where SelfT: Rendered
///         {
///             this.override_me(n, cx).await
///         }
///
///         inner(self, n, cx).await
///     }
/// }
/// ```
///
/// # Unsupported features
///
/// ## `impl Trait`
///
/// `impl Trait` represents an unnameable type which currently causes issues with macro
/// expansion.
///
/// ## Normal `fn` returning `Future` object
///
/// Most compelling reason:
/// *executor's `Future` type is different from original function*.
/// This is an obvious fact, but it also means there is no way to preserve the original signature.
/// Such behavior is not transparent to users and might be unexpected.
///
/// There are other technical difficulties that makes support for this feature problematic.
#[proc_macro_attribute]
pub fn query(_: TokenStream, item: TokenStream) -> TokenStream {
    use syn::{parse_macro_input, Item};

    let item = parse_macro_input!(item as Item);

    query::query_impl(item).unwrap_or_else(|e| e.to_compile_error().into())
}

/// Implement `get` and `set` functions on input's host type.
///
/// Output of the macro looks like this:
///
/// ```
/// # use std::hash::Hash;
/// # use salvia::{QueryContext, InputContext, Input, Stashable};
/// # #[derive(Eq, PartialEq, Copy, Clone, Hash)]
/// # struct Host;
/// impl Host {
///     async fn get<T>(self, cx: &QueryContext) -> T
///     where
///         Self: Input<T>,
///         T: Stashable
///     {
///         // ...
/// #       unimplemented!()
///     }
///
///     async fn set<T>(self, value: T, cx: &QueryContext)
///     where
///         Self: Input<T>,
///         T: Stashable
///     {
///         // ...
/// #       unimplemented!()
///     }
/// }
/// ```
#[proc_macro_derive(InputAccess)]
pub fn input_access(input: TokenStream) -> TokenStream {
    use syn::{parse_macro_input, DeriveInput};

    let data = parse_macro_input!(input as DeriveInput);

    input::hash_impl(data).unwrap_or_else(|e| e.to_compile_error().into())
}

#[cfg(test)]
mod test;
