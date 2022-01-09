//! Helper items to support macro implementations.

pub mod checks {
    use std::hash::Hash;
    use std::marker::PhantomData;

    use crate::Stashable;

    pub struct StashableCheck<T>(pub PhantomData<T>)
    where
        T: Stashable;

    pub struct HashCheck<T>(pub PhantomData<T>)
    where
        T: Hash;

    pub struct OrdCheck<T>(pub PhantomData<T>)
    where
        T: Ord;
}
