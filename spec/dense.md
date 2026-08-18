# S-dense — token-minimal Ax

This **is** Ax. Same AST as the Rust-shaped corpus dialect, fewer BPE
pieces. Default parse and `ax fmt`. A file that opens with `(` is the
prefix tree. There is no opt-in dense flag.

## Why these glyphs

Proxy tokenizer (and typical BPE): letters split at `_` / case / digit
boundaries; each punctuation mark is one token except `:=` `->` `==` …;
newlines cost; spaces are free.

| conventional | tokens (proxy) | dense | tokens |
|---|---:|---|---:|
| `fn` | 1 | `#` | 1 (and drops a space+ident split) |
| `i32` / `usz` / `bool` | 2 / 1 / 1 | `I` / `Z` / `B` | 1 |
| `let mut s: usz =` | 7 | `s Z:=` | 3 |
| `for i in range(0, n)` | 10 | `i~n` | 3 |
| `return` | 1 | `^` | 1 |
| `if c { t } else { e }` | 6+ | `$c{t}{e}` | 3+braces |
| `match e { Some(v) => v; None => d }` | ~12 | `e?d` | 2+ |
| `match e { Ok(v) => v; Err(_) => d }` | ~12 | `e\|d` | 2+ |
| `while c { body }` | 4+ | `@c{body}` | 2+ |
| `map.new(test.alloc)` | 8 | `%` | 1 |
| `7i64` | 2 | `7L` | 1 |
| `;` before `}` | 1 | dropped | 0 |
| newline | 1 | space (free) | 0 |

`I`/`L`/`Z`/`B`/`S` are single ASCII letters that vocabularies already
store as whole tokens. Spelled types (`i32`) usually become `i`+`32`.

## Grammar (delta from terse)

```
#name(a T, b U) R = body          fn
name T:= e                        let mut name: T = e
name:= e                          let mut name = e
i~n { … }                         for i in range(0, n)
i~lo..hi { … }                    for i in range(lo, hi)
^ e                               return e
$c{t}{e}                          if c { t } else { e }
e?d                               match e { Some(v) => v; None => d }
e|d                               match e { Ok(v) => v; Err(_) => d }
@c{body}                          while c { body }
%                                 map.new(test.alloc)
7L                                7i64
```

Type atoms: `I` i32 · `L` i64 · `Y` isz · `U` u32 · `W` u64 · `Z` usz ·
`B` bool · `F` f64 · `f` f32 · `S` String · `O` Option · `R` Result ·
`M` Map · `V` Vec. Everything else is terse.

## Example

```
#main() Z = { s Z:= 1; i~n { s = s * 6364136223846793005 + i }; s }
```

is the int_sum kernel. `ax-dense` is derived by `to_dense` and verified
to print the same value as conventional (`ax bench tokens`).
