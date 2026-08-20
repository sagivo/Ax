# Ax compiler protocol

Machine interface for agents. Prefer `--json` on every command that accepts it.
Text source is authoritative. The interpreter is the normative oracle.

## Install

```
cargo install --path crates/ax
```

Requires Rust 1.94+ and a C11 compiler. `ax jit` does not need `cc`.

## Surfaces

| flag | meaning |
|---|---|
| default / `--surface ax` | dense Ax (`#fn`, `:=`, `$if`, `+/`) |
| file opens with `(` | prefix tree |
| `--surface tree` | force tree |

Do not write Rust-shaped `fn` / `module` / `let`. That form exists so the
corpus can prove the IR.

## Loop

1. `ax check --json --allow-holes file.ax`
2. If holes: `ax hole --fills --json file.ax` — every fill has been substituted
   and type-checked. Apply one. Repeat.
3. `ax fix --apply` only applies `semantics_preserving` edits. Do not invent
   other rewrites.
4. `ax test` / `ax jit file.ax` / `ax build --tier release`.
5. Effectful runs: `ax run --seed N --trace f` then `ax replay f file.ax`
   (replay performs no IO).

## Commands

```
ax check [--json] [--allow-holes] [--strict-det] [--surface ax|tree]
ax hole [--fills] [--json]
ax fix [--apply]
ax test
ax run --seed N --trace f
ax jit <file> [args]
ax replay f source.ax
ax ir
ax types
ax effs
ax search
ax errs --into T
ax fmt
ax patch --tx
ax deps --affected
ax build [-o bin] [--tier dev|release|portable]
ax merge --semantic
ax label
ax card
ax pkg list | pkg write
ax perf [--json] [--diff baseline.json]
ax complete
ax context
ax repair
ax caps
ax gbnf
ax daemon
```

`ax complete --at` returns type-correct completions and a GBNF fragment of the
tree grammar. Target: syntax_err_rate = 0, hallucinated symbol rate = 0.

`ax perf --json` is the second diagnostic loop (ownership ladder, surviving
checks). Contracts such as `#[pure]`, `#[no_alloc]`, `#[no_panic]` refuse to
compile on violation.

## Latency (why the loop is this language)

| step | Ax | control |
|---|---:|---|
| check a module | 176 µs | rustc 1.18 s |
| compile and run | 417 µs | Cranelift, no cc; `cc` alone 74 ms |
| ax + protocol median wall | 0.6–1.0 ms | rust + protocol 361–612 ms |

The attempt-count win is the protocol (Rust + a protocol also hits 1.0
attempts). The latency win is the language implementation: in-process check
and Cranelift. No wrapper over `rustc` reaches it; `--emit=metadata` alone
is 37 ms.

## Safety the protocol cannot copy

Measured by `ax-dev silent-wrongness` (no model involved):

| | ax | rust |
|---|---:|---:|
| silent wrongness | 36% | 64% |
| tier-divergent | 0% | 18% |
| mechanism for the hazard class | 100% | 55% |

Three backends (oracle interpreter, C11, Cranelift) must agree on every
conformance case. Disagreement is a bug, not a vote.

## Capabilities

`ax label` reports what the program earned. `io.*` / `http.*` are ambient and
drop `capability-contained` and `replay-deterministic`. `fs` requires a
handle. `..`, absolute paths, and `widen` fail closed.

## Tiers

| tier | backend | use |
|---|---|---|
| `ax jit` | Cranelift, in-process | agent loop, ~0.3 ms |
| `--tier dev` | C11 | iteration |
| `--tier release` | C11 / LLVM-class via cc -O3 | ship |
| `--tier portable` | C11 portable | ship elsewhere |
| oracle | interpreter | normative |

## HTTP

```
fn handle(request: http.Request) -> http.Response = http.response(200u16, "{\"ok\":true}");
fn main() -> unit !{io[net], abort} = http.serve_handler(8080u16, handle);
```

`ax build --tier release app.ax`.

For verb routes and `api.*` helpers, use the standalone `ax-api` package.
See [api.md](api.md). Do not assume a router inside the compiler.

## Example hole fill

```
$ ax hole --fills examples/holes.ax
hole examples.holes::fn:distance  expects: f32
  7 of 7 candidates compile
    1  v.x    in scope, exact type
    2  v.y    in scope, exact type
    3  math.hypot(v.x, v.y)    prelude call, matching result type
```
