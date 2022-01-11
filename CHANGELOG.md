# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog][keepachangelog], 
and this project adheres to [Semantic Versioning][semver].

[keepachangelog]: https://keepachangelog.com/en/1.1.0/
[semver]: https://semver.org/spec/v2.0.0.html 

## [Unreleased]

## [0.1.0] - 2022-01-11

### Added

* Initial release.
* API surface: `#[query]` macro, `Input` trait, `InputAccess` derive macro,
  `InputAccess` trait (behind `async-trait` feature).
* `tracing` feature for the purpose of debugging library itself.