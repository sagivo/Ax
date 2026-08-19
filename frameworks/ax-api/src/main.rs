use ax::codegen::{self, Tier};
use ax::driver::{render_diags, Session};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use std::env;
use std::fs;
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ax-api: {error}");
            ExitCode::from(1)
        }
    }
}

fn run(mut args: Vec<String>) -> Result<(), String> {
    if args.is_empty() || matches!(args[0].as_str(), "help" | "-h" | "--help") {
        print_help();
        return Ok(());
    }
    let command = args.remove(0);
    match command.as_str() {
        "new" => new_app(args.first().map(String::as_str).unwrap_or("ax-api-app")),
        "expand" => {
            let path = source_arg(&args)?;
            let source = fs::read_to_string(path).map_err(|e| e.to_string())?;
            let (_, generated) = ax_api::compile(&source)?;
            print!("{generated}");
            Ok(())
        }
        "build" | "run" => {
            let path = source_arg(&args)?;
            let output = output_arg(&args);
            let tier = tier_arg(&args);
            let binary = build(path, output.as_deref(), tier)?;
            if command == "run" {
                if let Some((cert, key, tls_port)) = tls_args(&args)? {
                    let source = fs::read_to_string(path).map_err(|e| e.to_string())?;
                    let app = ax_api::compile(&source)?.0;
                    return tls_run(&binary, app.port, tls_port, &cert, &key);
                }
                let status = Command::new(&binary)
                    .status()
                    .map_err(|e| format!("start {}: {e}", binary.display()))?;
                if !status.success() {
                    return Err(format!("server exited with {status}"));
                }
            } else {
                println!("{}", binary.display());
            }
            Ok(())
        }
        "watch" => {
            let path = source_arg(&args)?;
            watch(path, tier_arg(&args), interval_arg(&args))
        }
        _ => Err(format!("unknown command `{command}`; try `ax-api help`")),
    }
}

fn print_help() {
    println!(
        "ax-api — standalone REST framework for Ax\n\n\
         Usage:\n  ax-api new [directory]\n  ax-api build [-o binary] [--tier dev|release] app.ax\n  ax-api run [--tier dev|release] [--tls-cert cert --tls-key key --tls-port 8443] app.ax\n  ax-api watch [--tier dev|release] [--interval ms] app.ax\n  ax-api expand app.ax"
    );
}

fn interval_arg(args: &[String]) -> Duration {
    let millis = args
        .windows(2)
        .find(|pair| pair[0] == "--interval")
        .and_then(|pair| pair[1].parse::<u64>().ok())
        .unwrap_or(250)
        .max(25);
    Duration::from_millis(millis)
}

fn tls_args(args: &[String]) -> Result<Option<(PathBuf, PathBuf, u16)>, String> {
    let cert = args
        .windows(2)
        .find(|pair| pair[0] == "--tls-cert")
        .map(|p| PathBuf::from(&p[1]));
    let key = args
        .windows(2)
        .find(|pair| pair[0] == "--tls-key")
        .map(|p| PathBuf::from(&p[1]));
    if cert.is_none() && key.is_none() {
        return Ok(None);
    }
    let cert = cert.ok_or("--tls-cert is required with --tls-key")?;
    let key = key.ok_or("--tls-key is required with --tls-cert")?;
    let port = args
        .windows(2)
        .find(|pair| pair[0] == "--tls-port")
        .and_then(|pair| pair[1].parse().ok())
        .unwrap_or(8443);
    Ok(Some((cert, key, port)))
}

fn tls_run(
    binary: &Path,
    backend_port: u16,
    tls_port: u16,
    cert_path: &Path,
    key_path: &Path,
) -> Result<(), String> {
    let cert_file = File::open(cert_path).map_err(|e| format!("read TLS certificate: {e}"))?;
    let key_file = File::open(key_path).map_err(|e| format!("read TLS key: {e}"))?;
    let certs = rustls_pemfile::certs(&mut BufReader::new(cert_file))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("parse TLS certificate: {e}"))?;
    let key = rustls_pemfile::private_key(&mut BufReader::new(key_file))
        .map_err(|e| format!("parse TLS key: {e}"))?
        .ok_or("TLS key file contains no private key")?;
    let config = Arc::new(
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| format!("configure TLS: {e}"))?,
    );
    let listener = TcpListener::bind(("127.0.0.1", tls_port))
        .map_err(|e| format!("bind TLS port {tls_port}: {e}"))?;
    let mut child = Command::new(binary)
        .spawn()
        .map_err(|e| format!("start {}: {e}", binary.display()))?;
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", backend_port)).is_ok() {
            break;
        }
        if let Some(status) = child.try_wait().map_err(|e| format!("wait backend: {e}"))? {
            return Err(format!(
                "backend exited before TLS listener was ready: {status}"
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("configure TLS listener: {e}"))?;
    println!("ax-api: TLS listening on https://127.0.0.1:{tls_port}");
    loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("wait backend: {e}"))? {
            eprintln!("ax-api: backend exited with {status}");
            break;
        }
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25));
                continue;
            }
            Err(e) => return Err(format!("ax-api TLS accept: {e}")),
        };
        let config = Arc::clone(&config);
        thread::spawn(move || {
            if let Err(e) = tls_proxy(stream, config, backend_port) {
                eprintln!("ax-api TLS connection: {e}");
            }
        });
    }
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
    Ok(())
}

fn tls_proxy(stream: TcpStream, config: Arc<ServerConfig>, backend_port: u16) -> io::Result<()> {
    let connection = ServerConnection::new(config).map_err(io::Error::other)?;
    let mut tls = StreamOwned::new(connection, stream);
    let request = read_http_message(&mut tls)?;
    let mut backend = TcpStream::connect(("127.0.0.1", backend_port))?;
    backend.write_all(&request)?;
    let response = read_http_message(&mut backend)?;
    tls.write_all(&response)?;
    tls.flush()
}

fn read_http_message<S: Read>(stream: &mut S) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buf = [0u8; 4096];
    let mut body_len = None;
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        if body_len.is_none() {
            if let Some(header_end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let header_end = header_end + 4;
                body_len = Some(parse_content_length(&bytes[..header_end]).unwrap_or(0));
            }
        }
        if let Some(length) = body_len {
            if let Some(header_end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                if bytes.len() >= header_end + 4 + length {
                    break;
                }
            }
        }
    }
    Ok(bytes)
}

fn parse_content_length(headers: &[u8]) -> Option<usize> {
    let text = std::str::from_utf8(headers).ok()?;
    text.lines()
        .find_map(|line| {
            line.strip_prefix("Content-Length:")
                .or_else(|| line.strip_prefix("content-length:"))
        })?
        .trim()
        .parse()
        .ok()
}

fn watch(source_path: &Path, tier: Tier, interval: Duration) -> Result<(), String> {
    let mut child: Option<Child> = None;
    let mut last_modified: Option<Vec<(PathBuf, std::time::SystemTime)>> = None;
    loop {
        let modified = source_fingerprint(source_path)?;
        if last_modified.as_ref() != Some(&modified) {
            if let Some(mut running) = child.take() {
                let _ = running.kill();
                let _ = running.wait();
            }
            let binary = build(source_path, None, tier)?;
            println!("ax-api: rebuilt {}", binary.display());
            child = Some(
                Command::new(&binary)
                    .spawn()
                    .map_err(|e| format!("start {}: {e}", binary.display()))?,
            );
            last_modified = Some(modified);
        }
        thread::sleep(interval);
    }
}

fn source_fingerprint(source_path: &Path) -> Result<Vec<(PathBuf, std::time::SystemTime)>, String> {
    let root = source_path
        .parent()
        .ok_or_else(|| format!("source has no parent: {}", source_path.display()))?;
    let mut files = Vec::new();
    fn visit(path: &Path, files: &mut Vec<(PathBuf, std::time::SystemTime)>) -> io::Result<()> {
        let metadata = fs::metadata(path)?;
        if metadata.is_file() {
            if path.extension().and_then(|ext| ext.to_str()) == Some("ax") {
                files.push((path.to_path_buf(), metadata.modified()?));
            }
            return Ok(());
        }
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            visit(&entry.path(), files)?;
        }
        Ok(())
    }
    visit(root, &mut files).map_err(|e| format!("scan {}: {e}", root.display()))?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    if !files.iter().any(|(path, _)| path == source_path) {
        let metadata = fs::metadata(source_path)
            .map_err(|e| format!("stat {}: {e}", source_path.display()))?;
        files.push((
            source_path.to_path_buf(),
            metadata.modified().map_err(|e| e.to_string())?,
        ));
    }
    Ok(files)
}

fn source_arg(args: &[String]) -> Result<&Path, String> {
    let mut source = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--tier" => index += 2,
            value if value.starts_with('-') => index += 1,
            value => {
                source = Some(Path::new(value));
                index += 1;
            }
        }
    }
    source.ok_or_else(|| "missing application source".into())
}

fn output_arg(args: &[String]) -> Option<PathBuf> {
    args.windows(2)
        .find(|pair| pair[0] == "-o")
        .map(|pair| PathBuf::from(&pair[1]))
}

fn tier_arg(args: &[String]) -> Tier {
    match args
        .windows(2)
        .find(|pair| pair[0] == "--tier")
        .map(|pair| pair[1].as_str())
    {
        Some("dev") => Tier::Dev,
        _ => Tier::Release,
    }
}

fn build(source_path: &Path, output: Option<&Path>, tier: Tier) -> Result<PathBuf, String> {
    let source = fs::read_to_string(source_path)
        .map_err(|e| format!("read {}: {e}", source_path.display()))?;
    let (_, generated) = ax_api::compile(&source)?;
    let build_dir = source_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".ax-api");
    fs::create_dir_all(&build_dir).map_err(|e| e.to_string())?;
    let stem = source_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("app");
    let generated_path = build_dir.join(format!("{stem}.generated.ax"));
    fs::write(&generated_path, &generated).map_err(|e| e.to_string())?;

    let mut session = Session::new();
    let file = session
        .parse(
            generated_path.to_str().unwrap_or("app.generated.ax"),
            &generated,
        )
        .map_err(|diags| render_diags(&session.sm, &session.intern, &diags))?;
    let checked = session.check(&file);
    if checked.diags.iter().any(|diag| diag.is_error()) {
        return Err(render_diags(&session.sm, &session.intern, &checked.diags));
    }
    let built = codegen::build_tier(
        &session.intern,
        &checked,
        generated_path.to_str().unwrap_or("app.generated.ax"),
        &build_dir,
        tier,
    )?;
    if let Some(destination) = output {
        fs::copy(&built.bin_path, destination)
            .map_err(|e| format!("write {}: {e}", destination.display()))?;
        Ok(destination.to_path_buf())
    } else {
        Ok(built.bin_path)
    }
}

fn new_app(directory: &str) -> Result<(), String> {
    let root = PathBuf::from(directory);
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let app = root.join("app.ax");
    if app.exists() {
        return Err(format!("{} already exists", app.display()));
    }
    fs::write(&app, ax_api::STARTER).map_err(|e| e.to_string())?;
    println!("created {}", app.display());
    println!("run: ax-api run {}", app.display());
    Ok(())
}
