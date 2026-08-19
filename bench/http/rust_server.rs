use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

const RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: keep-alive\r\n\r\n{\"ok\":true}";
const NOT_FOUND: &[u8] = b"HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: 21\r\nConnection: keep-alive\r\n\r\n{\"error\":\"not_found\"}";

fn serve(mut stream: TcpStream) {
    let mut request = [0u8; 4096];
    let mut used = 0;
    loop {
        let read = match stream.read(&mut request[used..]) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        used += read;
        while let Some(start) = request[..used].windows(4).position(|w| w == b"\r\n\r\n") {
            let end = start + 4;
            let response = if request[..end].starts_with(b"GET / HTTP/1.1") {
                RESPONSE
            } else {
                NOT_FOUND
            };
            if stream.write_all(response).is_err() {
                return;
            }
            request.copy_within(end..used, 0);
            used -= end;
        }
        if used == request.len() {
            return;
        }
    }
}

fn main() {
    let listener = TcpListener::bind(("127.0.0.1", 18080)).unwrap();
    for stream in listener.incoming() {
        if let Ok(stream) = stream {
            thread::spawn(|| serve(stream));
        }
    }
}
