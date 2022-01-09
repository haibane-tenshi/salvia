use salvia::{query, QueryContext};
use async_recursion::async_recursion;

#[query]
#[async_recursion]
async fn fibonacci(n: u64, cx: &QueryContext) -> u64 {
    match n {
        0 => 0,
        1 => 1,
        n => fibonacci(n - 1, cx).await + fibonacci(n - 2, cx).await
    }
}

fn main() {}