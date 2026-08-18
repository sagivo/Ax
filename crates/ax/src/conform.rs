//! Conformance suite runner.
//!
//! A case is one `.ax` file under `conformance/` whose header states what must
//! happen. Every case runs on every tier — oracle, native dev, native release,
//! and the Cranelift JIT — and all of them must agree with the stated
//! expectation, not merely with each other. Agreeing with each other only proves
//! the backends share a bug.
//!
//! The JIT tier is spawned as a child process rather than run in-process: an Ax
//! `abort` ends the process it runs in, and a suite that dies without a report
//! when one case regresses is not a suite. The runner is located explicitly (env
//! `AX_JIT_BIN`, else this executable when it is `ax`, else a sibling `ax`) and
//! `CaseResult::jit_ran` records whether it actually ran, so a missing runner
//! cannot masquerade as agreement.
//!
//! Header directives (first lines of the file, `//!`-prefixed):
//!
//! ```text
//! //! expect: 3i32              main's value, in canonical form
//! //! expect-abort: index out of bounds   program must abort with this message
//! //! expect-error: E0200       checker must reject with this code
//! //! expect-tests               module's `test` decls must all pass
//! //! skip-native: reason        oracle only (a documented backend gap)
//! ```
//!
//! Cases are ported from the Go and Rust suites by scenario, not by text: the
//! sources are their `test/` and `tests/ui` directories, but the semantics being
//! pinned here are Ax's, and where Ax deliberately differs (shift counts, NaN
//! canonicalisation) the case documents the difference.

use crate::codegen::{self, Tier};
use crate::driver::{run_main, run_tests, Session};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expect {
    /// `main` returns this, rendered canonically.
    Value(String),
    /// The program aborts with this message.
    Abort(String),
    /// The checker rejects the program with this diagnostic code.
    Error(String),
    /// Every `test` in the module passes.
    Tests,
}

#[derive(Clone, Debug)]
pub struct Case {
    pub path: PathBuf,
    pub name: String,
    pub expect: Expect,
    /// Documented reason this case is oracle-only.
    pub skip_native: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail { tier: String, detail: String },
}

#[derive(Clone, Debug)]
pub struct CaseResult {
    pub name: String,
    pub outcome: Outcome,
    /// Whether the Cranelift tier was exercised for this case.
    pub jit_ran: bool,
    /// Whether any backend tier applies. A case the checker must reject never
    /// reaches a backend, so counting it as a skipped tier would understate
    /// coverage; a case with `skip-native` states its own reason.
    pub runnable: bool,
}

/// Where to find an `ax` binary able to run `ax jit <file>`.
///
/// Explicit rather than guessed: under `cargo test` this process is a test
/// harness, not `ax`, so the integration test passes `CARGO_BIN_EXE_ax` through
/// the environment.
pub fn jit_runner() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("AX_JIT_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    let exe = std::env::current_exe().ok()?;
    if exe.file_stem().and_then(|s| s.to_str()) == Some("ax") {
        return Some(exe);
    }
    let sibling = exe.parent()?.join("ax");
    if sibling.exists() {
        return Some(sibling);
    }
    // `cargo test` puts test binaries in `target/<profile>/deps`.
    let up = exe.parent()?.parent()?.join("ax");
    if up.exists() {
        return Some(up);
    }
    None
}

/// Root of the conformance corpus.
pub fn suite_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../conformance")
}

pub fn discover(root: &Path) -> Result<Vec<Case>, String> {
    let mut out = Vec::new();
    collect(root, root, &mut out)?;
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<Case>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            collect(root, &p, out)?;
        } else if p.extension().and_then(|e| e.to_str()) == Some("ax") {
            let src = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            let name = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .with_extension("")
                .to_string_lossy()
                .to_string();
            let (expect, skip_native) = parse_header(&src)
                .ok_or_else(|| format!("{}: missing `//! expect...` header", p.display()))?;
            out.push(Case {
                path: p,
                name,
                expect,
                skip_native,
            });
        }
    }
    Ok(())
}

fn parse_header(src: &str) -> Option<(Expect, Option<String>)> {
    let mut expect = None;
    let mut skip = None;
    for line in src.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(rest) = line.strip_prefix("//!") else {
            if line.starts_with("//") {
                continue;
            }
            break;
        };
        let rest = rest.trim();
        if let Some(v) = rest.strip_prefix("expect:") {
            expect = Some(Expect::Value(v.trim().to_string()));
        } else if let Some(v) = rest.strip_prefix("expect-abort:") {
            expect = Some(Expect::Abort(v.trim().to_string()));
        } else if let Some(v) = rest.strip_prefix("expect-error:") {
            expect = Some(Expect::Error(v.trim().to_string()));
        } else if rest.starts_with("expect-tests") {
            expect = Some(Expect::Tests);
        } else if let Some(v) = rest.strip_prefix("skip-native:") {
            skip = Some(v.trim().to_string());
        }
    }
    expect.map(|e| (e, skip))
}

/// Run one case on the oracle and, unless skipped, on the native tiers.
pub fn run_case(case: &Case, out_dir: &Path, jit: Option<&Path>) -> CaseResult {
    let src = match std::fs::read_to_string(&case.path) {
        Ok(s) => s,
        Err(e) => return fail(case, "read", e.to_string()),
    };
    let stem = case.name.replace(['/', '\\'], "_");

    let mut s = Session::new();
    let file = match s.parse(&stem, &src) {
        Ok(f) => f,
        Err(diags) => {
            // A parse error satisfies `expect-error` if the code matches.
            return match &case.expect {
                Expect::Error(code) if diags.iter().any(|d| d.code == *code) => pass(case),
                Expect::Error(code) => fail(
                    case,
                    "parse",
                    format!(
                        "expected {code}, got [{}]",
                        diags
                            .iter()
                            .map(|d| d.code.clone())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ),
                _ => fail(
                    case,
                    "parse",
                    diags
                        .iter()
                        .map(|d| format!("{}: {}", d.code, d.msg))
                        .collect::<Vec<_>>()
                        .join("; "),
                ),
            };
        }
    };
    let checked = s.check(&file);
    let errors: Vec<_> = checked.diags.iter().filter(|d| d.is_error()).collect();
    if let Expect::Error(code) = &case.expect {
        return if errors.iter().any(|d| &d.code == code) {
            pass(case)
        } else {
            fail(
                case,
                "check",
                format!(
                    "expected {code}, got [{}]",
                    errors
                        .iter()
                        .map(|d| d.code.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
        };
    }
    if !errors.is_empty() {
        return fail(
            case,
            "check",
            errors
                .iter()
                .map(|d| format!("{}: {}", d.code, d.msg))
                .collect::<Vec<_>>()
                .join("; "),
        );
    }

    // ---- oracle ----
    match &case.expect {
        Expect::Value(want) => match run_main(&s.intern, &checked, 0) {
            Ok(v) => {
                let got = v.display();
                if got != *want {
                    return fail(case, "oracle", format!("want {want}, got {got}"));
                }
            }
            Err(e) => return fail(case, "oracle", format!("expected a value, aborted: {e}")),
        },
        Expect::Abort(want) => match run_main(&s.intern, &checked, 0) {
            Ok(v) => {
                return fail(
                    case,
                    "oracle",
                    format!("expected abort `{want}`, got value {}", v.display()),
                )
            }
            Err(e) => {
                if !e.contains(want) {
                    return fail(case, "oracle", format!("want abort `{want}`, got `{e}`"));
                }
            }
        },
        Expect::Tests => {
            let results = run_tests(&s.intern, &checked, 0);
            if results.is_empty() {
                return fail(case, "oracle", "module declares no tests".into());
            }
            if let Some(f) = results.iter().find(|r| !r.ok) {
                return fail(case, "oracle", format!("test {:?} failed", f.name));
            }
        }
        Expect::Error(_) => unreachable!("handled above"),
    }

    if case.skip_native.is_some() {
        return pass(case);
    }

    // ---- native tiers ----
    for tier in [Tier::Dev, Tier::Release] {
        let br = match codegen::build_tier(&s.intern, &checked, &stem, out_dir, tier) {
            Ok(b) => b,
            Err(e) => return fail(case, tier.as_str(), e),
        };
        let run = codegen::run_bin(&br.bin_path);
        match (&case.expect, run) {
            (Expect::Value(want), Ok(got)) => {
                if got.trim() != *want {
                    return fail(case, tier.as_str(), format!("want {want}, got {got}"));
                }
            }
            (Expect::Value(_), Err(e)) => return fail(case, tier.as_str(), e),
            (Expect::Abort(want), Err(e)) => {
                if !e.contains(want) {
                    return fail(
                        case,
                        tier.as_str(),
                        format!("want abort `{want}`, got `{e}`"),
                    );
                }
            }
            (Expect::Abort(want), Ok(got)) => {
                return fail(
                    case,
                    tier.as_str(),
                    format!("expected abort `{want}`, exited 0 with `{got}`"),
                )
            }
            (Expect::Tests, Ok(got)) => {
                if got.contains("FAIL") {
                    return fail(case, tier.as_str(), got);
                }
            }
            (Expect::Tests, Err(e)) => return fail(case, tier.as_str(), e),
            (Expect::Error(_), _) => unreachable!("handled above"),
        }
    }

    // ---- Cranelift tier ----
    let mut result = pass(case);
    if let Some(bin) = jit {
        if let Err(e) = run_jit(case, bin) {
            return fail(case, "jit", e);
        }
        result.jit_ran = true;
    }
    result
}

fn pass(case: &Case) -> CaseResult {
    CaseResult {
        name: case.name.clone(),
        outcome: Outcome::Pass,
        jit_ran: false,
        runnable: runnable(case),
    }
}

/// Whether a backend tier applies to this case at all.
pub fn runnable(case: &Case) -> bool {
    !matches!(case.expect, Expect::Error(_)) && case.skip_native.is_none()
}

fn fail(case: &Case, tier: &str, detail: String) -> CaseResult {
    CaseResult {
        name: case.name.clone(),
        outcome: Outcome::Fail {
            tier: tier.to_string(),
            detail,
        },
        jit_ran: false,
        runnable: runnable(case),
    }
}

/// Run one case through `ax jit` in a child process and check the expectation.
fn run_jit(case: &Case, bin: &Path) -> Result<(), String> {
    let out = std::process::Command::new(bin)
        .arg("jit")
        .arg(&case.path)
        .output()
        .map_err(|e| format!("spawn {}: {e}", bin.display()))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    // Exit 3 is the backend declining to compile something. It is a real gap and
    // must be reported as a failure here: the corpus states what the language
    // does, and "this tier cannot" is not one of the answers.
    if out.status.code() == Some(3) {
        return Err(format!("backend refused the program: {stderr}"));
    }
    match &case.expect {
        Expect::Value(want) => {
            if !out.status.success() {
                return Err(format!("expected {want}, exited with `{stderr}`"));
            }
            if stdout != *want {
                return Err(format!("want {want}, got {stdout}"));
            }
        }
        Expect::Abort(want) => {
            if out.status.success() {
                return Err(format!("expected abort `{want}`, exited 0 with `{stdout}`"));
            }
            if !stderr.contains(want) && !stdout.contains(want) {
                return Err(format!("want abort `{want}`, got `{stderr}`"));
            }
        }
        Expect::Tests => {
            if !out.status.success() || stdout.contains("FAIL") {
                return Err(format!("tests failed: {stdout} {stderr}"));
            }
            if !stdout.contains("pass ") {
                return Err(format!("no test ran: {stdout}"));
            }
        }
        Expect::Error(_) => unreachable!("handled before any tier runs"),
    }
    Ok(())
}

/// Run the whole suite. Returns results in case order.
pub fn run_suite(root: &Path, filter: Option<&str>) -> Result<Vec<CaseResult>, String> {
    let cases = discover(root)?;
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/conform");
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let jit = jit_runner();
    let mut out = Vec::new();
    for c in &cases {
        if let Some(f) = filter {
            if !c.name.contains(f) {
                continue;
            }
        }
        out.push(run_case(c, &out_dir, jit.as_deref()));
    }
    Ok(out)
}
