# Ax card (v0.3 / research-v1)

**The language an agent writes is a prefix tree.** There is no operator
precedence, no infix, no accept-and-elide, and exactly one spelling of each
construct. The tree *is* the AST. See `spec/v0.3.md` and `DECISIONS.md`.

The language an agent writes is the short syntax (`#fn`, `:=`, `$if`,
`i~n`, type glyphs `I`/`Z`/`B`). That is the default — there is no
opt-in dense mode. See `spec/dense.md`. A file that opens with `(` is
the prefix tree. Rust-shaped conventional / terse / verbose remain only
so the corpus keeps proving the IR. Side-by-side Rust / C / Go / Ax
syntax + tokens: `docs/usecases.md` (`ax bench usecases`).

`own T` is affine. `Untrusted[T]`/`Secret[T]` cannot reach sinks / logs.
`ax perf --json` is the second diagnostic loop.

Compiled systems language for LLM agents. Extension `.ax`. Text is authoritative.
A file that opens with `(` is the tree.

## Axioms
Minimize attempts-to-green. One tree, no sugar. Interfaces expose all language-level observable behavior. Pure core is deterministic; effectful runs are transcript-replayable. Every trust boundary is explicit.

## File (tree — what an agent writes)
```
(module path
  (export ident ...)
  (use path as ident)
  (fn [(T)] name ((x T) ...) R [(! (err E) (io c) (alloc a) susp diverge race nondet abort)]
    [(pre e)] [(post e)]
    body)
  (type N (rec (f T)) | (or V (V (f T))) [(from F V)])
  (dict Ord T (cmp fn))
  (test "name" expr)
)
```
No `;`. No `->`. No infix. Mandatory types on every `fn`. Local inference only.

```
(fn add ((a i32) (b i32)) i32 (+ a b))
(fn div ((a usz) (b usz)) usz
  (block (let mut x a) (while (!= b 0) (set x (% x b))) x))
```

## Types
`i8 i16 i32 i64 isz u8 u16 u32 u64 usz f32 f64 bool byte unit`
`String` `(ref r str)` `(Vec T)` `(ref r (slice T))` `(ref r mut (slice T))` `(Option T)` `(Result T E)` `Ordering` `Alloc` `(ref r T)` `(ref r mut T)`
`(own T)` is affine (use exactly once; A2020 / A2021). Reserved, not in v1: `(Map K V)` `(SortedMap K V)`.
No implicit numeric conversion; `(as expr T)` is the only one. Mutation is `(refmut e)`, never an effect.

## Ops
Prefix: `(+ a b)` `(- * / % == != < <= > >= && || & | ^ << >>)`. Wrap; shift counts mask to width.
`(int.div a b)` / `rem` / `(int.div_trunc a b)` → `(err DivError)`. `int.div_exact` `(pre (!= b 0))` → `abort`.
`(/ a b)` and `(% a b)` raise `(err DivError)` **only when the divisor may be zero**:
a non-zero literal, a `(range lo _)` variable with `lo >= 1`, or a name guarded by
`(while (!= c 0) …)` / `(if (!= c 0) …)` carries no error row. The fact ends at any `(set …)`
that could make it zero.
`checked_add`/`sub`/`mul` → `(Option T)`. `(as e T)`: narrowing wraps, float→int saturates (NaN → 0), int→float rounds.
`get` → `Option`. `at` bounds-checked always → `abort`. Floats: strict IEEE-754, canonical NaN, `NaN != NaN`.

## Control
`(if c t e)` `(match s (arm p e)…)` `(for p seq body)` `(while c body)` `(loop body)` `break` `continue` `(return e)`.
`for` over a finite sequence is bounded; `while` and `loop` add `diverge`. An omitted `(!)` reconstructs `diverge` from the body. An explicit empty `(!)` still means "this terminates" and is checked.

## Stdlib (all of it)
`(vec.new a)` `(xs.push v)` `(xs.reserve n)` `(xs.at i)` `(xs.get i)` `(xs.set i v)` `(xs.len)`
`(m.insert k v)` `(m.add k d)` `(m.get k)` → `Option`  `(m.len)`
`(str.concat a x y)` `(str.from_byte a b)` `(len s)` `(parse_i32 s)` → `(err ParseError)`
`(sort (refmut xs) cmp)` — stable, `cmp: (fn ((ref T) (ref T)) Ordering)`
`i32.cmp` `f32.cmp` `math.sqrt/abs/hypot` `(range a b)` `print` `assert` `fail`
`(fs.read cap a path)` → `(err fs.Error)`  `(json.decode_recs a raw)` → `(err json.Error)`
`test.alloc` `(test.read_cap (rec (name contents)))`
`io.*` / `http.*` take no capability: they are ambient and cost the
`capability-contained` and `replay-deterministic` labels.

## Effects
Inferred in body, checked against declaration. Omitted row reconstructs `diverge` (not `err`/`io`/`alloc`). Explicit empty row = effect-free, including termination. Pure = effect-free + no `&mut` + no mutable capture. `diverge` absence is the signal. `--strict-det` rejects `io`/`race`/`nondet`, not `diverge`.

An empty row is worth writing: it licenses the compiler to evaluate the call during the build when all arguments are literals, and to cache results at run time for self-recursive one- or two-argument integer functions. Both preserve values exactly (`ax conform` pins oracle == native). Nothing observable changes, so this is a cost property, not a semantic one.

## Errors
`(raise e)`  `(catch e (arm p h)…)`  `(attempt e)` → `(Result T E)`
At most one `(err E)`. Injections declared once, single-step, unambiguous.

## Memory
`store` of `(ref r T)` into `l` is legal iff `r` outlives `l` (inward only). Lexical regions. Exclusive mut. No reborrow, no interior mut, no user dtors.
A region is a bump arena and its name is an `Alloc`: `(region r (vec.new r))`
allocates by pointer bump and releases the whole arena at the close.
`(alloc a)` names the handle every allocation came from; a `Vec` carries its own.

## Contracts
Literals, params, `ret`, fields, Option/variant tests, cmp/bool, wrapping arith, `len`/`get`, `all`/`any`/`count`/`sorted_by` over finite seqs with contract lambdas, `contract fn`. No loops/alloc/io/errors. Never license release opts.

## Concurrency
`par` is **not implemented in v1** and is rejected by the native backend:
disjointness of mutable captures is not yet proven, and a sequential `par`
would train unsound programs. Declared shape, for when it lands:
`(par (let a …) (let b …))` disjoint mut captures, lexical bind,
lowest-index error wins, cancel at next yield.

## Caps / FFI
Capability handles are required for `fs`; `io.*`/`http.*` are ambient and
labelled as such. `trusted extern` is labeled `trusted-ffi` and excludes `safe`/`capability-contained` unless an OS sandbox is declared. Strict mode forbids raw FFI.

## Protocol
`ax check [--json] [--allow-holes] [--strict-det]`
`ax hole <def>` `ax types` `ax effs` `ax search` `ax errs --into T`
`ax fmt` `ax patch --tx` `ax deps --affected`
`ax test` `ax run --seed N --trace f` `ax replay f` `ax jit <file>` (compile+run, no cc)
`ax merge --semantic` `ax label` `ax card` `ax ir` `ax conform`
Fixes: only `semantics_preserving` auto-applied. `?` legal under `--allow-holes`; rejected by test/run/release.

## Dicts
`= default` resolves to the unique visible `dict D[T]`. Zero or two is an error.
