# Ax density design

Ax optimizes the source stream an AI agent emits and reads back. The metric is
model BPE tokens, not Unicode code points, bytes, words, or visual terseness.

## What dense languages teach

| language family | useful idea | Ax adaptation | reason not copied literally |
|---|---|---|---|
| K / q | verbs plus `over`/`scan` replace explicit loops | `+/`, `*/`, `|/`, `&/` and counted `i~n` | Ax retains static scalar widths, effects, and explicit mutation |
| BQN / APL | single primitives, trains, pervasive array operations | expression bodies, operator fusion, inferred identities for reductions | many APL glyphs cost multiple model tokens and require a special keyboard |
| Jelly | tacit links, implicit arguments, parser-level combinators | omitted `I` signatures and structure inferred from punctuation | its code-page glyphs optimize bytes/code golf, not a general LLM vocabulary |

The semantic lesson is to remove recurring syntax, not merely rename it. A
one-character spelling can still be three BPE tokens; a familiar two-character
ASCII operator is often one.

## Vocabulary audit

Counts below are exact for both target encodings.

| spelling | `o200k_base` | `cl100k_base` | decision |
|---|---:|---:|---|
| `??` | 1 | 1 | conditional separator |
| `+/` | 1 | 1 | sum reduction |
| `:=` | 1 | 1 | bind |
| `<-` | 1 | 1 | insert/store |
| `⍳` | 3 | 3 | rejected for ranges |
| `⍴` | 3 | 3 | rejected for shape |
| `⌿` | 2 | 3 | rejected for reduction |

This is why Ax is ASCII-dense rather than APL-shaped.

## Current rules

- Omit module/export headers when the file path already supplies the identity.
- Omitted function parameter and result types are `I`: `#add(a,b)=a+b`.
- Nullary functions may omit `()`: `#f=...`.
- A colon shares a non-default type across parameters and result:
  `#sum(n:Z)=+/n`.
- Bare `M` means the common `Map[String,i32]` shape; generic maps keep
  `M[K,V]`.
- `c??t:e` is right-associative and uses a tokenizer-native operator.
- `!a` is the common allocation effect `!alloc[a]`.
- Dense map bodies infer allocation effects, so `#f={m%{e:2,o:3};...}` does
  not repeat `!a`.
- `%{"e":2L,"o":3L}` infers a homogeneous `M[S,L]` and lowers to ordinary
  checked allocation plus inserts.
- Bare interpolation quotes (`"hello {name}"`) expand to checked `f"..."`.
  A bare postfix map `?` means `?0`; simple identifier map keys may omit quotes.
- A map literal binding also establishes its key vocabulary, so `m[e]` is the
  compact checked spelling of `m["e"]`.
- `e|d` performs `attempt e` and consumes the handled error, avoiding a false
  outward error row.
- `ax fmt` removes optional whitespace and top-level terminators. Spaces are
  kept only when removing one would merge lexical tokens or create an operator.
- High-frequency loops are semantic primitives (`+/n`) so the compiler still
  sees and optimizes the loop rather than calling an opaque library helper.

## Result

The eight-case public corpus is compiler-checked and counted with the real
vocabularies:

| encoding | TypeScript | Python | C | Rust | **Ax** |
|---|---:|---:|---:|---:|---:|
| `o200k_base` | 156 | 116 | 193 | 179 | **90** |
| `cl100k_base` | 153 | 115 | 192 | 174 | **90** |

Ax is 22% smaller than the best mainstream total on `o200k_base` and 22% smaller
on `cl100k_base`. It wins or ties all eight cases in this corpus. This remains
controlled-corpus evidence, not proof for every possible program.

This establishes “smallest on the checked public corpus,” not “smallest for
every program.” The development-tool gate fails if Ax ceases to be the smallest
total in either vocabulary. New counterexamples belong in
`tools/ax-density/src/lib.rs`.

## Sources

- [BQN syntax](https://mlochbaum.github.io/BQN/doc/syntax.html) and
  [BQN primitives](https://mlochbaum.github.io/BQN/doc/primitive.html)
- [KX q iterators / cheat sheet](https://code.kx.com/q/assets/q-cheat-sheet.pdf)
- [Jelly tutorial](https://github.com/DennisMitchell/jellylanguage/wiki/Tutorial)
- [OpenAI tiktoken](https://github.com/openai/tiktoken)

Regenerate the comparison with `cargo run -p ax-density -- --write`; see
[`docs/usecases.md`](usecases.md) for every source fragment and count.
