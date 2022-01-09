use std::fmt::Debug;
use std::marker::PhantomData;
use salvia::{query, QueryContext, Stashable};

struct A<T>(PhantomData<T>);

impl<T> A<T>
    where T: Stashable + Debug
{
    #[query]
    async fn some(_cx: &QueryContext) -> T {
        todo!()
    }
}

fn main() {}