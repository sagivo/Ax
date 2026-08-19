# Ax repository development tools

`ax-dev` owns validation and research machinery that must not ship in the Ax
compiler or library. The crate is `publish = false` and excluded from the
workspace's default build.

It contains:

- native/runtime benchmarks and software-use-case reports;
- conformance and external test-suite runners;
- differential fuzzing and GBNF equivalence sampling;
- attempts-to-green, K1, silent-wrongness, and kill-criteria experiments;
- Rust test harvesting, Rust-to-Ax translation, ax-mock, and proxy token
  accounting.

Run `cargo run -p ax-dev -- help` for commands. `cargo test -p ax-dev` runs its
dedicated suites. Model-specific density measurement remains isolated further
in the separate `ax-density` crate.
