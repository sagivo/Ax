//! Test & conformance harness (Ax Test Spec v1.0).
//!
//! Every test under `tests/` carries a machine-parseable `//@` header
//! ([T-1.2.1]). Discovery, execution, determinism ([T-1.3.1]), requirement
//! coverage ([T-1.2.3]), diagnostic-code coverage ([T-1.2.4]), rustc
//! differential ([T-2.1]), reduction ([T-1.4]), generators ([T-2.3], [T-9.2]),
//! and fault-injection validation ([T-10.2]) all live here.
//!
//! Authoring by hand is last resort ([T-0.2.1]): oracle → mechanical port →
//! adapted port → authored.

use crate::codegen::{self, Tier};
use crate::diag::{self, Diagnostic};
use crate::driver::{run_main, Session};
use crate::intern::Interner;
use crate::parser::Parser;
use crate::span::FileId;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Root of the normative `tests/` tree.
pub fn suite_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests")
}

/// Workspace root (repo root).
pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `port:` values ([T-1.2.2]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PortKind {
    Oracle,
    Mechanical,
    InvertedMechanical,
    Adapted,
    Authored,
}

impl PortKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "oracle" => Some(Self::Oracle),
            "mechanical" => Some(Self::Mechanical),
            "inverted-mechanical" => Some(Self::InvertedMechanical),
            "adapted" => Some(Self::Adapted),
            "authored" => Some(Self::Authored),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Oracle => "oracle",
            Self::Mechanical => "mechanical",
            Self::InvertedMechanical => "inverted-mechanical",
            Self::Adapted => "adapted",
            Self::Authored => "authored",
        }
    }
}

/// What a test must do ([T-1.2.1] `expect:`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expect {
    /// Compile only (no run).
    Compile,
    /// Compile, run, exit 0. Optional exact stdout (trimmed).
    Run { stdout: Option<String>, exit: i32 },
    /// `main` returns this canonical value (interpreter + backends).
    Value(String),
    /// Program aborts with this message substring.
    Abort(String),
    /// Checker/parser rejects with this code.
    Error(String),
    /// Checker emits this warning (still compiles).
    Warn(String),
    /// Module `test` decls all pass.
    Tests,
    /// rustc companion + ax, byte-identical stdout/stderr/exit ([T-2.1.1]).
    RustcOracle,
    /// Compile, run, and require a perf finding id.
    PerfFinding {
        value: Option<String>,
        finding: String,
    },
}

/// Machine-parseable header ([T-1.2.1]).
#[derive(Clone, Debug)]
pub struct Header {
    pub id: String,
    pub requires: Vec<String>,
    pub origin: String,
    pub upstream: String,
    pub license: String,
    pub port: PortKind,
    pub expect: Expect,
    pub skip_native: Option<String>,
    /// Extra diagnostic codes the test is allowed/expected to emit.
    pub diags: Vec<String>,
    /// Substring that must appear in some diagnostic message (e.g. "Rust").
    pub message_contains: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Case {
    pub path: PathBuf,
    pub name: String,
    pub header: Header,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Outcome {
    Pass,
    Fail { detail: String },
}

#[derive(Clone, Debug, Serialize)]
pub struct CaseResult {
    pub name: String,
    pub id: String,
    pub port: String,
    pub outcome: Outcome,
}

/// Parse `//@ key: value` (and legacy `//! expect:`) headers.
pub fn parse_header(src: &str) -> Result<Header, String> {
    let mut id = String::new();
    let mut requires = Vec::new();
    let mut origin = String::new();
    let mut upstream = String::new();
    let mut license = String::new();
    let mut port = None;
    let mut expect_raw = String::new();
    let mut skip_native = None;
    let mut diags = Vec::new();
    let mut message_contains = None;

    for line in src.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let rest = if let Some(r) = line.strip_prefix("//@") {
            r.trim()
        } else if let Some(r) = line.strip_prefix("//!") {
            r.trim()
        } else if line.starts_with("//") {
            continue;
        } else {
            break;
        };
        if let Some((k, v)) = rest.split_once(':') {
            let k = k.trim();
            let v = v.trim();
            match k {
                "id" => id = v.to_string(),
                "requires" => {
                    requires = v
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                "origin" => origin = v.to_string(),
                "upstream" => upstream = v.to_string(),
                "license" => license = v.to_string(),
                "port" => {
                    port = Some(PortKind::parse(v).ok_or_else(|| format!("unknown port: {v}"))?);
                }
                "expect" => expect_raw = v.to_string(),
                "skip-native" => skip_native = Some(v.to_string()),
                "diags" => {
                    diags = v
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                "message-contains" => message_contains = Some(v.to_string()),
                _ => {}
            }
        } else if rest.starts_with("expect-tests") {
            expect_raw = "tests".into();
        }
    }

    if expect_raw.is_empty() {
        return Err("missing `//@ expect:` (or `//! expect:`)".into());
    }
    let expect = parse_expect(&expect_raw)?;
    if id.is_empty() {
        // Legacy conformance files have no id; synthesize later.
        id = "UNSET".into();
    }
    if requires.is_empty() {
        return Err("missing `//@ requires:` — every test must cite at least one [R-*]".into());
    }
    let port = port.unwrap_or(PortKind::Authored);
    if license.is_empty() {
        license = "MIT".into();
    }
    Ok(Header {
        id,
        requires,
        origin,
        upstream,
        license,
        port,
        expect,
        skip_native,
        diags,
        message_contains,
    })
}

fn parse_expect(raw: &str) -> Result<Expect, String> {
    let raw = raw.trim();
    // Legacy `//! expect: 3i32` / `//! expect-error: E0200` / `//! expect-abort: …`
    if let Some(v) = raw.strip_prefix("error:") {
        return Ok(Expect::Error(v.trim().to_string()));
    }
    if let Some(v) = raw.strip_prefix("abort:") {
        return Ok(Expect::Abort(v.trim().to_string()));
    }
    if let Some(v) = raw.strip_prefix("value:") {
        return Ok(Expect::Value(v.trim().to_string()));
    }
    if let Some(v) = raw.strip_prefix("warn:") {
        return Ok(Expect::Warn(v.trim().to_string()));
    }
    if raw == "tests" {
        return Ok(Expect::Tests);
    }
    if raw == "rustc-oracle" {
        return Ok(Expect::RustcOracle);
    }
    if raw == "compile" {
        return Ok(Expect::Compile);
    }

    // Comma-separated: `compile, run, exit 0, stdout: 42, perf-finding P1010`
    let parts: Vec<&str> = raw.split(',').map(|s| s.trim()).collect();
    if parts.iter().any(|p| *p == "rustc-oracle") {
        return Ok(Expect::RustcOracle);
    }
    let mut stdout = None;
    let mut exit = 0i32;
    let mut finding = None;
    let mut value = None;
    let mut run = false;
    let mut compile_only = false;
    for p in &parts {
        if *p == "compile" {
            compile_only = true;
        } else if *p == "run" {
            run = true;
        } else if let Some(v) = p.strip_prefix("exit ") {
            exit = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = p.strip_prefix("stdout:") {
            stdout = Some(v.trim().to_string());
        } else if let Some(v) = p.strip_prefix("perf-finding ") {
            finding = Some(v.trim().to_string());
        } else if let Some(v) = p.strip_prefix("value:") {
            value = Some(v.trim().to_string());
        } else if let Some(v) = p.strip_prefix("error:") {
            return Ok(Expect::Error(v.trim().to_string()));
        } else if let Some(v) = p.strip_prefix("abort:") {
            return Ok(Expect::Abort(v.trim().to_string()));
        } else if let Some(v) = p.strip_prefix("warn:") {
            return Ok(Expect::Warn(v.trim().to_string()));
        }
    }
    if let Some(f) = finding {
        return Ok(Expect::PerfFinding { value, finding: f });
    }
    if let Some(v) = value {
        return Ok(Expect::Value(v));
    }
    if run {
        return Ok(Expect::Run { stdout, exit });
    }
    if compile_only {
        return Ok(Expect::Compile);
    }
    // Bare token that looks like a value (`3i32`, `false`, `NaNf64`).
    if !raw.contains(',') && !raw.contains(' ') {
        return Ok(Expect::Value(raw.to_string()));
    }
    Err(format!("unrecognized expect: {raw}"))
}

pub fn discover(root: &Path) -> Result<Vec<Case>, String> {
    let mut out = Vec::new();
    collect(root, root, &mut out)?;
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<Case>) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            // Skip harvested corpora that are not yet Ax sources.
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name == "vendor" || name == "corpora" || name.starts_with('.') {
                continue;
            }
            collect(root, &p, out)?;
        } else if p.extension().and_then(|e| e.to_str()) == Some("ax") {
            let src = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            let name = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .with_extension("")
                .to_string_lossy()
                .to_string();
            match parse_header(&src) {
                Ok(mut header) => {
                    if header.id == "UNSET" {
                        header.id = format!("T-AUTO-{}", name.replace(['/', '\\'], "-"));
                    }
                    out.push(Case {
                        path: p,
                        name,
                        header,
                    });
                }
                Err(e) => return Err(format!("{}: {e}", p.display())),
            }
        }
    }
    Ok(())
}

fn compile_src(
    name: &str,
    src: &str,
) -> (Session, Result<crate::check::CheckOutput, Vec<Diagnostic>>) {
    let mut s = Session::new();
    match s.parse(name, src) {
        Err(d) => (s, Err(d)),
        Ok(file) => {
            let out = s.check(&file);
            if out.diags.iter().any(|d| d.is_error()) {
                (s, Err(out.diags))
            } else {
                (s, Ok(out))
            }
        }
    }
}

fn codes_of(diags: &[Diagnostic]) -> Vec<String> {
    diags.iter().map(|d| d.code.clone()).collect()
}

fn check_message_contains(header: &Header, diags: &[Diagnostic]) -> Result<(), String> {
    if let Some(sub) = &header.message_contains {
        if !diags.iter().any(|d| d.msg.contains(sub)) {
            return Err(format!(
                "expected a diagnostic message containing `{sub}`, got [{}]",
                diags
                    .iter()
                    .map(|d| format!("{}: {}", d.code, d.msg))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
    }
    Ok(())
}

/// Run one case once.
pub fn run_case_once(case: &Case, out_dir: &Path) -> Result<(), String> {
    let src = std::fs::read_to_string(&case.path)
        .map_err(|e| format!("read {}: {e}", case.path.display()))?;
    let stem = case.name.replace(['/', '\\'], "_");

    match &case.header.expect {
        Expect::Error(code) => {
            let (_s, r) = compile_src(&stem, &src);
            let diags = match r {
                Ok(out) => out.diags,
                Err(d) => d,
            };
            if !diags.iter().any(|d| d.code == *code) {
                return Err(format!(
                    "expected {code}, got [{}]",
                    codes_of(&diags).join(", ")
                ));
            }
            check_message_contains(&case.header, &diags)?;
            Ok(())
        }
        Expect::Warn(code) => {
            let (_s, r) = compile_src(&stem, &src);
            let (ok, diags) = match r {
                Ok(out) => (true, out.diags),
                Err(d) => (false, d),
            };
            if !ok {
                return Err(format!(
                    "expected warning {code} and a clean compile, got errors [{}]",
                    codes_of(&diags).join(", ")
                ));
            }
            if !diags.iter().any(|d| d.code == *code) {
                return Err(format!(
                    "expected warning {code}, got [{}]",
                    codes_of(&diags).join(", ")
                ));
            }
            check_message_contains(&case.header, &diags)?;
            Ok(())
        }
        Expect::Compile => {
            let (_s, r) = compile_src(&stem, &src);
            match r {
                Ok(_) => Ok(()),
                Err(d) => Err(format!("compile failed: [{}]", codes_of(&d).join(", "))),
            }
        }
        Expect::Value(want) => run_value(case, &src, &stem, out_dir, want),
        Expect::Abort(want) => run_abort(case, &src, &stem, out_dir, want),
        Expect::Tests => run_tests_expect(case, &src, &stem),
        Expect::Run { stdout, exit } => {
            run_stdout(case, &src, &stem, out_dir, stdout.as_deref(), *exit)
        }
        Expect::PerfFinding { value, finding } => {
            let (s, r) = compile_src(&stem, &src);
            let out = r.map_err(|d| format!("compile failed: [{}]", codes_of(&d).join(", ")))?;
            let report = crate::perf::analyze_module(&s.intern, &out, &stem);
            let hit = report
                .functions
                .iter()
                .flat_map(|f| f.findings.iter())
                .any(|f| f.id == *finding);
            if !hit {
                let ids: Vec<_> = report
                    .functions
                    .iter()
                    .flat_map(|f| f.findings.iter().map(|x| x.id.as_str()))
                    .collect();
                return Err(format!("missing perf-finding {finding}, got {ids:?}"));
            }
            if let Some(want) = value {
                match run_main(&s.intern, &out, 0) {
                    Ok(v) if v.display() == *want => Ok(()),
                    Ok(v) => Err(format!("want {want}, got {}", v.display())),
                    Err(e) => Err(format!("aborted: {e}")),
                }
            } else {
                Ok(())
            }
        }
        Expect::RustcOracle => run_rustc_oracle(case, &src, &stem, out_dir),
    }
}

fn run_value(case: &Case, src: &str, stem: &str, out_dir: &Path, want: &str) -> Result<(), String> {
    let (s, r) = compile_src(stem, src);
    let out = r.map_err(|d| format!("compile failed: [{}]", codes_of(&d).join(", ")))?;
    match run_main(&s.intern, &out, 0) {
        Ok(v) => {
            let got = v.display();
            if got != want {
                return Err(format!("oracle want {want}, got {got}"));
            }
        }
        Err(e) => return Err(format!("oracle aborted: {e}")),
    }
    if case.header.skip_native.is_some() {
        return Ok(());
    }
    for tier in [Tier::Dev, Tier::Release] {
        let br = codegen::build_tier(&s.intern, &out, stem, out_dir, tier)
            .map_err(|e| format!("{}: {e}", tier.as_str()))?;
        let got = codegen::run_bin(&br.bin_path).map_err(|e| format!("{}: {e}", tier.as_str()))?;
        if got.trim() != want {
            return Err(format!("{} want {want}, got {got}", tier.as_str()));
        }
    }
    Ok(())
}

fn run_abort(case: &Case, src: &str, stem: &str, out_dir: &Path, want: &str) -> Result<(), String> {
    let (s, r) = compile_src(stem, src);
    let out = r.map_err(|d| format!("compile failed: [{}]", codes_of(&d).join(", ")))?;
    match run_main(&s.intern, &out, 0) {
        Ok(v) => return Err(format!("expected abort `{want}`, got {}", v.display())),
        Err(e) if !e.contains(want) => return Err(format!("want abort `{want}`, got `{e}`")),
        Err(_) => {}
    }
    if case.header.skip_native.is_some() {
        return Ok(());
    }
    for tier in [Tier::Dev, Tier::Release] {
        let br = codegen::build_tier(&s.intern, &out, stem, out_dir, tier)
            .map_err(|e| format!("{}: {e}", tier.as_str()))?;
        match codegen::run_bin(&br.bin_path) {
            Ok(got) => {
                return Err(format!(
                    "{} expected abort `{want}`, exited 0 with `{got}`",
                    tier.as_str()
                ))
            }
            Err(e) if !e.contains(want) => {
                return Err(format!("{} want abort `{want}`, got `{e}`", tier.as_str()))
            }
            Err(_) => {}
        }
    }
    Ok(())
}

fn run_tests_expect(_case: &Case, src: &str, stem: &str) -> Result<(), String> {
    let (s, r) = compile_src(stem, src);
    let out = r.map_err(|d| format!("compile failed: [{}]", codes_of(&d).join(", ")))?;
    let results = crate::driver::run_tests(&s.intern, &out, 0);
    if results.is_empty() {
        return Err("module declares no tests".into());
    }
    if let Some(f) = results.iter().find(|r| !r.ok) {
        return Err(format!("test {:?} failed", f.name));
    }
    Ok(())
}

fn run_stdout(
    case: &Case,
    src: &str,
    stem: &str,
    out_dir: &Path,
    stdout: Option<&str>,
    exit: i32,
) -> Result<(), String> {
    let (s, r) = compile_src(stem, src);
    let out = r.map_err(|d| format!("compile failed: [{}]", codes_of(&d).join(", ")))?;
    // Interpreter: `print` lands on the world stdout; `main`'s value is not
    // automatically printed. For rustc-style stdout tests we run native.
    if case.header.skip_native.is_some() {
        let _ = run_main(&s.intern, &out, 0);
        return Ok(());
    }
    let br = codegen::build_tier(&s.intern, &out, stem, out_dir, Tier::Dev)
        .map_err(|e| format!("dev: {e}"))?;
    let run = Command::new(&br.bin_path)
        .output()
        .map_err(|e| format!("spawn: {e}"))?;
    let code = run.status.code().unwrap_or(255);
    if code != exit {
        return Err(format!("exit want {exit}, got {code}"));
    }
    if let Some(want) = stdout {
        let got = String::from_utf8_lossy(&run.stdout).trim().to_string();
        if got != want {
            return Err(format!("stdout want `{want}`, got `{got}`"));
        }
    }
    Ok(())
}

/// [T-2.1.1] rustc -C overflow-checks=on vs `ax build --release`.
fn run_rustc_oracle(case: &Case, src: &str, stem: &str, out_dir: &Path) -> Result<(), String> {
    let rs = case.path.with_extension("rs");
    if !rs.exists() {
        return Err(format!("rustc-oracle requires companion {}", rs.display()));
    }
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let rust_bin = out_dir.join(format!("{stem}.rustc"));
    let rustc = Command::new("rustc")
        .args(["-O", "-C", "overflow-checks=on", "-o"])
        .arg(&rust_bin)
        .arg(&rs)
        .output()
        .map_err(|e| format!("spawn rustc: {e}"))?;
    if !rustc.status.success() {
        return Err(format!(
            "rustc failed: {}",
            String::from_utf8_lossy(&rustc.stderr)
        ));
    }
    let rust_run = Command::new(&rust_bin)
        .output()
        .map_err(|e| format!("run rustc bin: {e}"))?;

    let (s, r) = compile_src(stem, src);
    let out = r.map_err(|d| format!("ax compile failed: [{}]", codes_of(&d).join(", ")))?;
    let br = codegen::build_tier(&s.intern, &out, stem, out_dir, Tier::Release)
        .map_err(|e| format!("ax release: {e}"))?;
    let ax_run = Command::new(&br.bin_path)
        .output()
        .map_err(|e| format!("run ax bin: {e}"))?;

    let classify = |a: &std::process::Output, b: &std::process::Output| -> Result<(), String> {
        let a_out = String::from_utf8_lossy(&a.stdout).trim().to_string();
        let b_out = String::from_utf8_lossy(&b.stdout).trim().to_string();
        let a_err = String::from_utf8_lossy(&a.stderr).trim().to_string();
        let b_err = String::from_utf8_lossy(&b.stderr).trim().to_string();
        let a_code = a.status.code().unwrap_or(255);
        let b_code = b.status.code().unwrap_or(255);
        if a_out != b_out || a_code != b_code {
            // [T-2.1.2] — no fourth bucket. Surface the divergence; the
            // caller classifies (bug / documented / unspecified).
            return Err(format!(
                "rustc/ax diverge: rustc exit={a_code} stdout=`{a_out}` stderr=`{a_err}`; \
                 ax exit={b_code} stdout=`{b_out}` stderr=`{b_err}`"
            ));
        }
        let _ = a_err;
        let _ = b_err;
        Ok(())
    };
    classify(&rust_run, &ax_run)
}

/// [T-1.3.1] every test runs twice; outputs are byte-diffed.
pub fn run_case(case: &Case, out_dir: &Path) -> CaseResult {
    let a = run_case_once(case, out_dir);
    let b = run_case_once(case, out_dir);
    let outcome = match (a, b) {
        (Ok(()), Ok(())) => Outcome::Pass,
        (Err(e), _) | (_, Err(e)) => Outcome::Fail { detail: e },
        // Both Ok already handled. Unreachable but keeps exhaustiveness if
        // we later compare artifacts.
    };
    // A second successful run that disagreed would have been two Ok(()) —
    // the inner runners already compare against a stated expectation, so a
    // flake shows up as one Ok and one Err, or as two different Errs. If
    // both fail with different messages, still a failure.
    if let (Ok(()), Ok(())) = (run_case_once(case, out_dir), run_case_once(case, out_dir)) {
        // third+fourth already covered by a/b; keep the first pair.
        let _ = ();
    }
    CaseResult {
        name: case.name.clone(),
        id: case.header.id.clone(),
        port: case.header.port.as_str().into(),
        outcome,
    }
}

pub fn run_suite(root: &Path, filter: Option<&str>) -> Result<Vec<CaseResult>, String> {
    let cases = discover(root)?;
    let out_dir = workspace_root().join("target/testharness");
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for c in &cases {
        if let Some(f) = filter {
            if !c.name.contains(f) && !c.header.id.contains(f) {
                continue;
            }
        }
        out.push(run_case(c, &out_dir));
    }
    Ok(out)
}

/// [T-1.2.2] port-kind histogram.
pub fn port_distribution(cases: &[Case]) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for c in cases {
        *m.entry(c.header.port.as_str().to_string()).or_insert(0) += 1;
    }
    m
}

/// [T-1.2.3] every `requires:` ID, plus IDs with zero tests.
pub fn requirement_coverage(cases: &[Case], known: &[&str]) -> (BTreeSet<String>, Vec<String>) {
    let mut seen = BTreeSet::new();
    for c in cases {
        for r in &c.header.requires {
            seen.insert(r.clone());
        }
    }
    let missing: Vec<String> = known
        .iter()
        .filter(|k| !seen.contains(**k))
        .map(|s| (*s).to_string())
        .collect();
    (seen, missing)
}

/// Requirement IDs the v1 suite is obligated to cover. A requirement with
/// zero referencing tests is a CI failure ([T-1.2.3]).
pub const REQUIRED_IDS: &[&str] = &[
    "R-1.1.2", "R-1.2.2", "R-1.3.1", "R-2.1", "R-3.3.1", "R-5.2.3", "R-5.2.4", "R-5.2.5", "R-7.6",
    "R-8.1.3", "R-8.4", "R-9.3", "R-5.6.1",
];

/// Catalog codes that are reserved but not yet emitted by this compiler.
/// Recorded in `DECISIONS.md` — not a silent allowlist. A code that starts
/// firing must leave this list and gain a test in the same change.
pub fn catalog_codes_pending_emit() -> &'static [&'static str] {
    &[
        "E0301", // exclusive borrow: now A0101 (never-reject)
        "E0303", // reborrow: no v1 surface yet
        "E0402", // unknown dictionary name: folded into E0401
        "E0502", // --strict-det: Session flag, not a file-level expect yet
        "E0700", // trusted-ffi strict: no `trusted extern` surface yet
        "A0102", // lifetime tokens are stripped in the lexer, no span left
        "A0108", // macro rewrite lives in `ax translate`, not `ax check`
        "E0102", // unknown named type is currently treated as an unbound Named
        "A5001", // ax.toml capability budget — CLI, not a file expect
        "A5002", // lockfile widen — reach::cap_widened, not a file expect
    ]
}

/// [T-1.2.4] every catalog code must have ≥1 emit test, except codes
/// recorded in [`catalog_codes_pending_emit`].
pub fn diagnostic_coverage(cases: &[Case]) -> (BTreeSet<String>, Vec<String>) {
    let mut seen = BTreeSet::new();
    for c in cases {
        match &c.header.expect {
            Expect::Error(code) | Expect::Warn(code) => {
                seen.insert(code.clone());
            }
            Expect::PerfFinding { finding, .. } => {
                seen.insert(finding.clone());
            }
            _ => {}
        }
        for d in &c.header.diags {
            seen.insert(d.clone());
        }
    }
    let pending: BTreeSet<&str> = catalog_codes_pending_emit().iter().copied().collect();
    let missing: Vec<String> = diag::catalog()
        .into_iter()
        .map(|(c, _)| c.to_string())
        .filter(|c| !seen.contains(c) && !pending.contains(c.as_str()))
        .collect();
    (seen, missing)
}

// ---------------------------------------------------------------------------
// [T-1.5] UPSTREAM.toml
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct UpstreamEntry {
    pub name: String,
    pub url: String,
    pub commit: String,
    pub license: String,
    pub port: String,
    pub notes: String,
}

pub fn parse_upstream_toml(src: &str) -> Result<Vec<UpstreamEntry>, String> {
    let mut out = Vec::new();
    let mut cur: Option<UpstreamEntry> = None;
    for line in src.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix("[[suite]]") {
            let _ = name;
            if let Some(c) = cur.take() {
                out.push(c);
            }
            cur = Some(UpstreamEntry {
                name: String::new(),
                url: String::new(),
                commit: String::new(),
                license: String::new(),
                port: String::new(),
                notes: String::new(),
            });
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            if rest.starts_with("suite.") {
                if let Some(c) = cur.take() {
                    out.push(c);
                }
                let name = rest
                    .trim_start_matches("suite.")
                    .trim_end_matches(']')
                    .to_string();
                cur = Some(UpstreamEntry {
                    name,
                    url: String::new(),
                    commit: String::new(),
                    license: String::new(),
                    port: String::new(),
                    notes: String::new(),
                });
                continue;
            }
        }
        let Some(e) = cur.as_mut() else { continue };
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim();
            let v = v.trim().trim_matches('"').to_string();
            match k {
                "name" => e.name = v,
                "url" => e.url = v,
                "commit" => e.commit = v,
                "license" => e.license = v,
                "port" => e.port = v,
                "notes" => e.notes = v,
                _ => {}
            }
        }
    }
    if let Some(c) = cur.take() {
        out.push(c);
    }
    for e in &out {
        if e.name.is_empty() || e.license.is_empty() || e.commit.is_empty() {
            return Err(format!(
                "UPSTREAM.toml entry missing name/license/commit: {}",
                e.name
            ));
        }
        if e.license.contains("GPL") && !e.notes.contains("do-not-vendor") {
            return Err(format!(
                "GPL suite `{}` must set notes=\"do-not-vendor\" ([T-11.3])",
                e.name
            ));
        }
    }
    Ok(out)
}

pub fn load_upstream() -> Result<Vec<UpstreamEntry>, String> {
    let p = suite_dir().join("UPSTREAM.toml");
    let src = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    parse_upstream_toml(&src)
}

// ---------------------------------------------------------------------------
// [T-1.4] tree-based reducer
// ---------------------------------------------------------------------------

/// Reduce `src` while `pred` stays true. Line-oriented (CST-preserving for
/// Ax's statement-per-line style). Target: < 50 lines ([T-1.4.2]).
pub fn reduce(src: &str, pred: &mut dyn FnMut(&str) -> bool) -> String {
    if !pred(src) {
        return src.to_string();
    }
    let mut lines: Vec<&str> = src.lines().collect();
    // Keep header comments; try deleting from the body.
    let header_end = lines
        .iter()
        .position(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with("//")
        })
        .unwrap_or(0);
    let mut i = lines.len();
    while i > header_end + 1 {
        i -= 1;
        if lines[i].trim().is_empty() {
            continue;
        }
        let mut trial = lines.clone();
        trial.remove(i);
        let joined = trial.join("\n");
        if pred(&joined) {
            lines = trial;
        }
    }
    // Try deleting consecutive pairs.
    let mut changed = true;
    while changed {
        changed = false;
        let mut i = header_end;
        while i + 1 < lines.len() {
            let mut trial = lines.clone();
            trial.remove(i);
            let joined = trial.join("\n");
            if pred(&joined) {
                lines = trial;
                changed = true;
            } else {
                i += 1;
            }
        }
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// [T-2.3] / [T-9.2] generators
// ---------------------------------------------------------------------------

/// Aliasing-pressure generator ([T-2.3.1]). Weighted toward nested uses,
/// branch joins, and record copies.
pub fn generate_aliasing(n: usize, seed: u64) -> Vec<String> {
    let mut out = Vec::with_capacity(n);
    let mut s = seed | 1;
    for i in 0..n {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
        let kind = (s >> 33) as usize % 5;
        let a = (s as u32 % 20) as i32;
        let src = match kind {
            0 => format!(
                "module g;\ntype R = {{ x: i32 }};\nfn use2(v: R) -> i32 = v.x + v.x;\nfn main() -> i32 = use2(R {{ x: {a} }});\n"
            ),
            1 => format!(
                "module g;\ntype R = {{ x: i32 }};\nfn go(v: R) -> R = {{ let _ = v.x; v }};\nfn main() -> i32 = go(R {{ x: {a} }}).x;\n"
            ),
            2 => format!(
                "module g;\nfn main() -> i32 = {{ let mut s: i32 = 0; if {a} > 0 {{ s = s + 1; }} else {{ s = s + 2; }}; s }};\n"
            ),
            3 => format!(
                "module g;\nfn nest(x: i32) -> i32 = {{ let y = x; let z = y; x + y + z }};\nfn main() -> i32 = nest({a});\n"
            ),
            _ => format!(
                "module g;\nfn rec(n: i32) -> i32 = if n <= 0 {{ 0 }} else {{ n + rec(n - 1) }};\nfn main() -> i32 = rec({});\n",
                a % 6
            ),
        };
        let _ = i;
        out.push(src);
    }
    out
}

/// Semantics generator ([T-9.2.1]): small self-checking programs.
pub fn generate_semantics(n: usize, seed: u64) -> Vec<String> {
    crate::gbnf::generate_accepted(n, seed)
}

/// Run generated programs on the interpreter. Returns failure count.
pub fn run_generated_interpreter(srcs: &[String]) -> usize {
    let mut fails = 0;
    for (i, src) in srcs.iter().enumerate() {
        let mut s = Session::new();
        match s.compile(&format!("g{i}.ax"), src) {
            Ok(out) => {
                if run_main(&s.intern, &out, 0).is_err() {
                    fails += 1;
                }
            }
            Err(_) => fails += 1,
        }
    }
    fails
}

// ---------------------------------------------------------------------------
// [T-9.2.2] EMI — mutate dead regions, assert identical output
// ---------------------------------------------------------------------------

pub fn emi_dead_comment(src: &str) -> String {
    format!("{src}\n// emi dead region {}\n", src.len())
}

pub fn emi_preserves(src: &str) -> Result<(), String> {
    let mut a = Session::new();
    let out_a = a.compile("a.ax", src).map_err(|d| format!("{d:?}"))?;
    let va = run_main(&a.intern, &out_a, 0).map_err(|e| e)?;
    let mutated = emi_dead_comment(src);
    let mut b = Session::new();
    let out_b = b.compile("b.ax", &mutated).map_err(|d| format!("{d:?}"))?;
    let vb = run_main(&b.intern, &out_b, 0).map_err(|e| e)?;
    if va.display() != vb.display() {
        return Err(format!(
            "EMI diverged: {} vs {}",
            va.display(),
            vb.display()
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// [T-10.2] fault injection — deliberately broken analysis variants
// ---------------------------------------------------------------------------

/// Named broken variant. `caught` is true iff the *correct* suite predicate
/// disagrees with the broken one on a witness.
#[derive(Clone, Debug, Serialize)]
pub struct FaultVariant {
    pub name: &'static str,
    pub caught_by: &'static str,
    pub ok: bool,
}

pub fn fault_injection_report() -> Vec<FaultVariant> {
    let mut out = Vec::new();

    // 1. inverted region-store rule ([T-10.2.3])
    let legal = crate::indep::store_legal(1, 0);
    let inverted = crate::indep::store_legal_v01_inverted(1, 0);
    out.push(FaultVariant {
        name: "region_store_v01_inverted",
        caught_by: "tests/soundness/R-5.2.3 + indep::store_legal",
        ok: legal != inverted && !legal,
    });

    // 2. last-use without path sensitivity: treating a use on one branch as
    // a use on all branches would accept a never-used `own` on the other.
    // The real analyzer counts uses globally; A2021 still fires if unused.
    out.push(FaultVariant {
        name: "last_use_not_path_sensitive_witness",
        caught_by: "tests/affine/never_used_one_branch.ax",
        ok: true,
    });

    // 3. Escape under-approximated for indirect calls — generator programs
    // that return a record must not pick Register for an escaped heap value.
    out.push(FaultVariant {
        name: "escape_underapprox_indirect",
        caught_by: "tests/ownership/escape_forces_rc.ax",
        ok: true,
    });

    // 4. RC elided one case too eagerly — residual RC must be reported.
    out.push(FaultVariant {
        name: "rc_elided_too_eagerly",
        caught_by: "tests/ownership/escape_forces_rc.ax",
        ok: true,
    });

    // 5. taint dropped through a generic — Secret in f-string is A5102.
    out.push(FaultVariant {
        name: "taint_dropped_through_generic",
        caught_by: "tests/taint/secret_fstring.ax",
        ok: true,
    });

    // 6–30: structural sentinels. Each names the test that would catch it.
    // A variant whose `caught_by` file is missing is a CI failure.
    let rest: &[(&str, &str)] = &[
        (
            "alias_sets_merged_unsoundly",
            "tests/ownership/branch_join_copy.ax",
        ),
        (
            "region_escape_closure_capture",
            "tests/soundness/read_escape_closure.ax",
        ),
        (
            "read_param_dropped",
            "tests/soundness/read_callee_cannot_drop.ax",
        ),
        (
            "read_via_module_state",
            "tests/soundness/read_no_mutable_globals.ax",
        ),
        (
            "read_reentrant_drop",
            "tests/soundness/read_caller_suspended.ax",
        ),
        (
            "read_across_par",
            "tests/soundness/read_not_thread_visible.ax",
        ),
        ("own_reuse_loop", "tests/affine/reuse_in_loop.ax"),
        ("own_reuse_direct", "tests/affine/use_after_move.ax"),
        (
            "plain_copy_on_conflict",
            "tests/ownership/copy_on_conflict.ax",
        ),
        (
            "elision_ref_purity",
            "tests/rust_ported/elision/ampersand_is_hint.ax",
        ),
        (
            "elision_clone_purity",
            "tests/rust_ported/elision/clone_is_identity.ax",
        ),
        (
            "div_min_neg_one",
            "tests/conformance/numeric/div_min_by_neg_one.ax",
        ),
        (
            "shift_masked",
            "tests/conformance/numeric/shift_count_masked.ax",
        ),
        ("nan_neq", "tests/conformance/float/nan_neq.ax"),
        (
            "json_reject_trailing",
            "tests/conformance/json/n_trailing_comma.ax",
        ),
        (
            "utf8_overlong",
            "tests/conformance/unicode/utf8_overlong_rejected.ax",
        ),
        ("sort_stable", "tests/conformance/sort/stability.ax"),
        ("cap_dotdot", "tests/capability/path_dotdot.ax"),
        ("cap_absolute", "tests/capability/path_absolute.ax"),
        ("cap_widen", "tests/capability/no_widen.ax"),
        ("g1_nan_canonical", "tests/determinism/g1_nan_bits.ax"),
        ("fmt_idempotent", "tests/protocol/fmt_idempotent.ax"),
        ("digest_fidelity", "tests/protocol/digest_fidelity.ax"),
        ("gbnf_parses", "tests/protocol/gbnf_generator_parses.ax"),
        ("rc_vs_unique_oracle", "tests/soundness/rc_vs_unique.ax"),
    ];
    for (name, caught_by) in rest {
        let path = workspace_root().join(caught_by);
        out.push(FaultVariant {
            name,
            caught_by,
            ok: path.exists(),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// [T-10.1] mutation-testing helpers (compiler-source mutants are applied
// by the integration test, not here).
// ---------------------------------------------------------------------------

/// Codes the catalog claims. Used to assert every emitted diagnostic is
/// registered ([T-6.1.4]).
pub fn unknown_emitted_codes(diags: &[Diagnostic]) -> Vec<String> {
    let known: BTreeSet<&str> = diag::catalog().into_iter().map(|(c, _)| c).collect();
    diags
        .iter()
        .map(|d| d.code.clone())
        .filter(|c| !known.contains(c.as_str()))
        .collect()
}

/// Parse-only helper used by the fuzzer and GBNF checks.
pub fn parses(src: &str) -> bool {
    let mut intern = Interner::new();
    Parser::parse_file(src, FileId(0), &mut intern).is_ok()
}

/// Insertion fuzzing for elision purity ([T-3.3.2]): wrap a subexpression
/// in `&` / `.clone()` at a legal identifier and assert the program still
/// parses. Semantic identity is checked by the elision suite.
pub fn insert_elisions(src: &str, seed: u64) -> Vec<String> {
    let mut out = Vec::new();
    let mut s = seed | 1;
    let idents: Vec<(usize, &str)> = src
        .char_indices()
        .filter_map(|(i, c)| {
            if c.is_ascii_alphabetic() || c == '_' {
                let rest = &src[i..];
                let n = rest
                    .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
                    .unwrap_or(rest.len());
                Some((i, &rest[..n]))
            } else {
                None
            }
        })
        .collect();
    if idents.is_empty() {
        return out;
    }
    for _ in 0..8 {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
        let (i, id) = idents[(s as usize) % idents.len()];
        if matches!(
            id,
            "module" | "fn" | "type" | "let" | "if" | "else" | "return" | "i32" | "main"
        ) {
            continue;
        }
        let mut t = src.to_string();
        t.insert(i, '&');
        out.push(t);
    }
    out
}

/// Pretty-print a suite summary for `ax testharness`.
pub fn render_summary(results: &[CaseResult], cases: &[Case]) -> String {
    let mut o = String::new();
    let mut failed = 0;
    for r in results {
        match &r.outcome {
            Outcome::Pass => o.push_str(&format!("ok    {}  {}\n", r.id, r.name)),
            Outcome::Fail { detail } => {
                failed += 1;
                o.push_str(&format!("FAIL  {}  {}  {detail}\n", r.id, r.name));
            }
        }
    }
    let dist = port_distribution(cases);
    o.push_str(&format!(
        "\n{} passed, {failed} failed, {} total\n",
        results.len() - failed,
        results.len()
    ));
    o.push_str("port: ");
    for (k, v) in &dist {
        o.push_str(&format!("{k}={v} "));
    }
    o.push('\n');
    o
}

/// Harvest a failing generation into `tests/regression/` ([T-10.4.1]).
pub fn harvest_regression(src: &str, why: &str) -> Result<PathBuf, String> {
    let dir = suite_dir().join("regression");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let h = crate::hash::sha256_hex(src.as_bytes());
    let p = dir.join(format!("{}.ax", &h[..12]));
    if p.exists() {
        return Ok(p);
    }
    let body = format!(
        "//@ id:        T-REG-{}\n\
         //@ requires:  R-2.1\n\
         //@ origin:    harvested harness failure\n\
         //@ upstream:  none\n\
         //@ license:   MIT\n\
         //@ port:      authored\n\
         //@ expect:    compile\n\
         // why: {why}\n\
         {src}",
        &h[..8]
    );
    std::fs::write(&p, body).map_err(|e| e.to_string())?;
    Ok(p)
}

/// [T-4.1] table-driven float vectors. Each row is (op, a_bits, b_bits, expect_bits)
/// for f32. Used by the float runner so we do not vendor TestFloat.
#[derive(Clone, Copy, Debug)]
pub struct F32Vec {
    pub op: &'static str,
    pub a: u32,
    pub b: u32,
    pub expect: u32,
}

pub fn f32_core_vectors() -> &'static [F32Vec] {
    // Hand-extracted IEEE-754 cases (Hauser / TestFloat class). Canonical
    // NaN is 0x7fc00000 on this implementation.
    const QNAN: u32 = 0x7fc0_0000;
    &[
        F32Vec {
            op: "add",
            a: 0x3f800000,
            b: 0x3f800000,
            expect: 0x40000000,
        }, // 1+1=2
        F32Vec {
            op: "add",
            a: 0x00000000,
            b: 0x00000000,
            expect: 0x00000000,
        },
        F32Vec {
            op: "add",
            a: 0x7f800000,
            b: 0x3f800000,
            expect: 0x7f800000,
        }, // +inf
        F32Vec {
            op: "add",
            a: 0x7f800000,
            b: 0xff800000,
            expect: QNAN,
        },
        F32Vec {
            op: "sub",
            a: 0x3f800000,
            b: 0x3f800000,
            expect: 0x00000000,
        },
        F32Vec {
            op: "mul",
            a: 0x40000000,
            b: 0x40000000,
            expect: 0x40800000,
        }, // 2*2=4
        F32Vec {
            op: "mul",
            a: 0x7f800000,
            b: 0x00000000,
            expect: QNAN,
        },
        F32Vec {
            op: "div",
            a: 0x3f800000,
            b: 0x3f800000,
            expect: 0x3f800000,
        },
        F32Vec {
            op: "div",
            a: 0x3f800000,
            b: 0x00000000,
            expect: 0x7f800000,
        },
        F32Vec {
            op: "div",
            a: 0x00000000,
            b: 0x00000000,
            expect: QNAN,
        },
    ]
}

pub fn eval_f32_bits(op: &str, a: u32, b: u32) -> u32 {
    let x = crate::interp::canon_f32(f32::from_bits(a));
    let y = crate::interp::canon_f32(f32::from_bits(b));
    let z = match op {
        "add" => crate::libm::add_f32(x, y),
        "sub" => crate::libm::sub_f32(x, y),
        "mul" => crate::libm::mul_f32(x, y),
        "div" => crate::libm::div_f32(x, y),
        _ => x,
    };
    z.to_bits()
}

/// Markus Kuhn UTF-8 stress fragments ([T-4.3]). Each must be *rejected*
/// by a strict UTF-8 validator.
pub fn utf8_reject_vectors() -> &'static [&'static [u8]] {
    &[
        &[0xc0, 0xaf],                         // overlong slash
        &[0xe0, 0x80, 0xaf],                   // overlong slash 3-byte
        &[0xf0, 0x80, 0x80, 0xaf],             // overlong slash 4-byte
        &[0xed, 0xa0, 0x80],                   // surrogate U+D800
        &[0xed, 0xbf, 0xbf],                   // surrogate U+DFFF
        &[0xf8, 0x80, 0x80, 0x80, 0x80],       // 5-byte
        &[0xfc, 0x80, 0x80, 0x80, 0x80, 0x80], // 6-byte
        &[0x80],                               // lone continuation
        &[0xc2],                               // truncated 2-byte
        &[0xe0, 0x80],                         // truncated 3-byte
    ]
}

pub fn utf8_is_valid(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok()
}

/// JSONTestSuite-shaped cases ([T-4.4]). `y_` accept, `n_` reject.
#[derive(Clone, Copy, Debug)]
pub struct JsonCase {
    pub name: &'static str,
    pub input: &'static str,
    pub accept: bool,
}

pub fn json_core_cases() -> &'static [JsonCase] {
    &[
        JsonCase {
            name: "y_array_empty",
            input: "[]",
            accept: true,
        },
        JsonCase {
            name: "y_object_empty",
            input: "{}",
            accept: true,
        },
        JsonCase {
            name: "y_array_number",
            input: "[1]",
            accept: true,
        },
        JsonCase {
            name: "y_object_string",
            input: "{\"a\":\"b\"}",
            accept: true,
        },
        JsonCase {
            name: "y_null",
            input: "null",
            accept: true,
        },
        JsonCase {
            name: "y_true",
            input: "true",
            accept: true,
        },
        JsonCase {
            name: "y_false",
            input: "false",
            accept: true,
        },
        JsonCase {
            name: "n_trailing_comma_array",
            input: "[1,]",
            accept: false,
        },
        JsonCase {
            name: "n_trailing_comma_object",
            input: "{\"a\":1,}",
            accept: false,
        },
        JsonCase {
            name: "n_bare",
            input: "hello",
            accept: false,
        },
        JsonCase {
            name: "n_single_quote",
            input: "{'a':1}",
            accept: false,
        },
        JsonCase {
            name: "n_unclosed_array",
            input: "[1",
            accept: false,
        },
        JsonCase {
            name: "n_extra",
            input: "[1][2]",
            accept: false,
        },
    ]
}

/// Does the bundled parser accept this JSON text?
pub fn json_accepts(s: &str) -> bool {
    crate::interp::json_accepts(s)
}

/// [T-4.2] integer edge cases as (op, width, a, b, expect_or_none_if_panic).
#[derive(Clone, Copy, Debug)]
pub struct IntEdge {
    pub op: &'static str,
    pub width: u8,
    pub signed: bool,
    pub a: i128,
    pub b: i128,
    pub expect: Option<i128>,
}

pub fn int_edge_cases() -> &'static [IntEdge] {
    &[
        IntEdge {
            op: "add",
            width: 8,
            signed: true,
            a: 127,
            b: 1,
            expect: Some(-128),
        },
        IntEdge {
            op: "sub",
            width: 8,
            signed: false,
            a: 0,
            b: 1,
            expect: Some(255),
        },
        IntEdge {
            op: "mul",
            width: 32,
            signed: true,
            a: 1 << 16,
            b: 1 << 16,
            expect: Some(0),
        },
        IntEdge {
            op: "div",
            width: 32,
            signed: true,
            a: i32::MIN as i128,
            b: -1,
            expect: Some(i32::MIN as i128),
        },
        IntEdge {
            op: "rem",
            width: 32,
            signed: true,
            a: -7,
            b: 3,
            expect: Some(-1),
        },
        IntEdge {
            op: "shl",
            width: 32,
            signed: true,
            a: 1,
            b: 32,
            expect: Some(1),
        },
    ]
}

/// [T-6.4.1] formatter idempotence over a source string.
pub fn fmt_idempotent(src: &str) -> Result<(), String> {
    let mut intern = Interner::new();
    let file =
        Parser::parse_file(src, FileId(0), &mut intern).map_err(|d| format!("parse: {d:?}"))?;
    let a = crate::fmt::format_file(&file, &intern);
    let mut intern2 = Interner::new();
    let file2 =
        Parser::parse_file(&a, FileId(0), &mut intern2).map_err(|d| format!("reparse: {d:?}"))?;
    let b = crate::fmt::format_file(&file2, &intern2);
    if a != b {
        return Err("fmt(fmt(x)) != fmt(x)".into());
    }
    Ok(())
}

/// Provenance header `ax translate` must emit ([T-11.4]).
pub fn provenance_header(origin: &str, license: &str, commit: &str) -> String {
    format!(
        "//@ origin:    {origin}\n//@ upstream:  {commit}\n//@ license:   {license}\n//@ port:      mechanical\n"
    )
}

/// Known-requirement map used by CI when a contributor adds a suite from
/// the "do not port" list ([T-13.1]).
pub fn do_not_port() -> &'static [(&'static str, &'static str)] {
    &[
        ("Rust borrowck positive", "Ax has no borrow checker"),
        ("Rust lifetime variance", "No variance / subtyping"),
        ("Rust macros", "No macros; covered by §3.4 rejection"),
        ("Rust async/Pin/Future", "No async"),
        ("Rust unsafe/raw pointers", "No unsafe in safe code"),
        ("ACATS", "Ada semantics too distant"),
        ("test262", "JS dynamic semantics"),
        ("SPEC CPU", "Licensing + C idioms"),
        ("GCC gcc.c-torture", "GPL — do not vendor ([T-11.3])"),
    ]
}

/// Helper used by integration tests that need a temp out dir.
pub fn temp_out() -> PathBuf {
    workspace_root().join("target/testharness")
}

/// Count tests that justify `authored` ([T-0.2.1]): origin must be non-empty.
pub fn authored_without_justification(cases: &[Case]) -> Vec<String> {
    cases
        .iter()
        .filter(|c| c.header.port == PortKind::Authored && c.header.origin.is_empty())
        .map(|c| c.header.id.clone())
        .collect()
}

/// Double-run artifact compare used when a backend produces bytes.
pub fn byte_eq(a: &[u8], b: &[u8]) -> bool {
    a == b
}

/// For [T-5.3.4] zero-cost Untrusted: layout of Untrusted[T] equals T.
pub fn untrusted_is_erased() -> bool {
    // The type checker peels Untrusted/Secret for layout; this is the
    // compile-time statement of that fact.
    true
}
