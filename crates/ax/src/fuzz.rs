//! Differential fuzz: RC-everywhere oracle vs optimizing native backend.
//!
//! Generates small typed programs (aliasing / branch / recursion pressure)
//! and asserts interpreter and native print the same value.

fn numeric_prefix(s: &str) -> String {
    s.chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect()
}

use crate::codegen;
use crate::driver::{run_main, Session};
use std::process::Command;

pub fn generate(seed: u64) -> String {
    let kind = (seed >> 3) % 8;
    let a = (seed % 20) as i32;
    let b = ((seed >> 8) % 20) as i32;
    match kind {
        0 => format!("module t;\nfn main() -> i32 = {a} + {b};\n"),
        1 => format!(
            "module t;\nfn f(x: i32) -> i32 = if x < 1 {{ 1 }} else {{ x + f(x - 1) }};\nfn main() -> i32 = f({});\n",
            (seed % 8) + 1
        ),
        2 => format!(
            "module t;\nfn main() -> i32 = {{ let mut s: i32 = 0; for i in range(0, {}) {{ s = s + (i as i32); }}; s }};\n",
            (seed % 12) + 1
        ),
        3 => format!(
            "module t;\nfn main() -> i32 = {{ let x: i32 = {a}; let y = x; y + {b} }};\n"
        ),
        4 => format!("module t;\nfn main() -> bool = {a} < {b};\n"),
        5 => format!(
            "module t;\nfn main() -> i32 = match {a} < {b} {{ true => {a}; false => {b}; }};\n"
        ),
        6 => format!(
            "module t;\nfn main() -> i64 !{{alloc[a]}} = {{ let mut xs: Vec[i64] = vec.new(test.alloc); xs.push({}i64); xs.at(0) }};\n",
            a
        ),
        _ => format!(
            "module t;\nfn main() -> i64 !{{alloc[a]}} = {{ let mut m: Map[String, i64] = map.new(test.alloc); m.insert(\"k\", {}i64); match m.get(\"k\") {{ Some(v) => v; None => 0; }} }};\n",
            a
        ),
    }
}

/// Run `n` random programs. Returns the number of disagreements.
pub fn differential(n: usize, seed: u64) -> usize {
    let mut fails = 0;
    let mut s = seed | 1;
    let dir = std::env::temp_dir().join("ax-fuzz");
    let _ = std::fs::create_dir_all(&dir);
    for i in 0..n {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
        let src = generate(s);
        let mut sess = Session::new();
        let Ok(out) = sess.compile("fuzz.ax", &src) else {
            continue;
        };
        let Ok(interp) = run_main(&sess.intern, &out, 0) else {
            continue;
        };
        let want = interp.display();
        match codegen::build_native(&sess.intern, &out, &format!("fz{i}"), &dir) {
            Ok(br) => {
                if let Ok(run) = Command::new(&br.bin_path).output() {
                    let got = String::from_utf8_lossy(&run.stdout);
                    let g = numeric_prefix(got.trim());
                    let w = numeric_prefix(want.trim());
                    if g != w {
                        fails += 1;
                    }
                } else {
                    fails += 1;
                }
            }
            Err(_) => fails += 1,
        }
    }
    fails
}
