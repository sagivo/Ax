# Common Ax errors (from the harness)

Generated from real diagnostics, not imagined. Wrong / right / one sentence.

## E0101 type mismatch
Wrong: `let x: i32 = 1usz;`
Right: `let x: i32 = 1i32;`
Why: no implicit numeric conversion; use `to_i64()` / `as` / a matching literal suffix.

## E0108 implicit conversion
Wrong: passing `usz` where `i32` is expected
Right: `x as i32` or `to_i64(x)`
Why: silent truncation is the highest-frequency silent-wrongness bug.

## A2021 own never used
Wrong: `fn take(p: own i32) -> i32 = 0;`
Right: `fn take(p: own i32) -> i32 = p;`
Why: affine values must be used exactly once.

## A5101 untrusted in sink
Wrong: `f"path {body}"` when `body: Untrusted<String>`
Right: `f"path {declassify(body)}"`
Why: IO data cannot reach a format/path/FFI sink without an audited declassify.

## A5001 capability not permitted
Wrong: `io.bytesum_file` with `ax.toml` `allow = ["fs"]`
Right: add `"io"` to the allow list, or route through a capability handle
Why: reachability, not the manifest, decides what a program can do.

## E0600 par overlap
Wrong: two `par` bindings that both write `x`
Right: split into disjoint locals, or stay sequential
Why: overlapping mutable captures are a data race; disjoint `par` is accepted.

## E0200 effect not permitted
Wrong: `/` or `%` without `!{err[DivError]}` when the divisor is not proven non-zero
Right: declare the row, or prove the divisor (`if d != 0`)
Why: recoverable errors stay in the row unless proven away.

## P1001 rc_not_elided
Wrong: a value that aliases across a join
Right: hoist the allocation or consume it on one path
Why: residual RC is a performance finding, not a type error.

## E0500 hole not allowed
Wrong: `?` in a file passed to `ax test` / `ax run`
Right: `ax check --allow-holes`, then fill before test/run
Why: holes are an incomplete-build status.

## E0009 / own reserved (historical)
v0.3 accepts `own T`. If you still see a reservation error, update the toolchain.
