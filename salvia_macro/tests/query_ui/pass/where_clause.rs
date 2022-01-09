use salvia::{query, QueryContext};

#[query]
async fn f(_cx: &QueryContext) where i32: Clone {}

fn main() {}
