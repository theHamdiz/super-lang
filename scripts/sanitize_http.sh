#!/usr/bin/env bash
# ASan + UBSan gate for the AOT HTTP hello path (Round 7 Item 1.3).
#
# Puts a sanitizer `cc` wrapper first on PATH, builds a multi-route server,
# runs 10_000 keep-alive requests (GET /ping, POST /echo, GET /tick), fails on
# any sanitizer report. Optional SUPER_SANITIZE_VALGRIND=1 for a sampled valgrind run.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
SUPER_BIN="${SUPER_BIN:-$REPO/target/release/super}"
STD="${SUPER_STD:-$REPO/stdlib}"
PORT="${PORT:-3140}"
REQUESTS="${REQUESTS:-10000}"
WORKDIR=$(mktemp -d)
trap 'kill -INT ${SPID:-} 2>/dev/null || true; rm -rf "$WORKDIR"' EXIT

REAL_CC="$(command -v clang || command -v gcc || command -v cc || true)"
if [[ -z "$REAL_CC" ]]; then
  echo "FAIL: need clang or gcc with sanitizers" >&2
  exit 2
fi

mkdir -p "$WORKDIR/bin"
cat > "$WORKDIR/bin/cc" <<WRAP
#!/usr/bin/env bash
exec "$REAL_CC" -fsanitize=address,undefined -O1 -g -fno-omit-frame-pointer -fno-sanitize-recover=all "\$@"
WRAP
chmod +x "$WORKDIR/bin/cc"
# Also intercept common driver names the toolchain may pick.
ln -sf cc "$WORKDIR/bin/gcc"
ln -sf cc "$WORKDIR/bin/clang"
export PATH="$WORKDIR/bin:$PATH"

cat > "$WORKDIR/san.sr" <<'EOF'
use std.net.http
use std.sys.env

+main() {
    addr = env_get_or("SUPER_ADDR", "127.0.0.1:3140")
    app = App.new()
    app = @app.get("/ping", |_req| { ok("pong") })
    app = @app.post("/echo", |req| { ok(req.body) })
    app = @app.get("/tick", |_req| { ok("t") })
    cfg = ServeConfig.defaults()
    cfg.max_requests = 10000000
    @app.serve_with(addr, cfg) or 1
}
EOF

# detect_leaks=0 for the request-path UAF/invalid-free gate: process-lifetime
# App/Server/Router roots are intentionally not freed on SIGINT exit and would
# otherwise dominate LSan. Request-path leaks are covered by the flat-RSS soak
# and alloc_probe (3 alloc / 0 free). Set SUPER_SANITIZE_LEAKS=1 to also fail
# on process-exit LSan (expect known durable-root noise without suppressions).
if [[ "${SUPER_SANITIZE_LEAKS:-0}" == "1" ]]; then
  export ASAN_OPTIONS="${ASAN_OPTIONS:-detect_leaks=1:halt_on_error=1:abort_on_error=1:exitcode=42}"
else
  export ASAN_OPTIONS="${ASAN_OPTIONS:-detect_leaks=0:halt_on_error=1:abort_on_error=1:exitcode=42}"
fi
export UBSAN_OPTIONS="${UBSAN_OPTIONS:-halt_on_error=1:print_stacktrace=1:exitcode=42}"

echo "== building sanitized AOT server with $REAL_CC wrapper =="
# Debug profile avoids -O3/-flto/-s which fight sanitizers.
( cd "$WORKDIR" && SUPER_STD="$STD" "$SUPER_BIN" build -d san.sr -o "$WORKDIR/san" )

echo "== starting sanitized server on 127.0.0.1:${PORT} =="
ASAN_OPTIONS="$ASAN_OPTIONS" UBSAN_OPTIONS="$UBSAN_OPTIONS" \
  SUPER_ADDR="127.0.0.1:${PORT}" "$WORKDIR/san" >"$WORKDIR/server.log" 2>&1 &
SPID=$!
for _ in $(seq 1 400); do
  if curl -s -o /dev/null --max-time 0.2 "http://127.0.0.1:${PORT}/ping"; then break; fi
  if ! kill -0 "$SPID" 2>/dev/null; then
    echo "FAIL: server exited early" >&2
    cat "$WORKDIR/server.log" >&2 || true
    exit 1
  fi
  sleep 0.05
done

python3 - "$PORT" "$REQUESTS" <<'PY'
import socket, sys
port = int(sys.argv[1]); n = int(sys.argv[2])
shapes = [
    b"GET /ping HTTP/1.1\r\nHost: x\r\nConnection: keep-alive\r\n\r\n",
    b"POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\nContent-Type: text/plain\r\nConnection: keep-alive\r\n\r\nbody",
    b"GET /tick HTTP/1.1\r\nHost: x\r\nConnection: keep-alive\r\n\r\n",
]

def open_f():
    s = socket.create_connection(("127.0.0.1", port), 5)
    return s.makefile("rwb", buffering=0), s

def one(f, raw):
    f.write(raw); f.flush()
    status = f.readline(); cl = 0
    while True:
        line = f.readline()
        if line in (b"\r\n", b"\n", b""):
            break
        if line.lower().startswith(b"content-length:"):
            cl = int(line.split(b":")[1])
    f.read(cl)
    if not status.startswith(b"HTTP/1.1 200"):
        raise RuntimeError(status)

f, s = open_f()
for i in range(n):
    raw = shapes[i % 3]
    try:
        one(f, raw)
    except Exception:
        try: s.close()
        except Exception: pass
        f, s = open_f()
        one(f, raw)
s.close()
print(f"sanitize_http: {n} requests OK", flush=True)
PY

kill -INT "$SPID" 2>/dev/null || true
set +e
wait "$SPID"
SC=$?
set -e
if grep -E 'ERROR: AddressSanitizer|ERROR: UndefinedBehaviorSanitizer|runtime error:|heap-use-after-free|invalid-free|stack-buffer' "$WORKDIR/server.log"; then
  echo "FAIL: sanitizer diagnostics in server log" >&2
  cat "$WORKDIR/server.log" >&2
  exit 1
fi
if [[ "$SC" -eq 42 ]]; then
  echo "FAIL: sanitized server exited with sanitizer exitcode=42" >&2
  cat "$WORKDIR/server.log" >&2
  exit 1
fi
echo "PASS: ASan+UBSan clean for ${REQUESTS} requests (no UAF / invalid-free / UB)"


if [[ "${SUPER_SANITIZE_VALGRIND:-0}" == "1" ]] && command -v valgrind >/dev/null 2>&1; then
  echo "== valgrind leak check (sampled) =="
  # Rebuild without sanitizer wrapper for valgrind.
  export PATH="${PATH#$WORKDIR/bin:}"
  ( cd "$WORKDIR" && SUPER_STD="$STD" "$SUPER_BIN" build --release san.sr -o "$WORKDIR/san_vg" )
  VG_PORT=$((PORT + 1))
  valgrind --error-exitcode=99 --leak-check=full --show-leak-kinds=definite \
    env SUPER_ADDR="127.0.0.1:${VG_PORT}" "$WORKDIR/san_vg" >"$WORKDIR/vg.log" 2>&1 &
  VPID=$!
  for _ in $(seq 1 200); do
    if curl -s -o /dev/null --max-time 0.2 "http://127.0.0.1:${VG_PORT}/ping"; then break; fi
    sleep 0.05
  done
  python3 - "$VG_PORT" <<'PY'
import socket, sys
port=int(sys.argv[1]); n=200
s=socket.create_connection(("127.0.0.1",port),5)
f=s.makefile("rwb",buffering=0)
raw=b"GET /ping HTTP/1.1\r\nHost: x\r\nConnection: keep-alive\r\n\r\n"
for _ in range(n):
    f.write(raw); f.flush()
    st=f.readline(); cl=0
    while True:
        line=f.readline()
        if line in (b"\r\n",b"\n",b""): break
        if line.lower().startswith(b"content-length:"): cl=int(line.split(b":")[1])
    f.read(cl)
s.close()
print("valgrind client done", n)
PY
  kill -INT "$VPID" 2>/dev/null || true
  wait "$VPID" || true
  if grep -q "definitely lost:" "$WORKDIR/vg.log" && ! grep -q "definitely lost: 0 bytes" "$WORKDIR/vg.log"; then
    echo "FAIL: valgrind definite leaks" >&2
    tail -40 "$WORKDIR/vg.log" >&2
    exit 1
  fi
  echo "PASS: valgrind (sampled) clean"
fi
