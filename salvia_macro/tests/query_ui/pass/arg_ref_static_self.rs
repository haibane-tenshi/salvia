#![allow(unused_mut)]

use std::fmt::Debug;
use salvia::{query, QueryContext};

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct A;

impl A {
    #[query]
    async fn some(&'static self, _cx: &QueryContext) {}
}

fn main() {}