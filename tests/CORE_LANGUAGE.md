# Core language suite

This directory's core-language cases are scenario ports from the upstream
language suites, adapted to Ax's expression-oriented syntax and checked by the
Ax interpreter plus the native dev and release backends.

## Coverage map

| Ax bucket | Upstream scenarios | Ax coverage |
| --- | --- | --- |
| `go_ported/core` | Go `test/{assign,bool,closure,convert,for,func,if,range,string,switch}.go` | expressions, arithmetic, booleans, assignment, calls, closures, loops, strings, records, variants |
| `rust_ported/core` | Rust `tests/ui/{block-result,closures,expr/if,fn,loops,match,binding,cast}` | block values, shadowing, closures, control flow, patterns, variants, casts, diagnostics |

The ports are deliberately semantic rather than source copies: Ax has no Go
interfaces/channels/defer or Rust traits/macros/async/borrow-checker-positive
surface. Those upstream areas remain out of scope and are recorded in
`tests/UPSTREAM.toml` and `tools/ax-dev/src/testharness.rs`.
