#![allow(unused_assignments, unused_variables)]

use salvia::{query, QueryContext};

#[query]
async fn f(mut var1: bool, mut var2: u64, _cx: &QueryContext) {
    var1 = true;
    var2 = 3;
}

fn main() {}
