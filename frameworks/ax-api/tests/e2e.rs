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
fn show(request: http.Request, id: String) -> http.Response = api.ok(id);
fn nested(request: http.Request, user: String, post_id: String, expand: String, trace: String) -> http.Response = api.ok(expand);
fn stream(request: http.Request) -> http.Response = api.stream("hello");
fn create(request: http.Request) -> http.Response = api.created(request.body);
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

    let nested = request(
        port,
        "GET /users/alice/posts/7?expand=comments HTTP/1.1\r\nHost: localhost\r\nX-Trace: e2e\r\nConnection: close\r\n\r\n",
    );
    assert!(nested.starts_with("HTTP/1.1 200 OK\r\n"), "{nested}");
    assert!(
        nested.contains("Access-Control-Allow-Origin: *\r\n"),
        "{nested}"
    );
    assert!(nested.ends_with("\r\n\r\ncomments"), "{nested}");

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

    let missing = request(
        port,
        "GET /missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(
        missing.starts_with("HTTP/1.1 404 Not Found\r\n"),
        "{missing}"
    );
    assert!(missing.contains("\"code\":\"not_found\""), "{missing}");

    let schema = request(
        port,
        "GET /openapi.json HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(schema.starts_with("HTTP/1.1 200 OK\r\n"), "{schema}");
    assert!(schema.contains("\"openapi\":\"3.1.0\""), "{schema}");
    assert!(schema.contains("\"/items/{id}\""), "{schema}");

    let _ = server.0.kill();
    let _ = server.0.wait();
    fs::remove_dir_all(PathBuf::from(root)).unwrap();
}
