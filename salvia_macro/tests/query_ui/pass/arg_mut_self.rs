#![allow(unused_mut, unused_assignments, unused_variables)]

use std::fmt::Debug;
use salvia::{query, QueryContext};

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct A;

impl A {
    #[query]
    async fn some(mut self, _cx: &QueryContext) {
        self = A;
    }
}

fn main() {}