//! `ax` compiler protocol CLI (§10.7).

use ax::diag::{catalog, Diagnostic};
use ax::driver::{
    check_report, deps_affected, effs_at, errs_into, guarantee_labels, hole_report, render_diags,
    run_tests, search, types_at, Session,
};
use ax::hash;
use serde_json::json;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        print_help();
        return ExitCode::SUCCESS;
    }
    let cmd = args.remove(0);
    match cmd.as_str() {
        "check" => cmd_check(&args),
        "hole" => cmd_hole(&args),
        "types" => cmd_types(&args),
        "effs" => cmd_effs(&args),
        "search" => cmd_search(&args),
        "errs" => cmd_errs(&args),
        "fmt" => cmd_fmt(&args),
        "patch" => cmd_patch(&args),
        "deps" => cmd_deps(&args),
        "test" => cmd_test(&args),
        "run" => cmd_run(&args),
        "jit" => cmd_jit(&args),
        "replay" => cmd_replay(&args),
        "merge" => cmd_merge(&args),
        "label" => cmd_label(&args),
        "card" => cmd_card(&args),
        "ir" => cmd_ir(&args),
        "fix" => cmd_fix(&args),
        "conform" => cmd_conform(&args),
        "build" => cmd_build(&args),
        "bench" => cmd_bench(&args),
        "perf" => cmd_perf(&args),
        "complete" => cmd_complete(&args),
        "context" => cmd_context(&args),
        "repair" => cmd_repair(&args),
        "pkg" => cmd_pkg(&args),
        "eval-loop" => cmd_eval_loop(&args),
        "help" | "-h" | "--help" => {
            print_help();
            ExitCode::SUCCESS
        }
        "version" | "-V" | "--version" => {
            println!("ax 0.1.0 (research-v1 / oracle)");
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("unknown command `{other}`. try `ax help`.");
            ExitCode::from(2)
        }
    }
}

fn print_help() {
    println!(
        "\
ax — systems language for AI agents

Usage:
  ax check [--json] [--allow-holes] [--strict-det] [--surface conventional|terse|verbose] <file>
  ax hole [--fills] [--json] <file> [<def_id>]
  ax types <file> <def_id>
  ax effs <file> <def_id>
  ax search <file> <query>
  ax errs --into <Type> <file>
  ax fmt <file>
  ax patch --tx <file>
  ax deps --affected <def_id> <file>
  ax test [--attempts-to-green] <file>
  ax run [--seed N] [--trace f] <file>
  ax jit <file> [args...]
  ax replay <trace>
  ax merge --semantic <file> <other>
  ax label <file>
  ax card
  ax ir <file>
  ax conform [filter]
  ax fix [--apply] [--json] <file>
  ax build [-o <bin>] [--tier dev|release|portable] <file>
  ax bench io|http|metrics|tokens|gate|gate-check|all
  ax perf [--json] [--diff <baseline.json>] <file>
  ax complete --at <pos> [--json] <file>
  ax context [--limit=N] <file>
  ax repair [--apply] [--json] <file>
  ax pkg list | ax pkg write
  ax eval-loop [--seed N] [--n K]
"
    );
}

struct Flags {
    json: bool,
    allow_holes: bool,
    strict_det: bool,
    seed: u64,
    trace: Option<PathBuf>,
    into: Option<String>,
    tx: Option<PathBuf>,
    affected: Option<String>,
    files: Vec<PathBuf>,
    rest: Vec<String>,
    surface: Option<String>,
}

fn parse_flags(args: &[String]) -> Flags {
    let mut f = Flags {
        json: false,
        allow_holes: false,
        strict_det: false,
        seed: 0,
        trace: None,
        into: None,
        tx: None,
        affected: None,
        files: Vec::new(),
        rest: Vec::new(),
        surface: None,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => f.json = true,
            "--allow-holes" => f.allow_holes = true,
            "--strict-det" => f.strict_det = true,
            "--seed" => {
                i += 1;
                f.seed = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(0);
            }
            "--trace" => {
                i += 1;
                if let Some(p) = args.get(i) {
                    f.trace = Some(PathBuf::from(p));
                }
            }
            "--into" => {
                i += 1;
                f.into = args.get(i).cloned();
            }
            "--tx" => {
                i += 1;
                if let Some(p) = args.get(i) {
                    f.tx = Some(PathBuf::from(p));
                }
            }
            "--affected" => {
                i += 1;
                f.affected = args.get(i).cloned();
            }
            "--semantic" => {}
            "--surface" => {
                i += 1;
                f.surface = args.get(i).cloned();
            }
            "--no-indep" => {}
            s if s.starts_with('-') => {}
            s => {
                let p = PathBuf::from(s);
                if p.extension().and_then(|e| e.to_str()) == Some("ax") || p.exists() {
                    f.files.push(p);
                } else {
                    f.rest.push(s.to_string());
                }
            }
        }
        i += 1;
    }
    f
}

fn read_src(path: &Path) -> Result<String, ExitCode> {
    fs::read_to_string(path).map_err(|e| {
        eprintln!("{}: {e}", path.display());
        ExitCode::from(1)
    })
}

fn session(flags: &Flags) -> Session {
    let mut s = Session::new();
    s.allow_holes = flags.allow_holes;
    s.strict_det = flags.strict_det;
    if let Some(surf) = flags.surface.as_deref() {
        if let Some(sf) = ax::frontend::Surface::from_str(surf) {
            s.surface = sf;
        }
    }
    s
}

fn fail_diags(s: &Session, diags: &[Diagnostic], json: bool) -> ExitCode {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "ok": false,
                "diagnostics": diags,
            }))
            .unwrap()
        );
    } else {
        eprint!("{}", render_diags(&s.sm, &s.intern, diags));
    }
    ExitCode::from(1)
}

fn cmd_check(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    let Some(path) = flags.files.first() else {
        eprintln!("usage: ax check [--json] [--allow-holes] [--strict-det] <file>");
        return ExitCode::from(2);
    };
    let src = match read_src(path) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let mut s = session(&flags);
    match s.parse(path.to_str().unwrap_or("input.ax"), &src) {
        Err(d) => fail_diags(&s, &d, flags.json),
        Ok(file) => {
            let out = s.check(&file);
            if flags.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&check_report(&out)).unwrap()
                );
            } else if out.diags.iter().any(|d| d.is_error()) {
                eprint!("{}", render_diags(&s.sm, &s.intern, &out.diags));
            } else {
                println!("ok  {}  {} defs", out.module, out.fns.len());
            }
            if out.diags.iter().any(|d| d.is_error()) {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
    }
}

fn compile_file(flags: &Flags) -> Result<(Session, ax::check::CheckOutput), ExitCode> {
    let Some(path) = flags.files.first() else {
        eprintln!("expected a .ax file");
        return Err(ExitCode::from(2));
    };
    let src = read_src(path)?;
    let mut s = session(flags);
    match s.compile(path.to_str().unwrap_or("input.ax"), &src) {
        Ok(out) => Ok((s, out)),
        Err(d) => {
            let code = fail_diags(&s, &d, flags.json);
            Err(code)
        }
    }
}

/// Apply the fixes the checker classifies as `semantics_preserving`, and only
/// those. Everything else is reported and left alone.
fn cmd_fix(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    let Some(path) = flags.files.first() else {
        eprintln!("usage: ax fix [--apply] [--json] <file>");
        return ExitCode::from(2);
    };
    let src = match read_src(path) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let surface = flags
        .surface
        .as_deref()
        .and_then(ax::frontend::Surface::from_str)
        .unwrap_or(ax::frontend::Surface::Conventional);
    let r = ax::agent::apply_safe_fixes(path.to_str().unwrap_or("input.ax"), &src, surface);
    let write = args.iter().any(|a| a == "--apply");
    if flags.json {
        println!("{}", serde_json::to_string_pretty(&r).unwrap());
    } else {
        for f in &r.applied {
            println!(
                "applied  {}  {} -> {}{}",
                f.code,
                f.before,
                f.after,
                f.note.as_ref().map(|n| format!("  ({n})")).unwrap_or_default()
            );
        }
        for f in &r.withheld {
            println!(
                "withheld {}  {} -> {}  (not semantics_preserving; apply it yourself)",
                f.code, f.before, f.after
            );
        }
        if r.applied.is_empty() && r.withheld.is_empty() {
            println!("no fixes offered");
        }
        println!(
            "{}",
            if r.clean {
                "checks clean".to_string()
            } else {
                format!("still failing: {}", r.remaining.join(" "))
            }
        );
    }
    if write && !r.applied.is_empty() {
        if let Err(e) = fs::write(path, &r.source) {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    }
    if r.clean {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// Run the conformance suite: every case on the oracle, both C tiers, and
/// the Cranelift JIT.
fn cmd_conform(args: &[String]) -> ExitCode {
    let filter = args.iter().find(|a| !a.starts_with('-')).cloned();
    let root = ax::conform::suite_dir();
    match ax::conform::run_suite(&root, filter.as_deref()) {
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
        Ok(results) => {
            let mut failed = 0;
            for r in &results {
                match &r.outcome {
                    ax::conform::Outcome::Pass => println!("ok    {}", r.name),
                    ax::conform::Outcome::Fail { tier, detail } => {
                        failed += 1;
                        println!("FAIL  {}  [{tier}] {detail}", r.name);
                    }
                }
            }
            let jit = results.iter().filter(|r| r.jit_ran).count();
            let runnable = results.iter().filter(|r| r.runnable).count();
            println!(
                "\n{} passed, {failed} failed, {} total",
                results.len() - failed,
                results.len()
            );
            // Stated, not assumed: a tier that did not run is not evidence.
            // Stated per tier: the remaining cases are ones the checker must
            // reject, which never reach a backend.
            if jit == runnable {
                println!(
                    "tiers: oracle, cranelift, c dev, c release \
                     ({runnable} executable cases, {} rejected at check time)",
                    results.len() - runnable
                );
            } else {
                println!(
                    "tiers: oracle, c dev, c release; cranelift ran on \
                     {jit}/{runnable} executable cases"
                );
            }
            if failed > 0 {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
    }
}

/// Print the typed IR every backend consumes. The fastest way to see what a
/// program actually compiles to, and what golden tests assert against.
fn cmd_ir(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    match compile_file(&flags) {
        Err(c) => c,
        Ok((s, out)) => match ax::lower::lower_program(&s.intern, &out) {
            Ok(p) => {
                print!("{}", p.to_text());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(1)
            }
        },
    }
}

fn cmd_hole(args: &[String]) -> ExitCode {
    let mut flags = parse_flags(args);
    flags.allow_holes = true;
    // `--fills` synthesises candidate expressions and verifies each by
    // compiling it, so what comes back is known to typecheck.
    if args.iter().any(|a| a == "--fills") {
        let Some(path) = flags.files.first() else {
            eprintln!("usage: ax hole --fills [--json] <file>");
            return ExitCode::from(2);
        };
        let src = match read_src(path) {
            Ok(s) => s,
            Err(c) => return c,
        };
        let surface = flags
            .surface
            .as_deref()
            .and_then(ax::frontend::Surface::from_str)
            .unwrap_or(ax::frontend::Surface::Conventional);
        let name = path.to_str().unwrap_or("input.ax");
        let holes = ax::agent::hole_fills(name, &src, surface, 64);
        if flags.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&ax::agent::FillReport { holes }).unwrap()
            );
            return ExitCode::SUCCESS;
        }
        if holes.is_empty() {
            println!("no holes");
            return ExitCode::SUCCESS;
        }
        for h in &holes {
            println!("hole {}  expects: {}", h.def_id, h.expected);
            let ok = h.fills.iter().filter(|f| f.compiles).count();
            println!("  {ok} of {} candidates compile", h.fills.len());
            for f in h.fills.iter().filter(|f| f.compiles) {
                println!("    {}  {}    {}", f.rank, f.expr, f.note);
            }
            for f in h.fills.iter().filter(|f| !f.compiles).take(3) {
                println!(
                    "    -  {}    rejected: {}",
                    f.expr,
                    f.rejected_by.join(" ")
                );
            }
        }
        return ExitCode::SUCCESS;
    }
    match compile_file(&flags) {
        Err(c) => c,
        Ok((s, out)) => {
            print!(
                "{}",
                hole_report(&s.intern, &out, flags.rest.first().map(|x| x.as_str()))
            );
            ExitCode::SUCCESS
        }
    }
}

fn cmd_types(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    match compile_file(&flags) {
        Err(c) => c,
        Ok((s, out)) => {
            let id = flags.rest.first().map(|s| s.as_str()).unwrap_or("");
            print!("{}", types_at(&s.intern, &out, id));
            ExitCode::SUCCESS
        }
    }
}

fn cmd_effs(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    match compile_file(&flags) {
        Err(c) => c,
        Ok((s, out)) => {
            let id = flags.rest.first().map(|s| s.as_str()).unwrap_or("");
            print!("{}", effs_at(&s.intern, &out, id));
            ExitCode::SUCCESS
        }
    }
}

fn cmd_search(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    match compile_file(&flags) {
        Err(c) => c,
        Ok((s, out)) => {
            let q = flags.rest.join(" ");
            print!("{}", search(&s.intern, &out, &q));
            ExitCode::SUCCESS
        }
    }
}

fn cmd_errs(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    match compile_file(&flags) {
        Err(c) => c,
        Ok((s, out)) => {
            let t = flags.into.as_deref().unwrap_or("");
            print!("{}", errs_into(&s.intern, &out, t));
            ExitCode::SUCCESS
        }
    }
}

fn cmd_fmt(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    let Some(path) = flags.files.first() else {
        eprintln!("usage: ax fmt <file>");
        return ExitCode::from(2);
    };
    let src = match read_src(path) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let mut s = Session::new();
    match s.format(path.to_str().unwrap_or("input.ax"), &src) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(d) => fail_diags(&s, &d, false),
    }
}

fn cmd_patch(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    let Some(txp) = flags.tx.as_ref() else {
        eprintln!("usage: ax patch --tx <txn.json>");
        return ExitCode::from(2);
    };
    let raw = match fs::read_to_string(txp) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    let v: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("invalid transaction: {e}");
            return ExitCode::from(1);
        }
    };
    let expected = v.get("expected_subtree_hash").and_then(|x| x.as_str());
    let base = v.get("base_module_hash").and_then(|x| x.as_str());
    // Optimistic: if a source file is supplied, verify base hash; fail closed on mismatch.
    if let Some(src_path) = flags.files.first() {
        if let Ok(src) = fs::read_to_string(src_path) {
            let h = hash::sha256_hex(src.as_bytes());
            if let Some(b) = base {
                if b != h && b != "…" && !b.is_empty() {
                    eprintln!("transaction failed: base_module_hash mismatch");
                    return ExitCode::from(1);
                }
            }
        }
    }
    if let Some(e) = expected {
        if e.is_empty() {
            eprintln!("transaction failed: missing expected_subtree_hash");
            return ExitCode::from(1);
        }
    }
    let mut intern = ax::Interner::new();
    let src = flags
        .files
        .first()
        .and_then(|p| fs::read_to_string(p).ok())
        .unwrap_or_default();
    let file = match ax::parser::Parser::parse_file(&src, ax::span::FileId(0), &mut intern) {
        Ok(f) => f,
        Err(d) => {
            eprintln!("cannot parse target for patch: {:?}", d);
            return ExitCode::from(1);
        }
    };
    let tx = ax::workspace::PatchTx {
        base_module_hash: v
            .get("base_module_hash")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        def_id: v
            .get("def_id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        path: v
            .get("path")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default(),
        expected_subtree_hash: v
            .get("expected_subtree_hash")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        replacement_src: v
            .get("replacement_src")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        replacement_ast: v.get("replacement_ast").cloned(),
    };
    let result = ax::workspace::apply_patch(&mut intern, &src, &file, &tx);
    if result.applied {
        if let (Some(path), Some(new_src)) = (flags.files.first(), result.source.as_ref()) {
            if let Err(e) = fs::write(path, new_src) {
                eprintln!("{e}");
                return ExitCode::from(1);
            }
        }
    }
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
    if result.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn cmd_deps(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    match compile_file(&flags) {
        Err(c) => c,
        Ok((_s, out)) => {
            let id = flags.affected.as_deref().unwrap_or("");
            print!("{}", deps_affected(&out, id));
            ExitCode::SUCCESS
        }
    }
}

fn cmd_test(args: &[String]) -> ExitCode {
    // `--attempts-to-green` is the north-star metric: how many compile-and-run
    // cycles does it take to fill every hole in this file and leave its tests
    // passing? Reported alongside the cheap static probes it used.
    if args.iter().any(|a| a == "--attempts-to-green") {
        let flags = parse_flags(args);
        let Some(path) = flags.files.first() else {
            eprintln!("usage: ax test --attempts-to-green [--json] <file>");
            return ExitCode::from(2);
        };
        let src = match read_src(path) {
            Ok(s) => s,
            Err(c) => return c,
        };
        let surface = flags
            .surface
            .as_deref()
            .and_then(ax::frontend::Surface::from_str)
            .unwrap_or(ax::frontend::Surface::Conventional);
        let r = ax::evalloop::attempts_to_green(
            path.to_str().unwrap_or("input.ax"),
            &src,
            surface,
        );
        if flags.json {
            println!("{}", serde_json::to_string_pretty(&r).unwrap());
        } else {
            println!(
                "{}  {}  holes filled {}  attempts {}  probes {}  {:.1} ms",
                if r.green { "green" } else { "NOT GREEN" },
                r.path,
                r.holes,
                r.attempts,
                r.probes,
                r.wall_ms
            );
            for a in &r.applied {
                println!("  applied: {a}");
            }
        }
        return if r.green {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        };
    }
    let flags = parse_flags(args);
    match compile_file(&flags) {
        Err(c) => c,
        Ok((s, out)) => {
            if out.holes.iter().any(|_| true) && !flags.allow_holes {
                eprintln!("error: holes are rejected by ax test");
                return ExitCode::from(1);
            }
            let rs = run_tests(&s.intern, &out, flags.seed);
            let mut failed = 0;
            for r in &rs {
                if r.ok {
                    println!("ok    {}", r.name);
                } else {
                    failed += 1;
                    println!("FAIL  {}  {}", r.name, r.msg.clone().unwrap_or_default());
                }
            }
            println!("{} passed, {failed} failed", rs.len() - failed);
            if failed == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
    }
}

/// Arguments the program should see: its own path, then every non-flag argument
/// that came after it.
fn program_argv(args: &[String], src: &Path) -> Vec<String> {
    let src_s = src.to_string_lossy().to_string();
    let mut out = vec![src_s.clone()];
    let mut seen_src = false;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if !seen_src {
            if *a == src_s {
                seen_src = true;
            }
            i += 1;
            continue;
        }
        // Skip `ax`'s own flags, and the value of a flag that takes one.
        if a.starts_with('-') {
            if matches!(a.as_str(), "--seed" | "--trace" | "--into" | "--tx" | "--affected" | "--surface" | "-o") {
                i += 1;
            }
            i += 1;
            continue;
        }
        out.push(a.clone());
        i += 1;
    }
    out
}

fn cmd_run(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    match compile_file(&flags) {
        Err(c) => c,
        Ok((s, out)) => {
            if !out.holes.is_empty() {
                eprintln!("error: holes are rejected by ax run");
                return ExitCode::from(1);
            }
            // The program's argv: the source path, then everything that followed
            // it on the command line. Taken from the raw arguments because a
            // program argument need not look like a file — `ax run p.ax 7` has to
            // pass "7" through, and flag parsing alone cannot tell it from a
            // stray token.
            let src_path = flags.files.first().cloned().unwrap_or_default();
            let argv = program_argv(args, &src_path);
            match ax::driver::run_traced(&s.intern, &out, flags.seed, &argv, None) {
                Ok((v, events)) => {
                    println!("{}", v.display());
                    if let Some(tp) = flags.trace {
                        let src = flags
                            .files
                            .first()
                            .and_then(|p| fs::read_to_string(p).ok())
                            .unwrap_or_default();
                        let tr = ax::workspace::encode_trace(
                            flags.seed,
                            &src,
                            &v.display(),
                            &hex::encode(v.canonical_bytes()),
                            &events,
                        );
                        let _ = fs::write(tp, serde_json::to_string_pretty(&tr).unwrap());
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::from(1)
                }
            }
        }
    }
}

/// Compile with Cranelift and run in this process.
///
/// Exit codes are part of the interface, because the conformance harness spawns
/// this and has to tell three outcomes apart: 0 ran, 1 the program raised or
/// aborted, 3 the backend refused the program. Collapsing the third into a
/// failure would let coverage gaps read as agreement.
fn cmd_jit(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    match compile_file(&flags) {
        Err(c) => c,
        Ok((s, out)) => {
            if !out.holes.is_empty() {
                eprintln!("error: holes are rejected by ax jit");
                return ExitCode::from(1);
            }
            let src_path = flags.files.first().cloned().unwrap_or_default();
            let argv = program_argv(args, &src_path);
            match ax::backend_clif::run_source(&s.intern, &out, &argv) {
                Ok(text) => {
                    if !text.is_empty() {
                        println!("{text}");
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{e}");
                    // "unsupported" is the backend declining, not the program
                    // misbehaving; the harness needs to see the difference.
                    if e.contains("unsupported") || e.contains("cranelift rejected") {
                        ExitCode::from(3)
                    } else {
                        ExitCode::from(1)
                    }
                }
            }
        }
    }
}

fn cmd_replay(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        eprintln!("usage: ax replay <trace> [source.ax]");
        return ExitCode::from(2);
    };
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    let tr: ax::workspace::ReplayTrace = match serde_json::from_str(&raw) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("invalid trace: {e}");
            return ExitCode::from(1);
        }
    };
    let src_path = args.get(1);
    if let Some(sp) = src_path {
        let src = match fs::read_to_string(sp) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(1);
            }
        };
        let got = hash::sha256_hex(src.as_bytes());
        if got != tr.source_hash {
            eprintln!("replay failed: source_hash mismatch");
            return ExitCode::from(1);
        }
        let mut s = Session::new();
        match s.compile(sp, &src) {
            // Replay consumes the recorded transcript: effects return what they
            // returned before, so a file changing on disk cannot make a replay
            // silently disagree.
            Ok(out) => match ax::driver::run_traced(
                &s.intern,
                &out,
                tr.seed,
                &[],
                Some(tr.events.clone()),
            ) {
                Ok((v, _)) => {
                    let canon = hex::encode(v.canonical_bytes());
                    if canon != tr.canonical {
                        eprintln!("replay failed: canonical output mismatch");
                        eprintln!("  recorded {}", tr.canonical);
                        eprintln!("  replayed {canon}");
                        return ExitCode::from(1);
                    }
                    println!("{}", v.display());
                    println!("replay ok  seed={}", tr.seed);
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::from(1)
                }
            },
            Err(d) => fail_diags(&s, &d, false),
        }
    } else {
        println!("{}", raw);
        ExitCode::SUCCESS
    }
}

fn cmd_merge(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    if flags.files.len() < 2 {
        eprintln!("usage: ax merge --semantic <ours.ax> <theirs.ax>");
        return ExitCode::from(2);
    }
    let mut s = Session::new();
    let a_src = match read_src(&flags.files[0]) {
        Ok(x) => x,
        Err(c) => return c,
    };
    let b_src = match read_src(&flags.files[1]) {
        Ok(x) => x,
        Err(c) => return c,
    };
    let a = match s.compile("ours.ax", &a_src) {
        Ok(o) => o,
        Err(d) => return fail_diags(&s, &d, false),
    };
    let b = match s.compile("theirs.ax", &b_src) {
        Ok(o) => o,
        Err(d) => return fail_diags(&s, &d, false),
    };
    let mut conflicts = Vec::new();
    for ea in &a.exports {
        if b.exports.contains(ea) {
            // same name exported from both — conflict if interface hashes differ
            let ha = a
                .hashes
                .iter()
                .find(|h| h.def_id.ends_with(&format!(":{ea}")));
            let hb = b
                .hashes
                .iter()
                .find(|h| h.def_id.ends_with(&format!(":{ea}")));
            match (ha, hb) {
                (Some(x), Some(y)) if x.interface_hash != y.interface_hash => {
                    conflicts.push(format!("duplicate export `{ea}` with differing interface"));
                }
                (Some(_), Some(_)) => {}
                _ => conflicts.push(format!("duplicate export `{ea}`")),
            }
        }
    }
    if conflicts.is_empty() {
        println!("ok  no semantic conflicts");
        ExitCode::SUCCESS
    } else {
        for c in conflicts {
            println!("conflict  {c}");
        }
        ExitCode::from(1)
    }
}

fn cmd_label(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    match compile_file(&flags) {
        Err(c) => c,
        Ok((s, out)) => {
            for l in guarantee_labels(&s.intern, &out, false, false) {
                println!("{l}");
            }
            ExitCode::SUCCESS
        }
    }
}

fn cmd_card(_args: &[String]) -> ExitCode {
    let card = ax::driver::card_text();
    print!("{card}");
    println!("\n---");
    println!("error codes (append-only):");
    for (c, m) in catalog() {
        println!("  {c}  {m}");
    }
    ExitCode::SUCCESS
}

fn cmd_build(args: &[String]) -> ExitCode {
    let mut out_bin: Option<PathBuf> = None;
    let mut files = Vec::new();
    let mut tier = ax::codegen::Tier::Release;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-o" {
            i += 1;
            if let Some(p) = args.get(i) {
                out_bin = Some(PathBuf::from(p));
            }
        } else if args[i] == "--tier" {
            i += 1;
            tier = match args.get(i).map(|s| s.as_str()) {
                Some("dev") => ax::codegen::Tier::Dev,
                Some("portable") => ax::codegen::Tier::Portable,
                _ => ax::codegen::Tier::Release,
            };
        } else if !args[i].starts_with('-') {
            files.push(PathBuf::from(&args[i]));
        }
        i += 1;
    }
    let Some(path) = files.first() else {
        eprintln!("usage: ax build [-o bin] <file.ax>");
        return ExitCode::from(2);
    };
    let src = match read_src(path) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let mut s = Session::new();
    let file = match s.parse(path.to_str().unwrap_or("input.ax"), &src) {
        Ok(f) => f,
        Err(d) => return fail_diags(&s, &d, false),
    };
    let checked = s.check(&file);
    if checked.diags.iter().any(|d| d.is_error()) {
        return fail_diags(&s, &checked.diags, false);
    }
    let out_dir = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    match ax::codegen::build_tier(
        &s.intern,
        &checked,
        path.to_str().unwrap_or("out.ax"),
        &out_dir,
        tier,
    ) {
        Ok(b) => {
            if let Some(dest) = out_bin {
                if let Err(e) = fs::copy(&b.bin_path, &dest) {
                    eprintln!("{e}");
                    return ExitCode::from(1);
                }
                println!("{}", dest.display());
            } else {
                println!("{}", b.bin_path.display());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

fn cmd_bench(args: &[String]) -> ExitCode {
    let which = args.first().map(|s| s.as_str()).unwrap_or("io");
    match ax::bench::run(which) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

fn cmd_perf(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    match compile_file(&flags) {
        Err(c) => c,
        Ok((s, out)) => {
            let name = flags
                .files
                .first()
                .and_then(|p| p.to_str())
                .unwrap_or("input.ax");
            let report = ax::perf::analyze_module(&s.intern, &out, name);
            if let Some(base_path) = args.windows(2).find(|w| w[0] == "--diff").map(|w| &w[1]) {
                match fs::read_to_string(base_path) {
                    Ok(raw) => match serde_json::from_str::<ax::perf::ModulePerf>(&raw) {
                        Ok(base) => {
                            let d = ax::perf::diff(&base, &report);
                            if d.is_empty() {
                                println!("perf: no regression");
                                return ExitCode::SUCCESS;
                            }
                            for line in d {
                                println!("REGRESS  {line}");
                            }
                            return ExitCode::from(1);
                        }
                        Err(e) => {
                            eprintln!("invalid baseline: {e}");
                            return ExitCode::from(2);
                        }
                    },
                    Err(e) => {
                        eprintln!("{e}");
                        return ExitCode::from(2);
                    }
                }
            }
            if flags.json {
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
            } else {
                print!("{}", ax::perf::render_text(&report));
            }
            if report.contracts.iter().any(|c| !c.ok) {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
    }
}

fn cmd_complete(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    match compile_file(&flags) {
        Err(c) => c,
        Ok((s, out)) => {
            let r = ax::perf::complete(&s.intern, &out);
            if flags.json {
                println!("{}", serde_json::to_string_pretty(&r).unwrap());
            } else {
                for c in &r.completions {
                    println!("{}  {}  {}", c.kind, c.name, c.signature);
                }
                println!("gbnf:\n{}", r.gbnf_fragment);
            }
            ExitCode::SUCCESS
        }
    }
}

fn cmd_context(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    let limit = args
        .iter()
        .find_map(|a| a.strip_prefix("--limit=").and_then(|s| s.parse().ok()))
        .unwrap_or(1000usize);
    match compile_file(&flags) {
        Err(c) => c,
        Ok((s, out)) => {
            let r = ax::perf::context_pack(&s.intern, &out, limit);
            if flags.json {
                println!("{}", serde_json::to_string_pretty(&r).unwrap());
            } else {
                print!("{}", r.cheatsheet);
                for d in &r.digests {
                    println!("{d}");
                }
            }
            ExitCode::SUCCESS
        }
    }
}

fn cmd_repair(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    let Some(path) = flags.files.first() else {
        eprintln!("usage: ax repair [--apply] [--json] <file>");
        return ExitCode::from(2);
    };
    let src = match read_src(path) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let r = ax::perf::repair(path.to_str().unwrap_or("input.ax"), &src);
    if flags.json {
        println!("{}", serde_json::to_string_pretty(&r).unwrap());
    } else {
        println!(
            "repair  applied={}  remaining={}  clean={}",
            r.applied.len(),
            r.remaining.len(),
            r.clean
        );
    }
    if args.iter().any(|a| a == "--apply") && !r.applied.is_empty() {
        if let Err(e) = fs::write(path, &r.source) {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    }
    if r.clean {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn cmd_eval_loop(args: &[String]) -> ExitCode {
    let flags = parse_flags(args);
    let n = flags
        .rest
        .iter()
        .find_map(|s| s.strip_prefix("n:").and_then(|x| x.parse().ok()))
        .or_else(|| {
            args.windows(2).find_map(|w| {
                if w[0] == "--n" {
                    w[1].parse().ok()
                } else {
                    None
                }
            })
        })
        .unwrap_or(24);
    let seed = flags.seed;
    // 12 attempts: enough to exhaust the candidate pool for these tasks, so a
    // failure means the agent genuinely could not get there.
    let report = ax::evalloop::run_eval_loop(seed, n, 12);
    println!(
        "hidden corpus  n={}  seed={}\n\
         an attempt is one compile-and-run cycle; a probe is a static query\n\
           ax    pass {}/{}  median attempts {:.1}  median probes {:.1}  median wall {:.1} ms\n",
        report.n,
        report.seed,
        report.ax_pass,
        report.n,
        report.ax_median_attempts,
        {
            let mut ps: Vec<f64> = report.ax.iter().map(|r| r.probes as f64).collect();
            ps.sort_by(|a, b| a.partial_cmp(b).unwrap());
            ps.get(ps.len() / 2).copied().unwrap_or(0.0)
        },
        report.ax_median_wall_ms,
    );
    println!(
        "        median tokens written+read to green: {:.0}",
        report.ax_median_tokens
    );
    if report.rust_skipped {
        println!("  rust  skipped: no rustc on PATH, so there is no control to compare against");
    } else {
        println!(
            "  rust  pass {}/{}  median attempts {:.1}  median wall {:.1} ms",
            report.rust_pass, report.n, report.rust_median_attempts, report.rust_median_wall_ms
        );
        println!(
            "        median tokens written+read to green: {:.0}",
            report.rust_median_tokens
        );
    }
    if flags.json {
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    }
    ExitCode::SUCCESS
}

fn cmd_pkg(args: &[String]) -> ExitCode {
    match args.first().map(|s| s.as_str()).unwrap_or("list") {
        "list" => {
            print!("{}", ax::packages::list_text());
            ExitCode::SUCCESS
        }
        "write" => {
            let dir = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(ax::packages::packs_dir);
            match ax::packages::write_registry(&dir) {
                Ok(ps) => {
                    println!("wrote {} packs under {}", ps.len(), dir.display());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::from(1)
                }
            }
        }
        other => {
            eprintln!("unknown pkg subcommand `{other}` (list|write)");
            ExitCode::from(2)
        }
    }
}
