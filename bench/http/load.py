#!/usr/bin/env python3
"""Small dependency-free HTTP/1.1 concurrency driver for the Ax server bench."""

import argparse
import json
import statistics
import socket
import threading
import time


class HttpSession:
    def __init__(self, host, port, close_after_response=False):
        self.socket = socket.create_connection((host, port), timeout=5)
        self.buffer = b""
        connection = b"close" if close_after_response else b"keep-alive"
        self.request_bytes = (
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: "
            + connection
            + b"\r\n\r\n"
        )

    def request(self):
        started = time.perf_counter_ns()
        self.socket.sendall(self.request_bytes)
        while b"\r\n\r\n" not in self.buffer:
            chunk = self.socket.recv(65536)
            if not chunk:
                raise RuntimeError("connection closed before response headers")
            self.buffer += chunk
        header, self.buffer = self.buffer.split(b"\r\n\r\n", 1)
        if not header.startswith(b"HTTP/1.1 200"):
            raise RuntimeError("non-200 HTTP response")
        content_length = None
        for line in header.split(b"\r\n")[1:]:
            name, separator, value = line.partition(b":")
            if separator and name.lower() == b"content-length":
                content_length = int(value.strip())
                break
        if content_length is None:
            raise RuntimeError("missing Content-Length")
        while len(self.buffer) < content_length:
            chunk = self.socket.recv(65536)
            if not chunk:
                raise RuntimeError("connection closed before response body")
            self.buffer += chunk
        body, self.buffer = self.buffer[:content_length], self.buffer[content_length:]
        if not body:
            raise RuntimeError("empty response")
        return time.perf_counter_ns() - started

    def close(self):
        self.socket.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=18080)
    parser.add_argument("--concurrency", type=int, default=32)
    parser.add_argument("--requests", type=int, default=5000)
    parser.add_argument(
        "--fresh-connections",
        action="store_true",
        help="open a new TCP connection for each request instead of using keep-alive",
    )
    args = parser.parse_args()
    if args.concurrency < 1 or args.requests < 1:
        parser.error("concurrency and requests must be positive")

    latencies = []
    errors = []
    lock = threading.Lock()
    barrier = threading.Barrier(args.concurrency)

    def worker(worker_id):
        mine = args.requests // args.concurrency + (worker_id < args.requests % args.concurrency)
        session = None
        try:
            barrier.wait()
            for _ in range(mine):
                try:
                    if session is None:
                        session = HttpSession(
                            args.host, args.port, close_after_response=args.fresh_connections
                        )
                    latency = session.request()
                    with lock:
                        latencies.append(latency)
                    if args.fresh_connections:
                        session.close()
                        session = None
                except Exception as exc:
                    with lock:
                        errors.append(str(exc))
                    if session is not None:
                        session.close()
                        session = None
        except threading.BrokenBarrierError:
            errors.append("worker barrier broke")
        finally:
            if session is not None:
                session.close()

    started = time.perf_counter_ns()
    threads = [threading.Thread(target=worker, args=(i,)) for i in range(args.concurrency)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    elapsed_ns = time.perf_counter_ns() - started
    latencies.sort()

    def percentile(p):
        if not latencies:
            return None
        index = min(len(latencies) - 1, int((len(latencies) - 1) * p))
        return latencies[index] / 1_000_000

    result = {
        "concurrency": args.concurrency,
        "requested": args.requests,
        "completed": len(latencies),
        "errors": len(errors),
        "elapsed_s": elapsed_ns / 1_000_000_000,
        "requests_per_second": len(latencies) * 1_000_000_000 / elapsed_ns,
        "p50_ms": percentile(0.50),
        "p99_ms": percentile(0.99),
        "mean_ms": statistics.mean(latencies) / 1_000_000 if latencies else None,
    }
    print(json.dumps(result, sort_keys=True))
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
