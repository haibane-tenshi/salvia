use std::fmt::Debug;
use salvia::{query, QueryContext};

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct A;

impl A {
    #[query]
    async fn some(self: Box<Self>, _cx: &QueryContext) -> i64 {
        todo!()
    }
}

fn main() {}