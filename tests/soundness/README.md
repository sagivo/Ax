# Read-soundness adversarial tests (spec §5.2.3)

Each clause of the `Read` soundness argument has a dedicated case:

1. The caller is suspended for the duration of the call.
2. There is no mutable global state.
3. A `Read` parameter cannot be dropped by the callee.
4. An unresolvable indirect-call target set forces `Escape`.
5. No other thread observes the value unless it was `Escape`.

These are exercised by `crates/ax/tests/v03.rs` (ownership census) and by
the differential oracle-vs-native suite. Residual RC on a `Read` parameter
is a defect in the analysis, reported by `ax perf` as `P1001`.
