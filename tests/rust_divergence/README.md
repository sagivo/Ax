# Rust divergence corpus

Moved to `tests/rust_ported/divergence/` (Test Spec v1.0 §3.4).

| ID | Rust writes | Ax does |
|---|---|---|
| A0101 | `&x`, `&mut x` | Parses; `&` is a no-op / `&mut` is a mutation hint |
| A0102 | lifetimes | Parsed and discarded (pending emit — see `DECISIONS.md`) |
| A0103 | `.clone()` | Identity; warned |
| A0104 | `Box`/`Rc`/`Arc` | Treated as the inner value |
| A0105 | `RefCell` | Identity |
| A0106 | `unsafe { … }` | Runs normally; warns that it is meaningless |
| A0107 | `move \|x\|` | Accepted; warned |
| A0108 | `println!` / `format!` / `vec!` | `ax-dev translate` (pending emit in `ax check`) |
| P1010 | use after move of a non-affine value | Insert a copy, report the cost |
| A2020 | use after move of `own T` | Hard error |
