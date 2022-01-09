#![allow(unused_mut, unused_assignments, unused_variables)]

use std::fmt::Debug;
use salvia::{query, QueryContext};

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct A;

const VALUE: A = A;

impl A {
    #[query]
    async fn some(mut self: &'static Self, _cx: &QueryContext) {
        self = &VALUE;
    }
}

fn main() {}