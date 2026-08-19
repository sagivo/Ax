# Ax tree surface

A file that opens with `(` is this surface. The language an agent
writes is Ax (`spec/dense.md`). The Rust-shaped parser in
`spec/grammar.ebnf` is the corpus dialect, not Ax.
See `DECISIONS.md` R-14 / R-16.

## Why a tree

An LLM samples tokens. The things humans need in a language — infix
precedence, visual sugar, “also accept the other spelling”, comments
that explain intent to a reader — are costs to a sampler:

- **Precedence is silent-wrongness.** `a+b*c` has one spelling and two
  meanings. `(+ a (* b c))` has one meaning. An agent that groups
  wrong has to write the wrong grouping.
- **Sugar is a second language.** `v.x`, `xs[i]`, `e?`, `a + b` are
  convenience for a person who reads. An agent reads `ax types` /
  `ax hole` / `ax ir` and writes once. One form per construct.
- **Accept-and-elide trains Rust.** Forgiving `pub`, `unsafe`,
  lifetimes, `.clone()` exists so a model that emits Rust is not
  stuck. That is a coat. The tree has nothing to elide.
- **Constrained decoding is a list.** GBNF for a prefix tree is
  `list ::= "(" form* ")"`. GBNF for Rust is a reconstruction of
  Rust.
- **`fmt` is a bijection.** The printer is the inverse of the parser.
  Patches are tree edits. There is no layout debate.

What humans needed and we dropped: infix, precedence tables, statement
vs expression punctuation, visual nesting via braces-and-semicolons,
keyword synonyms, “also parse the Rust form”.

What stays, because it is not for humans: effects in the signature,
regions, affine `own`, `Untrusted`/`Secret`, holes, four agreeing
backends, a protocol that answers in microseconds.

Rust is still the compiler. That is an implementation choice. It is
not the identity of Ax.

## Grammar

Whitespace and `;…` / `//…` / `/*…*/` comments are ignored.

```
file     ::= form+ | (module path form*)
form     ::= atom | string | list
list     ::= "(" form* ")"
atom     ::= [A-Za-z0-9_+*/%<>=!&|.?~^-]+
string   ::= '"' char* '"'
```

A file that opens with `(` uses this machine-oriented representation and is
detected automatically.

### Module

```
(module path
  (export ident…)
  (use path [as ident])
  (@ key value?)
  decl…)
```

Omitting `(module …)` is legal: the file stem is the module name, and
the remaining forms are the body.

### Declarations

```
(fn [(T…)] name ((x T)…) Ret [(! effect…)] [(pre e)] [(post e)] body)
(contract fn name ((x T)…) Ret body)
(type Name [(T…)] body [(from T V)…])
(dict Name T (field expr)…)
(test "name" expr)
```

Params are **one** list. `()` is empty. `((a i32) (b i32))` is two
parameters. `(fn add (a i32) (b i32) i32 …)` is rejected: grouping is
what keeps `(Result i32 E)` a return type.

Type bodies:

```
(rec (field T)…)
(or Name (Name (field T)…)…)
alias
```

Effects:

```
(!)
(! (err E) (io c) (alloc a) susp diverge race nondet abort)
```

An omitted `(!)` reconstructs `diverge` from `while`/`loop`. An
explicit empty `(!)` is a termination claim.

### Types

```
i32 | ? | (own T) | (untrusted T) | (secret T)
(ref T) | (ref r T) | (ref r mut T)
(fn (T…) R [(! …)])
(tuple T…)
(Name T…)          ; Vec, Option, Result, …
```

### Expressions

Atoms: names (`foo`, `math.hypot`), literals (`1`, `1i32`, `3.0`,
`true`, `false`, `"hi"`), `?` (hole), `break`, `continue`.

Lists, by head:

| head | form | AST |
|---|---|---|
| `+ - * / % == != < <= > >= && \|\| & \| ^ << >>` | `(op a b)` | Binary |
| `not bnot neg ref refmut deref` | `(op e)` | Unary |
| `as` | `(as e T)` | Cast |
| `field` | `(field e name)` | Field |
| `index` | `(index e i)` | Index |
| `let` | `(let [mut] pat [T] e)` | Let |
| `set` | `(set lhs rhs)` | Assign |
| `block` | `(block form… tail?)` | Block |
| `if` | `(if c t [e])` | If |
| `match` | `(match s (arm p e)…)` | Match |
| `for` | `(for pat seq body)` | For |
| `while` | `(while c body)` | While |
| `loop` | `(loop body)` | Loop |
| `return` | `(return [e])` | Return |
| `raise` | `(raise e)` | Raise |
| `catch` | `(catch e (arm p e)…)` | Catch |
| `attempt` | `(attempt e)` | Attempt |
| `try` | `(try e)` | Try |
| `rec` | `(rec (f e)…)` | Record |
| `var` | `(var Name payload…)` | Variant |
| `fn` | `(fn ((x T)…) [R] body)` | Lambda |
| `region` | `(region r body)` | Region |
| `par` | `(par (let …)…)` | Par |
| `interp` | `(interp "lit" e …)` | Interpolate |
| otherwise | `(callee arg…)` | Call |

A leftover form after a `fn` body is an error. There is no infix.

### Patterns

`_` · literals · names · `(rec (f p)…)` · `(tuple p…)` ·
`(var Name payload…)` · `(Name payload…)` (positional variant).

## Printer

`ax fmt` on a tree file emits `tree::format_file`. The printer is the
inverse of the parser: parse → print → parse is identity on the AST,
and print → parse → print is identity on the text.

## Protocol

`ax hole --fills` on a tree source proposes tree expressions
(`(field v x)`, `(math.hypot (field v x) (field v y))`), not
`v.x` / `math.hypot(v.x, v.y)`. `ax gbnf` prints the tree list
grammar.

## What is not here

No `pub`. No `unsafe`. No lifetimes. No `.clone()`. No `?` postfix
sugar (`(try e)` is the form). No `struct` / `enum` / `impl` / `trait`.
Those exist only in the corpus dialect, and only so the four backends
keep proving the same IR.
