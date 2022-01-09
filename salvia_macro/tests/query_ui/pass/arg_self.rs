use std::fmt::Debug;
use salvia::{query, QueryContext};

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct A;

impl A {
    #[query]
    async fn some(self, _cx: &QueryContext) -> i64 {
        todo!()
    }
}

fn main() {}