#![allow(unused_imports)]

use std::fmt::Debug;
use salvia::{query, QueryContext, Stashable};

#[query]
async fn f(_cx: &QueryContext) -> impl Stashable + Debug {
    todo!()
}

fn main() {}
