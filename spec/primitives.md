# Primitive operation table (normative, §3.3)

Build-mode-independent. Memory safety never depends on build mode.

| Operation | Semantics | Row contribution |
|---|---|---|
| `+ - *` on integers | wrap modulo width | none |
| `checked_add`, `checked_mul`, `checked_sub` | `Option[T]` | none |
| `int.div(a,b)`, `int.rem(a,b)` | truncating | `err[DivError]` |
| `int.div_exact(a,b)` | truncating, `pre b != 0` | `abort` |
| `xs.get(i)` | `Option[T]` | none |
| `xs.at(i)` | bounds-checked in all modes | `abort` |
| `f32`/`f64` `+ - * / sqrt fma` | strict IEEE-754, canonical NaN | none |
| transcendentals | bundled deterministic libm | none |
| stack exhaustion | guard page, abort | `abort` |
| allocation failure | `err[AllocError]` on fallible APIs; `abort` on infallible | as declared |
| unbounded `loop`, recursive SCC | — | `diverge` |

A condition is guarded by **either** a precondition **or** a recoverable error, never both.
)