use std::marker::{PhantomData};
use salvia::{query, QueryContext};

struct A(PhantomData<* mut ()>);

#[query]
async fn f(_cx: &QueryContext) -> A {
    todo!()
}

fn main() {}
