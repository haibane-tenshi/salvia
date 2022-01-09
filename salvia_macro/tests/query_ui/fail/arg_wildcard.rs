#![allow(unused_imports)]

use salvia::{query, QueryContext};

#[query]
async fn f(_: i32, _cx: &QueryContext) {}

fn main() {}
