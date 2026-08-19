# Ax

This is how you write Ax. Default parse. `ax fmt` prints this. One
spelling per construct.

A file that opens with `(` is the same language as a prefix tree
(`spec/tree.md`). Rust-shaped `fn` / `module` / `let` still parses so
the test corpus keeps proving the IR. That is not Ax. An agent does
not write it.

## Program

```
#add(a,b)=a+b
#sum(n:Z)=+/n
#sumv(xs V[Z])Z=+/xs#
#pick(b B)=b??1:0
#get(m M[S,L],k S)L=m[k]?0
#main()Z={s Z:=1;i~n{s=s*6364136223846793005+i};s}
```

A file is a sequence of declarations. The module name is the file
stem. `export` / `use` are optional; omit them unless you need them.

## Types

| write | meaning |
|---|---|
| `I` | 32-bit signed |
| `L` | 64-bit signed |
| `Y` | pointer-width signed |
| `U` | 32-bit unsigned |
| `W` | 64-bit unsigned |
| `Z` | pointer-width unsigned |
| `B` | bool |
| `F` | f64 |
| `f` | f32 |
| `S` | string |
| `O[T]` | option of `T` |
| `R[T, E]` | result of `T` or `E` |
| `M[K, V]` | map |
| `V[T]` | vec |
| `own T` | affine: use exactly once |

No implicit numeric conversion. `as` is the only one. `7L` is the
literal 7 as `L`. `7I` / `7Z` / `7W` likewise.

## Bindings

```
name T:= e          bind `name` as `T`, mutable
name:= e            bind, type inferred
name = e            assign
name += e           assign `name = name + e`   (also -= *= /= %= &= |= ^=)
name++              assign `name = name + 1`
name--              assign `name = name - 1`
```

`:=` is the only binder. There is no separate immutable form.

## Functions

```
#name(a T, b U) R = body
#name(a T) R !err[E] = body
#name() R !alloc[a] = body
#add(a,b)=a+b
#same(a,b:L)=a+b
```

`#name` starts a function. The body is an expression. A one-liner
does not need a trailing `;`. A block `{ … }` is an expression; its
value is the last expression.

An omitted parameter or result type is `I`, so `#add(a,b)=a+b` is the
same signature as `#add(a I,b I)I=a+b`. `a,b:T` shares `T` across all
parameters and the result; use it for a monomorphic non-`I` helper.

`^e` returns `e` from the enclosing function.

## Control

```
c??t:e              if `c` then `t` else `e` (right-associative)
$c{t}               if `c` then `t`
i~n { … }           `i` from 0 to `n` (exclusive)
i~lo..hi { … }      `i` from `lo` to `hi`
i~xs# { … }         `i` from 0 to `xs#`
@c{body}            while `c`
loop { … }          unbounded; adds `diverge`
break               leave the loop
continue            next iteration
match s { p => e; … }
```

`i~n` is bounded. `@c` and `loop` add `diverge` to the effect row.

## Reduce

These are the language, not helpers. Each is the loop you would have
written; the compiler sees that loop.

```
+/n                 sum of 0..n as Z
+/lo..hi            sum of lo..hi as Z
*/n                 product of 0..n as Z
+/xs#               sum of a Z-vec
*/xs#               product of a Z-vec
|/xs#               max of a Z-vec   (empty aborts)
&/xs#               min of a Z-vec   (empty aborts)
```

`+/n` and `*/n` on an empty range are 0 and 1. `+/xs#` / `*/xs#` on
an empty vec are 0 and 1. `|/` / `&/` seed from `xs[0]`; an empty
vec aborts.

A walk `i~xs# { … xs[i] … }` does not bounds-check: `i` is in range
by construction.

## Collections

```
[]                  empty vec
%                   empty map
%{"k":2L}           inferred homogeneous map literal (`M[S,L]` here)
xs#                 length of `xs`
xs[i]               element `i` (aborts if out of range, unless proven)
xs<-e               append `e`
xs[i]<-v            store `v` at `i`
m[k]<-v             insert `k → v`
m[k]?d              get `k`, or `d` if missing
```

`[]`, `%`, and `%{k:v}` allocate from the test allocator. Literal key
and value types are inferred from homogeneous literals. `xs[i]` and
`xs[i]<-v` use a numeric index. `m[k]<-v` / `m[k]?d` use a key
(usually a string).

## Errors

```
e?d                 option: value, or `d` if none
e|d                 attempt `e`: value, or `d` if it raises
raise e             fail with `e` (a branch, not unwind)
```

At most one `err[E]` in a signature. `!err[ParseError]` declares it.

## Effects

Inferred from the body, checked against the signature.

```
#f() I !err[E] = …
#f() I !alloc[a] = …
#f()I!a=…
#f() I !io[c] = …
```

`!a` is the compact spelling of the common `!alloc[a]` row.

Omit the row and `diverge` is reconstructed from `@` / `loop`. An
explicit empty row claims termination.

## What this is not

- Not a mode. There is no `--surface dense` to opt into.
- Not a coat over Rust. `fn` / `let mut` / `for i in range` are the
  corpus dialect the tests still parse. `ax fmt` will not print them.
- Not Unicode APL. Glyphs are ASCII letters and punctuation a
  tokenizer already stores as whole tokens.

Exact BPE counts against TypeScript / Python / C / Rust: `docs/usecases.md`
(`cargo run -p ax-density -- --write`, development workspace only).
