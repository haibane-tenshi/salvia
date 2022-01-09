use salvia::{query, QueryContext};

#[query]
async fn f(_var1: bool, _var2: u64, _cx: &QueryContext) {}

fn main() {}
