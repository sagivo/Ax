use std::env;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ax-db: {error}");
            ExitCode::from(1)
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    if args.is_empty() || matches!(args[0].as_str(), "help" | "-h" | "--help") {
        println!(
            "ax-db — database tools for Ax\n\nUsage:\n  ax-db migrate DATABASE [MIGRATIONS_DIR]"
        );
        return Ok(());
    }
    match args[0].as_str() {
        "migrate" => {
            let database = args.get(1).ok_or("migrate requires a database path")?;
            let directory = args.get(2).map(String::as_str).unwrap_or("migrations");
            let files = ax_db::migrate(Path::new(database), Path::new(directory))?;
            println!("applied {} migration files", files.len());
            Ok(())
        }
        command => Err(format!("unknown command `{command}`; try `ax-db help`")),
    }
}
