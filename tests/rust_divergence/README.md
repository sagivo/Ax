# Rust divergence corpus (spec §13.3)

One case per accept-and-elide entry and per semantic divergence.

| ID | Rust writes | Ax does |
|---|---|---|
| A0101 | `&x`, `&mut x` | Parses; `&` is a no-op / `&mut` is a mutation hint |
| A0102 | lifetimes | Parsed and discarded |
| A0103 | `.clone()` | Parsed; elided when the copy is unnecessary |
| A0104 | `Box`/`Rc`/`Arc` | Treated as the inner value |
| A0105 | `RefCell` | Identity |
| A0106 | `unsafe { … }` | Runs normally; warns that it is meaningless |
| A0107 | `move \|x\|` | Accepted and ignored |
| A0108 | `println!` / `format!` / `vec!` | Rewrite to `f"…"`, `[…]` |
| P1010 | use after move of a non-affine value | Insert a copy, report the cost |
| A2020 | use after move of `own T` | Hard error |
