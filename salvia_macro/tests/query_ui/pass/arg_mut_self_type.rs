#![allow(unused_mut, unused_assignments, unused_variables)]

use std::fmt::Debug;
use salvia::{query, QueryContext};

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct A;

impl A {
    #[query]
    async fn some(mut self: Box<Self>, _cx: &QueryContext) {
        self = Box::new(A);
    }
}

fn main() {}