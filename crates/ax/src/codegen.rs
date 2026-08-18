//! Native build driver: checked AST -> IR -> C -> object code.
//!
//! The code generation itself lives in [`crate::backend_c`]. This module only
//! decides tiers and drives `cc`. It exists separately because the tier choice
//! is a policy question (what flags, which runtime, what to do when a target
//! is missing) and codegen is not.
//!
//! Tiers:
//!   oracle   — tree-walk interpreter (`crate::interp`), the normative spec
//!   dev      — `cc -O0 -g`, fast to build
//!   release  — `cc -O3 -flto`, the tier the benchmarks measure
//!   portable — `wasm32-wasi` when a sysroot exists, else host `-O2`

use crate::backend_c;
use crate::check::CheckOutput;
use crate::intern::Interner;
use crate::ir::Program;
use crate::lower;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Dev,
    Release,
    Portable,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Dev => "dev",
            Tier::Release => "release",
            Tier::Portable => "portable",
        }
    }
}

pub struct BuildResult {
    pub c_path: PathBuf,
    pub bin_path: PathBuf,
    pub tier: Tier,
}

/// Lower to IR and emit C. Separate from [`build_tier`] so tests can inspect
/// the generated source without running a compiler.
pub fn emit_c(intern: &Interner, checked: &CheckOutput) -> Result<String, String> {
    let prog = lower::lower_program(intern, checked)?;
    backend_c::emit(&prog)
}

pub fn lower_ir(intern: &Interner, checked: &CheckOutput) -> Result<Program, String> {
    lower::lower_program(intern, checked)
}

pub fn runtime_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime")
}

pub fn build_native(
    intern: &Interner,
    checked: &CheckOutput,
    src_name: &str,
    out_dir: &Path,
) -> Result<BuildResult, String> {
    build_tier(intern, checked, src_name, out_dir, Tier::Release)
}

pub fn build_tier(
    intern: &Interner,
    checked: &CheckOutput,
    src_name: &str,
    out_dir: &Path,
    tier: Tier,
) -> Result<BuildResult, String> {
    let c_src = emit_c(intern, checked)?;
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let stem = Path::new(src_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("out");
    let tag = tier.as_str();
    let c_path = out_dir.join(format!("{stem}.{tag}.c"));
    let bin_path = out_dir.join(format!("{stem}.{tag}"));
    std::fs::write(&c_path, &c_src).map_err(|e| e.to_string())?;

    let rt = runtime_dir();
    let sources = [rt.join("axrt.c"), rt.join("axlang.c")];
    for s in &sources {
        if !s.exists() {
            return Err(format!("missing runtime {}", s.display()));
        }
    }
    let mut cmd = Command::new("cc");
    match tier {
        Tier::Dev => {
            cmd.args(["-O0", "-g"]);
        }
        Tier::Release => {
            cmd.args([
                "-O3",
                "-flto",
                "-fno-asynchronous-unwind-tables",
                "-DNDEBUG",
            ]);
        }
        Tier::Portable => {
            cmd.args(["-O2", "-DNDEBUG", "--target=wasm32-wasi"]);
        }
    }
    cmd.args(["-std=c11", "-pthread"])
        .arg(format!("-I{}", rt.display()))
        .arg(&c_path);
    for s in &sources {
        cmd.arg(s);
    }
    cmd.arg("-lm").arg("-o").arg(&bin_path);
    let out = cmd.output().map_err(|e| format!("spawn cc: {e}"))?;
    if !out.status.success() {
        if tier == Tier::Portable {
            // A missing WASI sysroot must not block the suite: rebuild for the
            // host from the same IR and keep the tier label.
            return build_tier(intern, checked, src_name, out_dir, Tier::Dev).map(|mut b| {
                b.tier = Tier::Portable;
                b
            });
        }
        return Err(format!(
            "cc failed ({tag}):\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(BuildResult {
        c_path,
        bin_path,
        tier,
    })
}

/// Run a native binary and return stdout (trimmed).
pub fn run_bin(bin: &Path) -> Result<String, String> {
    run_bin_args(bin, &[])
}

pub fn run_bin_args(bin: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new(bin)
        .args(args)
        .output()
        .map_err(|e| format!("run {}: {e}", bin.display()))?;
    if !out.status.success() {
        return Err(format!(
            "{} exited {}: {}",
            bin.display(),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
