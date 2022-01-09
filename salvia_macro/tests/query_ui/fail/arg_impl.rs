#![allow(unused_imports)]

use std::hash::Hash;
use salvia::{query, QueryContext, Stashable};

#[query]
async fn f(_a: impl Stashable + Hash, _cx: &QueryContext) {}

fn main() {}
