# Inverted Rust borrowck corpus ([T-3.2])

Rust rejects these; Ax must **compile, run, and report cost**.

Pinned to rustc **1.85.0** (`4d91de4e48198da2e33413efdcd9cd2cc0c46688`).
Acquire more with:

```
cargo run -p ax-dev -- harvest /path/to/rust/tests/ui
```

`unsafe` / unknown macros are recorded, not silently skipped. Cases that
need a human-specified `main` value stay at `expect: compile` until
classified ([T-3.2.3]).
