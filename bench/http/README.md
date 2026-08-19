# HTTP concurrency benchmark

For the architectural explanation, headline results, and scope of Ax's
performance claim, see [`docs/http-performance.md`](../../docs/http-performance.md).

This compares the same routed JSON `GET /` endpoint in direct Ax, the Ax API
framework, Rust, Go, Python, and Node.js. Every implementation parses an
HTTP/1.1 request, routes `/`, and returns the same 11-byte `{"ok":true}` body
with keep-alive enabled. Both Ax arms call a compiled
`fn(http.Request) -> http.Response` handler for every request; neither is a
static-response shortcut. Including both makes framework dispatch overhead
visible instead of hiding it in the language result.

Run it from the repository root with:

```sh
sh bench/http/run.sh
```

The harness builds the Ax, Rust, and Go servers and starts each on
`127.0.0.1:18080`. It prefers `wrk` (4 load threads, 256 connections, 10
seconds), falls back to ApacheBench (100 connections, 100,000 requests), and
finally to the bundled Python client. The selected generator and machine are
printed with every run.

The `ax-framework` arm is built by the separate `ax-api` package. Three
alternating five-second samples on the reference machine produced medians of
145,382 requests/sec for generated framework routing and 145,044 requests/sec
for direct Ax routing. The 0.23% difference is noise-level evidence of parity,
not a claim that either path is intrinsically faster.

Results are machine- and OS-dependent. Record CPU, OS, toolchain versions, and
the exact command. A loopback run measures both client and server on one
machine; a serious capacity claim requires a separate load-generator host and
multiple payload sizes. Requests/second at one configured concurrency is not a
maximum-connections claim.

## Ax fast path

`http.serve_handler` runs a kqueue reactor on macOS and an epoll reactor on
Linux. It uses non-blocking sockets, HTTP pipelining, a pre-resolved record ABI,
and no per-request allocation. Literal response bodies are proven static by
lowering, allowing the runtime to serialize each distinct response once and
reuse it safely. Dynamic bodies use the same API but are serialized per
request. `AX_HTTP_THREADS=N` enables `SO_REUSEPORT` workers; one reactor is the
measured loopback default because it was faster on the test machine.

See [`examples/api_server.ax`](../../examples/api_server.ax) for typed routing.
The lower-level `http.listen`, `http.accept`, `http.respond`, and `http.close`
primitives remain available for custom protocols and request loops.

## Reference run

One full harness run on 2026-08-19 used Darwin arm64 (12 logical CPUs), `wrk`
with 4 threads and 256 connections for 10 seconds, Apple clang 21, Rust 1.95,
Go 1.26.2, Node 25.1, and Python 3.13.7:

| server | requests/sec | p50 | p99 | socket errors |
|---|---:|---:|---:|---:|
| **Ax** | **143,273** | 1.82 ms | **2.34 ms** | 0 |
| Go | 140,841 | **1.59 ms** | 9.38 ms | 0 |
| Rust | 140,189 | 1.75 ms | 3.41 ms | 0 |
| Node.js | 96,762 | 2.79 ms | 4.64 ms | 0 |
| Python | 28,996 | 3.19 ms | 13.47 ms | 149 reads |

Ax was fastest in this run, but its 1.7% lead over Go is within the range where
host scheduling and thermal state matter. Three additional five-second samples
put Ax, Go, and Rust in the same broad 147k requests/sec band. Treat this as a
reproducible local result, not evidence that Ax is universally the world's
fastest server. Production comparisons still need separate client hardware,
TLS, larger and dynamic payloads, and sustained soak testing.
