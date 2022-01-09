use salvia::{query, QueryContext};

struct A;

impl A {
    #[query]
    async fn f(_cx: &QueryContext) {}
}

fn main() {}
