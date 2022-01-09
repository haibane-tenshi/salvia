# Contribution guidelines

First off, thank you for considering contributing to Salvia!

If your contribution is not straightforward, please first discuss the change you
wish to make by creating a new issue before making the change.

There are also some potential features discussed in [design doc][repo:design_soc].

## Reporting issues

Before reporting an issue on the [issue tracker][repo:issue_tracker],
please check that it has not already been reported by searching for some related keywords.

## Pull requests

Try to do one pull request per change.

### Updating the changelog

Update the changes you have made in [CHANGELOG][repo:changelog] file under the **Unreleased** section.

Add the changes of your pull request to one of the following subsections,
depending on the types of changes defined by [Keep a changelog][keepachangelog]:

- `Added` for new features.
- `Changed` for changes in existing functionality.
- `Deprecated` for soon-to-be removed features.
- `Removed` for now removed features.
- `Fixed` for any bug fixes.
- `Security` in case of vulnerabilities.

If the required subsection does not exist yet under **Unreleased**, create it!

## Developing

### Set up

This is no different than other Rust projects.

```shell
git clone https://github.com/haibane-tenshi/salvia.git
cd salvia
cargo test
```

### Useful Commands

- Run Clippy:

  ```shell
  cargo clippy --all-targets --features async-trait --workspace
  ```

- Run all tests:

  ```shell
  cargo test --features async-trait --workspace
  ```

- Check to see if there are code formatting issues

  ```shell
  cargo fmt --all -- --check
  ```

- Format the code in the project

  ```shell
  cargo fmt --all
  ```
  
- Compile documentation

  ```shell
  cargo +nightly doc --features nightly
  ```

### Some test cases in `salvia_macro` crate failed, but I didn't touch it. What happened?

`salvia_macro` provides procedural macro implementations.
To verify that macros produce expected diagnostic they are run through so-called "UI tests".
You can read about the idea in more detail in documentation to [`trybuild`][docs.rs:trybuild]
(which is test harness we are using).

The gist of it, UI tests also include failed cases where macro should fail with specific error messages.
This is where things can go wrong:

* Some of the diagnostic is semantic, i.e. it reuses existing `rustc` diagnostic.
  However, compiler's diagnostic is not part of SemVer guarantees, so it can change between releases.
  
* It is *very* hard to provide clean diagnostics.
  Some failure messages refer to locations of specific items in `salvia` crate.
  Simply modifying files can move those items around, which breaks tests cases.

#### What should I do?

Check differing test output,
in case changes are small/cosmetic (changed line numbers etc.) you can just add it right into PR.

`trybuild` puts newly produced output in a `salvia/salvia_macro/wip/` folder.
Alternatively, you can run tests with `TRYBUILD=overwrite` environment variable,
so you can run `git diff` in place or your editor/IDE integrates with git.

### Using `tracing`

Crate provides `tracing` feature which enables logging for internals of the library itself.
Feel free to use it for exploration or debugging.

**Warning**: don't use feature in production, it modifies definition of `Stashable` trait (by adding `Debug`) and
some internals to provide meaningful output.

You will need to set up logging subscriber as described in
[`tracing` crate documentation][docs.rs:tracing#in-executables].

Logging levels:
* `INFO` - logs all important state changes directly observable by user.
  Only events are emitted.

  This level includes:
  * Runtime phase transitions
  * Spawn of new nodes (both query and input)
  * Recording of new values (both query and input)

* `DEBUG` - logs all communication between tasks which can affect any internal state.
  Both events and spans are emitted.

  This level includes:
  * Clock task: spawn, phase transition processing, index requests by input nodes
  * Input node: set value, get value, validation, actualization requests; index acquisition
  * Query node: get value, validation requests; function evaluation

* `TRACE` - reserved for fine-grained per-function logging. Not implemented at the moment.

[repo:issue_tracker]: https://github.com/haibane-tenshi/salvia/issues
[repo:changelog]: ./CHANGELOG.md
[repo:design_soc]: ./DESIGN_DOC.md
[keepachangelog]: https://keepachangelog.com/en/1.0.0/
[docs.rs:trybuild]: https://docs.rs/trybuild/latest/trybuild/
[docs.rs:tracing#in-executables]: https://docs.rs/tracing/latest/tracing/#in-executables