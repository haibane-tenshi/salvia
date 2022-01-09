#![allow(unused_imports)]

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use salvia::{query, QueryContext};

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
struct A;

impl Future for A {
    type Output = ();

    fn poll(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>
    ) -> Poll<Self::Output> {
        todo!()
    }
}

#[query]
fn f(_cx: &QueryContext) -> A {
    A
}

fn main() {}
