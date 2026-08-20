use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

pub fn migrate(database_url: &str, directory: &Path) -> Result<Vec<PathBuf>, String> {
    let connection = Connection::parse(database_url)?;
    let files = migration_files(directory)?;
    run_sql(&connection, "CREATE TABLE IF NOT EXISTS _ax_migrations (name VARCHAR(255) PRIMARY KEY, applied_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP);")?;
    let mut applied = Vec::new();
    for path in files {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| format!("invalid migration name: {}", path.display()))?;
        let escaped = name.replace('\'', "''");
        let exists = run_sql(
            &connection,
            &format!("SELECT COUNT(*) FROM _ax_migrations WHERE name = '{escaped}';"),
        )?;
        if exists.trim() == "1" {
            continue;
        }
        let mut script = String::from("START TRANSACTION;\n");
        script.push_str(
            &fs::read_to_string(&path)
                .map_err(|error| format!("read {}: {error}", path.display()))?,
        );
        script.push_str(&format!(
            "\nINSERT INTO _ax_migrations(name) VALUES ('{escaped}');\nCOMMIT;\n"
        ));
        run_sql(&connection, &script)?;
        applied.push(path);
    }
    Ok(applied)
}

#[derive(Clone, Debug)]
struct Connection {
    user: String,
    password: String,
    host: String,
    port: String,
    database: String,
}

impl Connection {
    fn parse(url: &str) -> Result<Self, String> {
        let rest = url
            .strip_prefix("mysql://")
            .ok_or("MySQL URL must start with mysql://")?;
        let (authority, database) = rest
            .split_once('/')
            .ok_or("MySQL URL is missing a database")?;
        let (credentials, host_port) = authority.rsplit_once('@').unwrap_or(("root:", authority));
        let (user, password) = credentials.split_once(':').unwrap_or((credentials, ""));
        let (host, port) = host_port.rsplit_once(':').unwrap_or((host_port, "3306"));
        if user.is_empty() || host.is_empty() || database.is_empty() {
            return Err("MySQL URL has an empty user, host, or database".into());
        }
        Ok(Self {
            user: user.into(),
            password: password.into(),
            host: host.into(),
            port: port.into(),
            database: database.into(),
        })
    }
}

fn run_sql(connection: &Connection, script: &str) -> Result<String, String> {
    let mut child = Command::new("mysql")
        .args([
            "--batch",
            "--raw",
            "--skip-column-names",
            "--host",
            &connection.host,
            "--port",
            &connection.port,
            "--user",
            &connection.user,
            &connection.database,
        ])
        .env("MYSQL_PWD", &connection.password)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("start mysql client: {error}"))?;
    use std::io::Write;
    child
        .stdin
        .take()
        .ok_or("mysql stdin unavailable")?
        .write_all(script.as_bytes())
        .map_err(|error| format!("write MySQL migration input: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("wait for mysql: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mysql_urls() {
        let connection = Connection::parse("mysql://alice:secret@db.example:3307/app").unwrap();
        assert_eq!(connection.user, "alice");
        assert_eq!(connection.password, "secret");
        assert_eq!(connection.host, "db.example");
        assert_eq!(connection.port, "3307");
        assert_eq!(connection.database, "app");
    }

    #[test]
    fn orders_migrations_by_file_name() {
        let root = std::env::temp_dir().join(format!("ax-db-mysql-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("002_second.sql"), "SELECT 2;").unwrap();
        fs::write(root.join("001_first.sql"), "SELECT 1;").unwrap();
        assert_eq!(
            migration_files(&root).unwrap()[0].file_name().unwrap(),
            "001_first.sql"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
