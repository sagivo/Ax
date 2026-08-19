use std::path::{Path, PathBuf};
use std::process::Command;

const DEV_MODULES: &[&str] = &[
    "axmock",
    "bench",
    "conform",
    "evalloop",
    "fuzz",
    "harvest",
    "silent",
    "software",
    "testharness",
    "tokens",
    "translate",
];

const DEV_COMMANDS: &[&str] = &[
    "bench",
    "conform",
    "harvest",
    "testharness",
    "kill-criteria",
    "silent-wrongness",
    "k1",
    "eval-loop",
    "translate",
    "attempts-to-green",
];

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn core_source_and_manifest_have_no_development_tools() {
    let root = workspace();
    let core = root.join("crates/ax");
    let library = read(&core.join("src/lib.rs"));
    let cli = read(&core.join("src/main.rs"));
    let manifest = read(&core.join("Cargo.toml"));

    for module in DEV_MODULES {
        assert!(
            !core.join(format!("src/{module}.rs")).exists(),
            "development module {module}.rs is inside the core source tree"
        );
        assert!(
            !library.contains(&format!("mod {module}")),
            "development module {module} is exported by the core library"
        );
    }
    for command in DEV_COMMANDS {
        assert!(
            !cli.contains(&format!("\"{command}\" =>")),
            "development command {command} is dispatched by the shipped CLI"
        );
    }
    for forbidden in ["tiktoken-rs", "ax-dev", "ax-density"] {
        assert!(
            !manifest.contains(forbidden),
            "core manifest contains development dependency {forbidden}"
        );
    }
}

#[test]
fn packaged_ax_crate_excludes_development_sources_and_tests() {
    let root = workspace();
    let output = Command::new("cargo")
        .args([
            "package",
            "-p",
            "ax",
            "--allow-dirty",
            "--no-verify",
            "--list",
        ])
        .current_dir(&root)
        .output()
        .expect("run cargo package --list");
    assert!(
        output.status.success(),
        "cargo package failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let files = String::from_utf8(output.stdout).expect("package list is UTF-8");
    assert!(!files.lines().any(|path| path.starts_with("tests/")));
    assert!(!files.lines().any(|path| path.starts_with("tools/")));
    for module in DEV_MODULES {
        assert!(
            !files.lines().any(|path| path == format!("src/{module}.rs")),
            "development source src/{module}.rs is present in the ax package"
        );
    }
}
