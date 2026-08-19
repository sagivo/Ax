use ax_dev::{conform, evalloop, harvest, silent, testharness};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().cloned() else {
        help();
        return ExitCode::SUCCESS;
    };
    args.remove(0);
    match command.as_str() {
        "bench" => bench(&args),
        "conform" => conform_cmd(&args),
        "attempts-to-green" => attempts_to_green(&args),
        "translate" => translate(&args),
        "harvest" => harvest_cmd(&args),
        "gbnf-check" => gbnf_check(&args),
        "testharness" => testharness_cmd(&args),
        "silent-wrongness" => silent_cmd(&args),
        "k1" => k1(&args),
        "eval-loop" => eval_loop(&args),
        "kill-criteria" => {
            println!("{}", evalloop::kill_criteria_report());
            ExitCode::SUCCESS
        }
        "help" | "-h" | "--help" => {
            help();
            ExitCode::SUCCESS
        }
        other => fail(&format!("unknown command `{other}`; try `ax-dev help`"), 2),
    }
}

fn help() {
    println!(
        "\
ax-dev — repository-only Ax validation and experiments

Usage:
  ax-dev bench io|http|metrics|tokens|software|gate|gate-check|all
  ax-dev conform [filter]
  ax-dev attempts-to-green [--json] <file>
  ax-dev translate <rust-file>
  ax-dev harvest <rust-tests-ui-dir>
  ax-dev gbnf-check [N]
  ax-dev testharness [filter]
  ax-dev silent-wrongness [--json] [filter]
  ax-dev k1 [--json] [--seed N] [--n K]
  ax-dev eval-loop [--json] [--seed N] [--n K]
  ax-dev kill-criteria
"
    );
}

fn bench(args: &[String]) -> ExitCode {
    let which = args.first().map(String::as_str).unwrap_or("io");
    match ax_dev::bench::run(which) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(&error, 1),
    }
}

fn conform_cmd(args: &[String]) -> ExitCode {
    let filter = args.iter().find(|arg| !arg.starts_with('-'));
    let root = conform::suite_dir();
    match conform::run_suite(&root, filter.map(String::as_str)) {
        Err(error) => fail(&error, 2),
        Ok(results) => {
            let mut failed = 0;
            for result in &results {
                match &result.outcome {
                    conform::Outcome::Pass => println!("ok    {}", result.name),
                    conform::Outcome::Fail { tier, detail } => {
                        failed += 1;
                        println!("FAIL  {}  [{tier}] {detail}", result.name);
                    }
                }
            }
            println!(
                "\n{} passed, {failed} failed, {} total",
                results.len() - failed,
                results.len()
            );
            if failed == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
    }
}

fn attempts_to_green(args: &[String]) -> ExitCode {
    let Some(path) = args.iter().find(|arg| !arg.starts_with('-')) else {
        return fail("usage: ax-dev attempts-to-green [--json] <file>", 2);
    };
    let src = match std::fs::read_to_string(path) {
        Ok(src) => src,
        Err(error) => return fail(&format!("{path}: {error}"), 1),
    };
    let surface = ax_dev::tree::detect_surface(&src, ax_dev::frontend::Surface::Tree);
    let result = evalloop::attempts_to_green(path, &src, surface);
    if has(args, "--json") {
        println!("{}", serde_json::to_string_pretty(&result).unwrap());
    } else {
        println!(
            "{}  {}  holes filled {}  attempts {}  probes {}  {:.1} ms",
            if result.green { "green" } else { "NOT GREEN" },
            result.path,
            result.holes,
            result.attempts,
            result.probes,
            result.wall_ms
        );
        for applied in &result.applied {
            println!("  applied: {applied}");
        }
    }
    if result.green {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn harvest_cmd(args: &[String]) -> ExitCode {
    let Some(dir) = args.iter().find(|arg| !arg.starts_with('-')) else {
        return fail("usage: ax-dev harvest <rust-tests-ui-dir>", 2);
    };
    let dest = testharness::suite_dir().join("rust_ported/inverted/harvested");
    match harvest::harvest_into(
        Path::new(dir),
        &dest,
        "4d91de4e48198da2e33413efdcd9cd2cc0c46688",
    ) {
        Err(error) => fail(&error, 1),
        Ok(report) => {
            println!(
                "harvest hits={} written={} skipped_unsafe_or_macro={} skipped_other={}",
                report.hits.len(),
                report.written.len(),
                report.skipped_unsafe_or_macro,
                report.skipped_other.len()
            );
            ExitCode::SUCCESS
        }
    }
}

fn translate(args: &[String]) -> ExitCode {
    let Some(path) = args.iter().find(|arg| !arg.starts_with('-')) else {
        return fail("usage: ax-dev translate <rust-file>", 2);
    };
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => return fail(&format!("{path}: {error}"), 1),
    };
    let report = ax_dev::translate::translate_rust(&source);
    print!(
        "{}",
        ax_dev::translate::with_provenance(
            &report.source,
            path,
            "MIT OR Apache-2.0",
            "unpinned-local",
        )
    );
    for note in &report.notes {
        eprintln!("note: {note}");
    }
    for rejected in &report.rejected {
        eprintln!("rejected: {rejected}");
    }
    if report.rejected.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn testharness_cmd(args: &[String]) -> ExitCode {
    let filter = args.iter().find(|arg| !arg.starts_with('-'));
    let root = testharness::suite_dir();
    let cases = match testharness::discover(&root) {
        Ok(cases) => cases,
        Err(error) => return fail(&error, 2),
    };
    match testharness::run_suite(&root, filter.map(String::as_str)) {
        Err(error) => fail(&error, 2),
        Ok(results) => {
            print!("{}", testharness::render_summary(&results, &cases));
            if results
                .iter()
                .any(|result| matches!(result.outcome, testharness::Outcome::Fail { .. }))
            {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
    }
}

fn gbnf_check(args: &[String]) -> ExitCode {
    let n = args
        .first()
        .and_then(|value| value.parse().ok())
        .unwrap_or(100_000);
    let generated = ax_dev::gbnf_check::check_generator_parses(n, 1);
    let roundtrip = ax_dev::gbnf_check::check_parser_subset(n, 2);
    println!("gbnf check n={n} gen_parse_fail={generated} fmt_roundtrip_fail={roundtrip}");
    if generated == 0 && roundtrip == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn silent_cmd(args: &[String]) -> ExitCode {
    let filter = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .map(String::as_str);
    let report = silent::run(filter);
    if has(args, "--json") {
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    } else {
        print!("{}", silent::render(&report));
    }
    ExitCode::SUCCESS
}

fn k1(args: &[String]) -> ExitCode {
    let report = evalloop::run_2x2(value(args, "--seed", 42), value(args, "--n", 24), 12);
    if has(args, "--json") {
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    } else {
        print!("{}", evalloop::render_2x2(&report));
    }
    ExitCode::SUCCESS
}

fn eval_loop(args: &[String]) -> ExitCode {
    let report = evalloop::run_eval_loop(value(args, "--seed", 42), value(args, "--n", 24), 12);
    if has(args, "--json") {
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    } else {
        println!(
            "hidden corpus n={} seed={} ax={}/{} rust={}/{}",
            report.n, report.seed, report.ax_pass, report.n, report.rust_pass, report.n
        );
    }
    ExitCode::SUCCESS
}

fn value<T: std::str::FromStr + Copy>(args: &[String], name: &str, default: T) -> T {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .and_then(|pair| pair[1].parse().ok())
        .unwrap_or(default)
}

fn has(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == flag)
}

fn fail(message: &str, code: u8) -> ExitCode {
    eprintln!("ax-dev: {message}");
    ExitCode::from(code)
}
