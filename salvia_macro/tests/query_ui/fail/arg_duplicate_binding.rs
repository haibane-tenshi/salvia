#![allow(unused_imports)]

use salvia::{query, QueryContext};

#[query]
async fn f(_i: i32, _i: u64, _cx: &QueryContext) {}

fn main() {}
