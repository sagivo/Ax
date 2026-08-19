use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn migration_files(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = fs::read_dir(directory)
        .map_err(|error| format!("read {}: {error}", directory.display()))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    files.retain(|path| path.extension().and_then(|value| value.to_str()) == Some("sql"));
    files.sort();
    Ok(files)
}

pub fn migrate(database: &Path, directory: &Path) -> Result<Vec<PathBuf>, String> {
    let files = migration_files(directory)?;
    run_sql(
        database,
        "PRAGMA foreign_keys = ON; CREATE TABLE IF NOT EXISTS _ax_migrations (name TEXT PRIMARY KEY, applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);",
    )?;
    let mut applied = Vec::new();
    for path in files {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| format!("invalid migration name: {}", path.display()))?;
        let escaped = name.replace('\'', "''");
        let exists = run_sql(
            database,
            &format!("SELECT EXISTS(SELECT 1 FROM _ax_migrations WHERE name = '{escaped}');"),
        )?;
        if exists.trim() == "1" {
            continue;
        }
        let mut script = String::from("BEGIN IMMEDIATE;\n");
        script.push_str(
            &fs::read_to_string(&path)
                .map_err(|error| format!("read {}: {error}", path.display()))?,
        );
        script.push_str("\n");
        script.push_str(&format!(
            "INSERT INTO _ax_migrations(name) VALUES ('{escaped}');\nCOMMIT;\n"
        ));
        run_sql(database, &script)?;
        applied.push(path);
    }
    Ok(applied)
}

fn run_sql(database: &Path, script: &str) -> Result<String, String> {
    let mut child = Command::new("sqlite3")
        .arg("-bail")
        .arg(database)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("start sqlite3: {error}"))?;
    use std::io::Write;
    child
        .stdin
        .take()
        .ok_or("sqlite3 stdin unavailable")?
        .write_all(script.as_bytes())
        .map_err(|error| format!("write sqlite3 migration input: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("wait for sqlite3: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn orders_migrations_by_file_name() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("ax-db-migrations-{nonce}"));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("002_second.sql"), "SELECT 2;").unwrap();
        fs::write(directory.join("001_first.sql"), "SELECT 1;").unwrap();
        fs::write(directory.join("notes.txt"), "ignored").unwrap();
        let files = migration_files(&directory).unwrap();
        assert_eq!(files[0].file_name().unwrap(), "001_first.sql");
        assert_eq!(files[1].file_name().unwrap(), "002_second.sql");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn applies_each_migration_once() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("ax-db-apply-{nonce}"));
        let directory = root.join("migrations");
        let database = root.join("app.sqlite");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("001_items.sql"),
            "CREATE TABLE items (id INTEGER PRIMARY KEY);",
        )
        .unwrap();
        assert_eq!(migrate(&database, &directory).unwrap().len(), 1);
        assert!(migrate(&database, &directory).unwrap().is_empty());
        let tables = run_sql(
            &database,
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='items';",
        )
        .unwrap();
        assert_eq!(tables.trim(), "1");
        fs::remove_dir_all(root).unwrap();
    }
}
