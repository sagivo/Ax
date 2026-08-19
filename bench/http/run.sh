#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
BENCH="$ROOT/bench/http"
TMP=$(mktemp -d "${TMPDIR:-/tmp}/ax-http-bench.XXXXXX")
PIDS=""

cleanup() {
    for pid in $PIDS; do
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    done
    rm -rf "$TMP"
}
trap cleanup EXIT INT TERM

echo "building benchmark servers"
cargo run -q -p ax -- build --tier release -o "$TMP/ax_server" "$BENCH/ax_server.ax"
cargo run -q -p ax-api -- build --tier release -o "$TMP/ax_framework_server" "$BENCH/ax_framework_server.ax"
rustc -C opt-level=3 -o "$TMP/rust_server" "$BENCH/rust_server.rs"
go build -trimpath -o "$TMP/go_server" "$BENCH/go_server.go"

if command -v wrk >/dev/null 2>&1; then
    LOAD_GENERATOR=wrk
elif command -v ab >/dev/null 2>&1; then
    LOAD_GENERATOR=ab
else
    LOAD_GENERATOR=python
fi

run_one() {
    name=$1
    shift
    "$@" >"$TMP/$name.log" 2>&1 &
    pid=$!
    PIDS="$PIDS $pid"
    i=0
    while ! python3 "$BENCH/load.py" --concurrency 1 --requests 1 >"$TMP/ready.json" 2>/dev/null; do
        i=$((i + 1))
        [ "$i" -lt 100 ] || { echo "$name did not start" >&2; return 1; }
        sleep 0.05
    done
    printf '%s\n' "# $name"
    case "$LOAD_GENERATOR" in
        wrk)
            wrk -t4 -c256 -d10s --latency http://127.0.0.1:18080/
            ;;
        ab)
            ab -q -k -n 100000 -c 100 http://127.0.0.1:18080/
            ;;
        *)
            for concurrency in 1 8 32; do
                python3 "$BENCH/load.py" --concurrency "$concurrency" --requests 10000
            done
            ;;
    esac
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    PIDS=$(printf '%s\n' "$PIDS" | sed "s/ $pid//")
}

echo "workload: GET /, 11-byte JSON, HTTP/1.1 keep-alive, generator=$LOAD_GENERATOR"
echo "machine: $(uname -s) $(uname -m), $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
run_one ax env AX_HTTP_THREADS="${AX_HTTP_THREADS:-1}" "$TMP/ax_server"
run_one ax-framework env AX_HTTP_THREADS="${AX_HTTP_THREADS:-1}" "$TMP/ax_framework_server"
run_one rust "$TMP/rust_server"
run_one go "$TMP/go_server"
run_one python python3 "$BENCH/python_server.py"
if command -v node >/dev/null 2>&1; then
    run_one node node "$BENCH/node_server.js"
else
    echo "# node (skipped: node not installed)"
fi
