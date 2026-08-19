use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::thread;
use std::time::Duration;

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn request(port: u16, raw: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(raw.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn standalone_build_routes_real_http_requests() {
    let port = TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let root = std::env::temp_dir().join(format!("ax-api-e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let source = root.join("app.ax");
    let binary = root.join("app");
    fs::write(
        &source,
        format!(
            r#"module app;
// ax-api port {port}
// ax-api body_limit 2048
// ax-api cors *
// ax-api GET /items/{{id}} -> show
// ax-api GET /users/{{user}}/posts/{{post_id}} -> nested query=expand header=X-Trace
// ax-api GET /stream -> stream
// ax-api POST /items -> create
// ax-api POST /typed -> create_typed body=Item
type Item = {{name: String, count: u64}};
fn show(request: http.Request, id: String) -> http.Response = api.ok(id);
fn nested(request: http.Request, user: String, post_id: String, expand: String, trace: String) -> http.Response = api.ok(expand);
fn stream(request: http.Request) -> http.Response = api.stream("hello");
fn create(request: http.Request) -> http.Response = api.created(request.body);
fn create_typed(request: http.Request, item: Item) -> http.Response = api.ok(item.name);
"#
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ax-api"))
        .args([
            "build",
            "--tier",
            "release",
            "-o",
            binary.to_str().unwrap(),
            source.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let mut server = Server(Command::new(&binary).spawn().unwrap());
    let mut ready = false;
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            ready = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(ready, "server did not listen");

    let shown = request(
        port,
        "GET /items/42?expand=true HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(shown.starts_with("HTTP/1.1 200 OK\r\n"), "{shown}");
    assert!(shown.ends_with("\r\n\r\n42"), "{shown}");

    let decoded = request(
        port,
        "GET /items/a%2Fb?expand=hello+world HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(decoded.ends_with("\r\n\r\na/b"), "{decoded}");

    let nested = request(
        port,
        "GET /users/alice/posts/7?expand=hello+world HTTP/1.1\r\nHost: localhost\r\nX-Trace: e2e\r\nConnection: close\r\n\r\n",
    );
    assert!(nested.starts_with("HTTP/1.1 200 OK\r\n"), "{nested}");
    assert!(
        nested.contains("Access-Control-Allow-Origin: *\r\n"),
        "{nested}"
    );
    assert!(nested.ends_with("\r\n\r\nhello world"), "{nested}");

    let streamed = request(
        port,
        "GET /stream HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(
        streamed.contains("Transfer-Encoding: chunked\r\n"),
        "{streamed}"
    );
    assert!(
        streamed.ends_with("\r\n\r\n5\r\nhello\r\n0\r\n\r\n"),
        "{streamed}"
    );

    let created = request(
        port,
        "POST /items HTTP/1.1\r\nHost: localhost\r\nContent-Length: 12\r\nConnection: close\r\n\r\n{\"name\":\"x\"}",
    );
    assert!(created.starts_with("HTTP/1.1 201 Created\r\n"), "{created}");
    assert!(created.ends_with("\r\n\r\n{\"name\":\"x\"}"), "{created}");

    let chunked = request(
        port,
        "POST /items HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nc\r\n{\"name\":\"x\"}\r\n0\r\n\r\n",
    );
    assert!(chunked.starts_with("HTTP/1.1 201 Created\r\n"), "{chunked}");
    assert!(chunked.ends_with("\r\n\r\n{\"name\":\"x\"}"), "{chunked}");

    let typed = request(
        port,
        "POST /typed HTTP/1.1\r\nHost: localhost\r\nContent-Length: 26\r\nConnection: close\r\n\r\n{\"name\":\"typed\",\"count\":1}",
    );
    assert!(typed.starts_with("HTTP/1.1 200 OK\r\n"), "{typed}");
    assert!(typed.ends_with("\r\n\r\ntyped"), "{typed}");

    let invalid_typed = request(
        port,
        "POST /typed HTTP/1.1\r\nHost: localhost\r\nContent-Length: 26\r\nConnection: close\r\n\r\n{\"name\":\"typed\",\"extra\":1}",
    );
    assert!(
        invalid_typed.starts_with("HTTP/1.1 422 Unprocessable Content\r\n"),
        "{invalid_typed}"
    );

    let missing = request(
        port,
        "GET /missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(
        missing.starts_with("HTTP/1.1 404 Not Found\r\n"),
        "{missing}"
    );
    assert!(missing.contains("\"code\":\"not_found\""), "{missing}");

    let wrong_method = request(
        port,
        "PATCH /stream HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(
        wrong_method.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"),
        "{wrong_method}"
    );
    assert!(
        wrong_method.contains("Allow: GET, POST, PUT, PATCH, DELETE, OPTIONS\r\n"),
        "{wrong_method}"
    );

    let schema = request(
        port,
        "GET /openapi.json HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(schema.starts_with("HTTP/1.1 200 OK\r\n"), "{schema}");
    assert!(schema.contains("\"openapi\":\"3.1.0\""), "{schema}");
    assert!(schema.contains("\"/items/{id}\""), "{schema}");
    assert!(schema.contains("\"in\":\"query\""), "{schema}");
    assert!(schema.contains("\"in\":\"header\""), "{schema}");

    let _ = server.0.kill();
    let _ = server.0.wait();
    fs::remove_dir_all(PathBuf::from(root)).unwrap();
}

#[test]
fn database_state_serves_typed_rows() {
    let port = TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let root = std::env::temp_dir().join(format!("ax-api-db-e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let source = root.join("app.ax");
    let database = root.join("app.sqlite");
    let binary = root.join("app");
    fs::write(
        &source,
        format!(
            r#"module app;
// ax-api port {port}
// ax-api database {}
// ax-api GET /items -> items
type Item = {{id: i64, name: String}};
fn items(database: db.Pool, request: http.Request) -> http.Response !{{alloc[a], err[db.Error], io[db]}} = {{
    db.exec0(database, "CREATE TABLE IF NOT EXISTS items (id INTEGER PRIMARY KEY, name TEXT NOT NULL)");
    db.exec0(database, "INSERT OR IGNORE INTO items(id, name) VALUES (1, 'first')");
    let rows: Vec[Item] = db.query0(database, test.alloc, "SELECT id, name FROM items ORDER BY id");
    api.ok(json.encode(test.alloc, rows))
}};
"#,
            database.display()
        ),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ax-api"))
        .args([
            "build",
            "--tier",
            "release",
            "-o",
            binary.to_str().unwrap(),
            source.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let server = Server(Command::new(&binary).spawn().unwrap());
    let mut ready = false;
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            ready = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(ready, "server did not listen");
    let response = request(
        port,
        "GET /items HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(
        response.ends_with("\r\n\r\n[{\"id\":1,\"name\":\"first\"}]"),
        "{response}"
    );
    drop(server);
    fs::remove_dir_all(PathBuf::from(root)).unwrap();
}
