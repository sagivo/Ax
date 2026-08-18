# Ax test tree (Test & Conformance Specification v1.0)

Normative layout ([T-1.1]). Every `.ax` file carries a `//@` header ([T-1.2.1]).
Authoring by hand is last resort ([T-0.2.1]).

```
tests/
  UPSTREAM.toml          pinned remotes + licenses ([T-1.5], [T-11])
  differential/          rustc oracle, backend cross-check
  rust_ported/
    subset/              shared subset, rustc-oracle
    inverted/            Rust rejects, Ax accepts ([T-3.2])
    elision/             accept-and-elide ([T-3.3])
    divergence/          documented divergences ([T-3.4])
  conformance/           float, unicode, json, sort, fmt, numeric
  soundness/             one file per [R-5.2.3] premise
  ownership/             strategy ladder
  taint/                 Untrusted / Secret
  affine/                own T
  capability/            red team
  determinism/           G1 / G2 / G3
  diagnostics/           goldens + autofix
  perf/                  allocation / layout goldens
  protocol/              daemon, digests, GBNF, fmt
  fuzz/                  generators, reduction
  regression/            harvested failures ([T-10.4])
```

Run:

```
cargo test -p ax --test testharness
ax testharness
```

Do not vendor GPL ([T-11.3]). See `DECISIONS.md` for recorded gaps.
