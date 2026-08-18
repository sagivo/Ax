# Common Ax errors (from the harness)

Generated from real diagnostics, not imagined. Wrong / right / one sentence.

## E0101 type mismatch
Wrong: `x I:= 1Z`
Right: `x I:= 1I`
Why: no implicit numeric conversion; use `as` / a matching literal suffix.

## E0108 implicit conversion
Wrong: passing `Z` where `I` is expected
Right: `x as I`
Why: silent truncation is the highest-frequency silent-wrongness bug.

## A2021 own never used
Wrong: `#take(p own I) I = 0`
Right: `#take(p own I) I = p`
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

## E0100 unknown name
Wrong: `fn main() -> i32 = no_such_name;`
Right: bind the name, or import the module that defines it
Why: every path must resolve; the harness emits this from `tests/diagnostics/unknown_name.ax`.

## E0103 arity mismatch
Wrong: `add(1)` when `add` takes two arguments
Right: `add(1, 2)`
Why: Ax does not default missing arguments.

## E0104 unknown field
Wrong: `r.nope` on `{ x: i32 }`
Right: `r.x`
Why: field names are checked, not delayed to runtime.

## E0106 not a function
Wrong: `let x: i32 = 1; x(2)`
Right: call a function, or index/match the value
Why: only function-typed values are callable.

## E0112 non-exhaustive match
Wrong: `match c { Red => 1; Green => 2; }` on `| Red | Green | Blue`
Right: cover every variant, or add `_ => …`
Why: a missed case is an abort at runtime if it were accepted.

## A2020 own use-after-move
Wrong: `#take(p own I) I = p + p`
Right: `#take(p own I) I = p`
Why: affine `own T` is used exactly once; a second use is A2020 (`tests/affine/use_after_move.ax`).

## A5102 Secret in format
Wrong: `f"secret {s}"` when `s: Secret[i32]`
Right: do not format secrets; declassify only at an audited sink
Why: Secret cannot reach f-strings (`tests/taint/secret_fstring.ax`).

## capturing lambda (native)
Wrong: `|x| x + n` when `n` is a local
Right: `|x, n| x + n` and pass `n` at the call site
Why: v1 function values are bare pointers; they do not carry an environment.

## this is Ax
Wrong: writing `fn` / `let mut` / `for i in range` because that looks like Rust
Right: `#name`, `:=`, `i~n` — that is the language; `ax fmt` prints it
Why: the Rust-shaped form is the corpus dialect. An agent does not write it (`spec/dense.md`).

## `+/` is not division
Wrong: `a+/n` for `a + (sum 0..n)`
Right: `a + +/n` or `a+(+/n)`
Why: `+/` is plus-over and must not sit against an ident (`spec/dense.md`).

## `[]` after `=` is an empty vec
Wrong: `xs V[Z]:= []` confused with the type `V[Z]`
Right: `[]` after `=` / `,` / `(` is `vec.new`; `V[Z]` after a name is a type
Why: the previous byte decides (`spec/dense.md`).

## `m[k]<-v` vs `xs[i]<-v`
Wrong: `xs["k"]<-v` for a vec store
Right: numeric index → `set`; string key → `insert`
Why: the key's shape picks the operation (`spec/dense.md`).
