use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let arg = args.next();
    if args.next().is_some() {
        fail("usage: ax-density [--write]");
    }

    let report = ax_density::render_doc();
    match arg.as_deref() {
        None => print!("{report}"),
        Some("--write") => {
            let path = workspace().join("docs/usecases.md");
            if let Err(error) = ax_density::write_doc(&path) {
                fail(&error);
            }
            print!("{report}");
            eprintln!("wrote {}", path.display());
        }
        Some(_) => fail("usage: ax-density [--write]"),
    }
}

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fail(message: &str) -> ! {
    eprintln!("ax-density: {message}");
    std::process::exit(2)
}
