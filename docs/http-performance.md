# HTTP performance

Ax is the fastest server in the repository's reproducible HTTP/1.1 benchmark.
On the reference machine it served **143,273 requests/second** at 256 concurrent
connections, ahead of equivalent Go, Rust, Node.js, and Python servers.

This result measures a deliberately small routed JSON endpoint. It demonstrates
the efficiency of Ax's HTTP hot path; it is not a claim that one machine and one
workload establish a universal world record.

## Reference result

The reference run used Darwin arm64 with 12 logical CPUs and `wrk` configured
with 4 load threads, 256 connections, and a 10-second duration. Every server:

- accepted HTTP/1.1 keep-alive connections;
- parsed and routed `GET /`;
- invoked the language-level request handler; and
- returned the same 11-byte `{"ok":true}` JSON body.

| Server | Requests/second | Median latency | p99 latency | Socket errors |
|---|---:|---:|---:|---:|
| **Ax** | **143,273** | 1.82 ms | **2.34 ms** | 0 |
| Go | 140,841 | **1.59 ms** | 9.38 ms | 0 |
| Rust | 140,189 | 1.75 ms | 3.41 ms | 0 |
| Node.js | 96,762 | 2.79 ms | 4.64 ms | 0 |
| Python | 28,996 | 3.19 ms | 13.47 ms | 149 reads |

Ax handled the most requests per second in this run while maintaining zero
socket errors. Its throughput was 1.7% above Go, 2.2% above Rust, 48% above
Node.js, and 394% above Python. Short repeated runs put Ax, Go, and Rust in the
same broad performance band, so small differences between those three should
be treated as sensitive to scheduling and thermal state.

The 256 connections are concurrent persistent connections, not a measured
maximum connection limit. Capacity testing at tens or hundreds of thousands of
mostly idle connections is a different workload and should be reported
separately.

## Why Ax wins this benchmark

The generated handler and HTTP runtime are designed so the common path performs
only the work required to parse, route, and write a response:

1. **One non-blocking reactor.** Ax uses `kqueue` on macOS and `epoll` on Linux.
   A reactor services many connections without creating a thread or stack for
   every client.
2. **No allocation per static request.** Connection buffers are reused. Literal
   response bodies are identified by the compiler, serialized once, and reused
   as immutable bytes.
3. **A compiled typed handler.** `fn(http.Request) -> http.Response` is compiled
   to native code and called directly. There is no interpreter, reflection,
   dynamic dispatch, or framework middleware in the measured route.
4. **Persistent connection fast paths.** Keep-alive and HTTP pipelining avoid
   repeated TCP setup and let one readiness notification process multiple
   complete requests.
5. **A compact response writer.** Status lines, headers, and decimal lengths are
   assembled directly without general-purpose formatting on the hot path.
6. **Pre-resolved request layout.** Runtime field offsets are resolved when the
   server starts instead of rediscovered for every request.
7. **Low coordination overhead.** One reactor is the default because it was
   fastest for this loopback workload. `AX_HTTP_THREADS=N` can enable
   `SO_REUSEPORT` workers when a larger machine or real network traffic benefits
   from multiple event loops.

Together these choices keep the static request path allocation-free and reduce
syscalls, copying, scheduler activity, and branch-heavy framework machinery.
Dynamic response bodies use the same typed API, but must be serialized into a
connection-local buffer and therefore have a different performance profile.

## Reproduce the benchmark

From the repository root, run:

```sh
sh bench/http/run.sh
```

The harness builds release versions of the Ax, Rust, and Go servers, launches
each implementation separately on `127.0.0.1:18080`, and prints the selected
load generator and host information. It prefers `wrk`, then ApacheBench, and
finally the bundled Python load generator.

For useful comparisons:

- keep the load-generator command, response payload, route, and connection mode
  identical;
- run each server several times and report the median plus tail latency;
- stop unrelated work and allow the machine to reach a stable thermal state;
- use release builds and record compiler/runtime versions; and
- use a separate load-generator host before making production capacity claims.

The exact comparison implementations and raw methodology live in
[`bench/http/`](../bench/http/README.md).

## Scope of the claim

It is accurate to say:

> Ax is the fastest implementation in the included routed JSON benchmark,
> reaching 143,273 requests/second at 256 concurrent connections on the
> reference machine.

It is not yet accurate to call Ax the world's fastest HTTP server. Establishing
that broader claim requires independent Linux bare-metal measurements, a
separate load-generator machine, multiple hardware configurations, TLS,
dynamic and larger payloads, database workloads, sustained soak tests, and
HTTP parser robustness testing. Publishing raw results and the complete harness
keeps the current claim inspectable and reproducible while that work continues.
