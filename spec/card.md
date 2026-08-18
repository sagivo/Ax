# Ax card (v0.3 / research-v1)

Surface is a **Rust subset**. `&`/`&mut`/lifetimes/`.clone()`/`unsafe` parse
and are elided. `Result`/`?`/`From` as in Rust. `own T` is affine.
`Untrusted[T]`/`Secret[T]` cannot reach sinks / logs. `ax perf --json` is
the second diagnostic loop. See `spec/v0.3.md` and `DECISIONS.md`.

Compiled systems language for LLM agents. Extension `.ax`. Text is authoritative.

## Axioms
Minimize attempts-to-green. Familiar syntax. Interfaces expose all language-level observable behavior. Pure core is deterministic; effectful runs are transcript-replayable. Every trust boundary is explicit.

## File
```
module path;
export { ident, ... };
use path as ident;
fn name[T](x: T) -> R !{err[E], io[c], alloc[a], susp, diverge, race, nondet, abort}
  pre cexpr  post cexpr
= expr;
type N = { f: T } | |V { f: T } with from F => V;
dict Ord[T] = { cmp: fn };
test "name" = expr;
```
Expression blocks. Required `;`. Mandatory top-level types. Local inference only.

## Terse surface (what an agent should write)
`--surface terse` omits what the toolchain can reconstruct: no `module`/`export`
header (the module name is the file stem), no `:` before a parameter type, no `->`
before a result, `!a+b` instead of `!{a, b}`, and no `!diverge` when `while`/`loop`
already imply it. Same AST, 0.84× the tokens.
```
fn add(a i32, b i32) i32 = a + b;
fn div(a usz, b usz) usz = { let mut x = a; while b != 0 { x = x % b }; x };
```

## Types
`i8 i16 i32 i64 isz u8 u16 u32 u64 usz f32 f64 bool byte unit`
`String` `&r str` `Vec[T]` `&r [T]` `&r mut [T]` `Option[T]` `Result[T,E]` `Ordering` `Alloc` `&r T` `&r mut T`
`own T` is affine (use exactly once; A2020 / A2021). Reserved, not in v1: `Map[K,V]` `SortedMap[K,V]`.
No implicit numeric conversion; `expr as T` is the only one. Mutation is `&mut`, never an effect.

## Ops
`+ - * ~ & | ^ << >>` wrap; shift counts mask to the operand width.
`int.div`/`rem`/`div_trunc[T]` → `err[DivError]`. `int.div_exact` `pre b!=0` → `abort`.
Operator `%` and `/` raise `err[DivError]` **only when the divisor may be zero**:
a non-zero literal, a `range(lo, _)` variable with `lo >= 1`, or a name guarded by
`while c != 0` / `if c != 0` carries no error row. The fact ends at any assignment
that could make it zero.
`checked_add`/`sub`/`mul` → `Option[T]`. `as`: narrowing wraps, float→int saturates (NaN → 0), int→float rounds.
`get` → `Option`. `at` bounds-checked always → `abort`. Floats: strict IEEE-754, canonical NaN, `NaN != NaN`.

## Control
`if` `match` `for p in seq` `while c` `loop` `break` `continue` `return`.
`for` over a finite sequence is bounded; `while` and `loop` add `diverge`. An omitted `!{…}` reconstructs `diverge` from the body. An explicit empty row still means "this terminates" and is checked.

## Stdlib (all of it)
`vec.new(a)` `xs.push(v)` `xs.at(i)` `xs.get(i)` `xs.set(i,v)` `xs.len()`
`str.concat(a,x,y)` `len(s)` `parse_i32(s)` → `err[ParseError]`
`sort(&mut xs, cmp)` — stable, `cmp: fn(&T,&T) -> Ordering`
`i32.cmp` `f32.cmp` `math.sqrt/abs/hypot` `range(a,b)` `print` `assert` `fail`
`fs.read(cap,a,path)` → `err[fs.Error]`  `json.decode_recs(a,raw)` → `err[json.Error]`
`test.alloc` `test.read_cap({ "name": contents })`
`io.*` / `http.*` take no capability: they are ambient and cost the
`capability-contained` and `replay-deterministic` labels.

## Effects
Inferred in body, checked against declaration. Omitted row reconstructs `diverge` (not `err`/`io`/`alloc`). Explicit empty row = effect-free, including termination. Pure = effect-free + no `&mut` + no mutable capture. `diverge` absence is the signal. `--strict-det` rejects `io`/`race`/`nondet`, not `diverge`.

An empty row is worth writing: it licenses the compiler to evaluate the call during the build when all arguments are literals, and to cache results at run time for self-recursive one- or two-argument integer functions. Both preserve values exactly (`ax conform` pins oracle == native). Nothing observable changes, so this is a cost property, not a semantic one.

## Errors
`raise e`  `catch e { p => h }`  `attempt e` → `Result[T,E]`
At most one `err[E]`. Injections declared once, single-step, unambiguous.

## Memory
`store(&r T, l)` legal iff `r` outlives `l` (inward only). Lexical regions. Exclusive mut. No reborrow, no interior mut, no user dtors.
A region is a bump arena and its name is an `Alloc`: `region r { vec.new(r) }`
allocates by pointer bump and releases the whole arena at the brace.
`alloc[a]` names the handle every allocation came from; a `Vec` carries its own.

## Contracts
Literals, params, `ret`, fields, Option/variant tests, cmp/bool, wrapping arith, `len`/`get`, `all`/`any`/`count`/`sorted_by` over finite seqs with contract lambdas, `contract fn`. No loops/alloc/io/errors. Never license release opts.

## Concurrency
`par` is **not implemented in v1** and is rejected by the native backend:
disjointness of mutable captures is not yet proven, and a sequential `par`
would train unsound programs. Declared shape, for when it lands:
`par { let a = …; let b = …; }` disjoint mut captures, lexical bind,
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
