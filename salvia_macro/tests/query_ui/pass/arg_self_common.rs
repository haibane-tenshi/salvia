use std::fmt::Debug;
use salvia::{query, QueryContext};

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct A;

impl A {
    #[query]
    async fn some(self, _a: &'static A, _int: i32, _cx: &QueryContext) -> i64 {
        todo!()
    }
}

fn main() {}