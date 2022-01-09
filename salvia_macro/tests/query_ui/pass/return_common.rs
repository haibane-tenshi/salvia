use salvia::{query, QueryContext};

#[query]
async fn f(_cx: &QueryContext) -> i32 {
    todo!()
}

fn main() {}
