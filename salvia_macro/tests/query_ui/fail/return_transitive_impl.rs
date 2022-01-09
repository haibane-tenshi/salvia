#![allow(unused_imports)]

use std::fmt::Debug;
use salvia::{query, QueryContext};

#[query]
async fn f(_cx: &QueryContext) -> (impl Debug + Clone + Eq + 'static,) {
    todo!()
}

fn main() {}
