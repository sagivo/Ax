//! Development-only inverted Rust-ui harvest ([T-3.2.1]).
//!
//! Walk a pinned `rust-lang/rust` tree (or a directory of `.rs` files),
//! extract `//~ ERROR` / `E0xxx` annotations, and emit Ax sources that
//! *must compile* (never-reject). Translation failures that are `unsafe`
//! or macros are recorded, not skipped silently.

use crate::translate;
use std::path::{Path, PathBuf};

/// Error codes the inverted bucket starts from ([T-3.2.1]).
pub const INVERT_CODES: &[&str] = &[
    "E0382", "E0499", "E0502", "E0505", "E0506", "E0507", "E0515", "E0597", "E0716", "E0621",
    "E0623", "E0373", "E0521", "E0381", "E0596",
];

#[derive(Clone, Debug)]
pub struct HarvestHit {
    pub path: PathBuf,
    pub codes: Vec<String>,
    pub rust_src: String,
}

#[derive(Clone, Debug)]
pub struct HarvestReport {
    pub hits: Vec<HarvestHit>,
    pub written: Vec<PathBuf>,
    pub skipped_unsafe_or_macro: usize,
    pub skipped_other: Vec<String>,
}

/// Extract E0xxx codes from rustc ui annotations.
pub fn extract_codes(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        for code in INVERT_CODES {
            if line.contains(code) {
                if !out.iter().any(|c| c == *code) {
                    out.push((*code).to_string());
                }
            }
        }
        // `//~ ERROR cannot borrow` etc. without a code: classify by text.
        if line.contains("//~ ERROR") || line.contains("//~^ ERROR") || line.contains("//~| ERROR")
        {
            let lower = line.to_ascii_lowercase();
            let mapped = if lower.contains("moved") || lower.contains("use of moved") {
                Some("E0382")
            } else if lower.contains("mutable more than once") || lower.contains("as mutable more")
            {
                Some("E0499")
            } else if lower.contains("borrowed as immutable") || lower.contains("also borrowed") {
                Some("E0502")
            } else if lower.contains("move out of") && lower.contains("borrow") {
                Some("E0505")
            } else if lower.contains("cannot assign") && lower.contains("borrow") {
                Some("E0506")
            } else if lower.contains("cannot assign twice") {
                Some("E0384")
            } else if lower.contains("does not live long enough") {
                Some("E0597")
            } else if lower.contains("cannot return reference") {
                Some("E0515")
            } else if lower.contains("temporary") && lower.contains("dropped") {
                Some("E0716")
            } else if lower.contains("not declared as mutable") {
                Some("E0596")
            } else {
                None
            };
            if let Some(c) = mapped {
                if !out.iter().any(|x| x == c) {
                    out.push(c.to_string());
                }
            }
        }
    }
    out
}

pub fn should_invert(codes: &[String]) -> bool {
    codes
        .iter()
        .any(|c| INVERT_CODES.contains(&c.as_str()) || c == "E0384" || c == "E0596")
}

/// Walk `root` for `.rs` files (skips `.stderr`).
pub fn scan_tree(root: &Path) -> std::io::Result<Vec<HarvestHit>> {
    let mut out = Vec::new();
    walk(root, &mut out)?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<HarvestHit>) -> std::io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        if p.is_dir() {
            walk(&p, out)?;
        } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
            let src = std::fs::read_to_string(&p)?;
            let codes = extract_codes(&src);
            if should_invert(&codes) {
                out.push(HarvestHit {
                    path: p,
                    codes,
                    rust_src: src,
                });
            }
        }
    }
    Ok(())
}

/// Emit an inverted Ax file. `expect: compile` is the default: the original
/// Rust test never ran, so a value is not known ([T-3.2.3]).
pub fn emit_inverted(
    hit: &HarvestHit,
    dest_dir: &Path,
    id: &str,
    commit: &str,
) -> Result<PathBuf, String> {
    let stem = hit
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("case")
        .replace('-', "_");
    let rel = hit.path.to_string_lossy();
    let tr = translate::translate_rust(&hit.rust_src);
    if !tr.rejected.is_empty() {
        return Err(format!("rejected: {}", tr.rejected.join(", ")));
    }
    let codes = hit.codes.join(", ");
    let header = format!(
        "//@ id:        {id}\n\
         //@ requires:  R-1.2.2, R-3.3.1, R-1.3.1\n\
         //@ origin:    rust-lang/rust {rel}\n\
         //@ upstream:  {commit}\n\
         //@ license:   MIT OR Apache-2.0\n\
         //@ port:      inverted-mechanical\n\
         //@ expect:    compile\n\
         //@ diags:     A0101\n\
         // inverted from Rust {codes}: Ax must compile (never-reject).\n"
    );
    std::fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;
    let dest = dest_dir.join(format!("{stem}.ax"));
    let body = format!(
        "{header}module tests.rust_ported.inverted.{stem};\n{}",
        tr.source
    );
    std::fs::write(&dest, body).map_err(|e| e.to_string())?;
    Ok(dest)
}

pub fn harvest_into(rust_ui: &Path, dest: &Path, commit: &str) -> Result<HarvestReport, String> {
    let hits = scan_tree(rust_ui).map_err(|e| e.to_string())?;
    let mut written = Vec::new();
    let mut skipped_unsafe_or_macro = 0;
    let mut skipped_other = Vec::new();
    for (i, hit) in hits.iter().enumerate() {
        let id = format!("T-INV-H{:04}", i + 1);
        match emit_inverted(hit, dest, &id, commit) {
            Ok(p) => written.push(p),
            Err(e) => {
                if e.contains("unsafe") || e.contains("macro") {
                    skipped_unsafe_or_macro += 1;
                } else {
                    skipped_other.push(format!("{}: {e}", hit.path.display()));
                }
            }
        }
    }
    Ok(HarvestReport {
        hits,
        written,
        skipped_unsafe_or_macro,
        skipped_other,
    })
}
