# Ax density development tool

This crate owns model-specific token measurement, the cross-language source
corpus, report generation, and density regression gates. It is deliberately
separate from `crates/ax`:

- `publish = false` prevents it from shipping as a package;
- the workspace `default-members` excludes it from normal core builds;
- `tiktoken-rs` appears only in this crate's dependency tree;
- the shipped `ax` binary has no density-benchmark command.

Run the regression tests:

```sh
cargo test -p ax-density
```

Regenerate the checked-in comparison:

```sh
cargo run -p ax-density -- --write
```
