#![allow(unused_imports)]

use salvia::{query, QueryContext};

#[query]
async fn f(_i: i32, _: &QueryContext) {}

fn main() {}
