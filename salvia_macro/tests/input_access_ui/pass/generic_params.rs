use std::marker::PhantomData;
use salvia::InputAccess;

#[derive(InputAccess)]
struct A<T>(PhantomData<T>);

fn main() {}