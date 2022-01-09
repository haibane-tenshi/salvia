use std::marker::PhantomData;
use std::fmt::Display;
use salvia::InputAccess;

#[derive(InputAccess)]
struct A<T>(PhantomData<T>) where T: Display;

fn main() {}