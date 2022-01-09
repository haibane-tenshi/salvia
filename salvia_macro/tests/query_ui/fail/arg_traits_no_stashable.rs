use std::marker::{PhantomData};
use salvia::{query, QueryContext};

#[derive(Hash)]
struct A(PhantomData<* mut ()>);

#[query]
async fn f(_a: A, _cx: &QueryContext) {}

fn main() {}
