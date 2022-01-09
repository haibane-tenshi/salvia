use std::fmt::Debug;
use std::hash::Hash;
use salvia::{query, QueryContext, Stashable};

#[query]
async fn f<T>(_t: T, _cx: &QueryContext)
where T: Stashable + Debug + Hash
{}

fn main() {}
