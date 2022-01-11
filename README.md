# Salvia

[![CI](https://img.shields.io/github/workflow/status/haibane-tenshi/salvia/CI)](https://github.com/haibane-tenshi/salvia/actions/workflows/ci.yaml)
[![Security audit](https://img.shields.io/github/workflow/status/haibane-tenshi/salvia/Security%20audit?label=audit)](https://github.com/haibane-tenshi/salvia/actions/workflows/audit.yaml)

*Incremental computing framework in async Rust.*

> **Obligatory warning**
> 
> This project is highly experimental and mostly is proof-of-concept.
> Unless you absolutely need `async` consider using [`salsa`][github:salsa] crate instead.

## Primer

* [Async in Rust](https://rust-lang.github.io/async-book/)
* [Incremental computing](https://en.wikipedia.org/wiki/Incremental_computing)
* [Adapton][adapton] - research initiative for incremental computing
* [`salsa`][github:salsa] - incremental computation framework for sync Rust

`salvia` allows you to define *queries* (functions which values will be cached) and *inputs*
("functions" which value is set directly by user).
Upon execution, queries record which other queries or inputs it called and can avoid
recalculation when none of those values change.

`salvia` was inspired by `salsa` library.
As a major difference from it, this crate was designed to work in async Rust from the start
and had to make some concessions to achieve that.

## Usage example

```rust
use salvia::{query, Input, InputAccess, QueryContext, Runtime};

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, InputAccess)]
enum Team {
    Blue,
    Red,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
struct Score(u32);

impl Input<Score> for Team {
    fn initial(&self) -> Score {
        Score(0)
    }
}

#[query]
async fn leader(cx: &QueryContext) -> Option<Team> {
    use std::cmp::Ordering;

    let red: Score = Team::Red.get(cx).await;
    let blue: Score = Team::Blue.get(cx).await;

    match red.cmp(&blue) {
        Ordering::Less => Some(Team::Blue),
        Ordering::Equal => None,
        Ordering::Greater => Some(Team::Red),
    }
}

#[tokio::main]
async fn main() {
    let rt = Runtime::new().await;

    rt.mutate(|cx| async move {
        Team::Red.set(Score(1), &cx).await;
    }).await;

    let leader = rt.query(|cx| async move {
        leader(&cx).await
    }).await;

    assert_eq!(leader, Some(Team::Red));
}
```

## License

This project is dual licensed under either of

* Apache License, Version 2.0 (LICENSE-APACHE or http://www.apache.org/licenses/LICENSE-2.0)
* MIT license (LICENSE-MIT or http://opensource.org/licenses/MIT)

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in the work by you shall be dual licensed as above, without any additional terms or conditions.

[github:tokio]: https://github.com/tokio-rs/tokio
[github:salsa]: https://github.com/salsa-rs/salsa
[adapton]: http://adapton.org/