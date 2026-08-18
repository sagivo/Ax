# Read-soundness adversarial tests ([R-5.2.3] / [T-5.1])

One file per premise. Each asserts the *specific* mechanism that stops the attack.

| File | Premise | Mechanism |
|---|---|---|
| `read_caller_suspended.ax` | Caller is suspended | Direct static call; no re-entrant dispatch |
| `read_no_mutable_globals.ax` | No mutable globals | `static mut` rejected at parse (E0002) |
| `read_callee_cannot_drop.ax` | Callee cannot drop a Read | Primitive Read is a register copy |
| `read_escape_closure.ax` | Unknown targets force Escape | Returned record → residual RC |
| `read_not_thread_visible.ax` | Non-Escape is thread-invisible | Overlapping `par` `&mut` is E0600 |
| `rc_vs_unique.ax` | RC-everywhere == unique-heap | Same value on both strategies |
