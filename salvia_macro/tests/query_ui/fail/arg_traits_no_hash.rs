use salvia::{query, QueryContext};

#[derive(Copy, Clone, Eq, PartialEq)]
struct A;

#[query]
async fn f(_a: A, _cx: &QueryContext) {}

fn main() {}
