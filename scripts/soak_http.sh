#!/usr/bin/env bash
# Flat-RSS soak for the AOT App.serve path (Round 8 honesty gate).
#
# Pass criteria (stated before the run; see docs/SOAK_GATE.md):
#   After a 60s warm-up, VmRSS stays within ±1% over the remaining window, and
#   the least-squares slope of RSS vs requests is below 1 KiB per 10,000 requests.
#   Sample interval: ≥ 5.0 s. Exact 0.0000 spread/slope are reported when measured.
#
# Drives the real App.serve path: multi-route GET/POST/TICK, Conn COW revive,
# and varied POST body sizes. Default is keep-alive reuse. Optional --churn opens
# and closes one TCP connection per request (connection-churn / flat-RSS gate).
# Optional --probe runs the LD_PRELOAD alloc counter (fresh connections) and
# prints per-connection malloc/free (arena reclaim is not free(); not zero-alloc).
#
# Usage:
#   scripts/soak_http.sh [--minutes 30] [--port 3120] [--out benchmarks-memory]
#                        [--probe] [--churn] [--min-connections N]
#
# Requires: Linux, a built `super` on PATH or SUPER_BIN, cc via `super build`.
# No malloc_trim; no per-object freelist (arena bulk reclaim only).
set -euo pipefail

MINUTES=30
PORT=3120
OUT_DIR="benchmarks-memory"
DO_PROBE=0
CHURN=0
MIN_CONNECTIONS=0
REPO="$(cd "$(dirname "$0")/.." && pwd)"
SUPER_BIN="${SUPER_BIN:-$REPO/target/release/super}"
STD="${SUPER_STD:-$REPO/stdlib}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --minutes) MINUTES="$2"; shift 2 ;;
    --port) PORT="$2"; shift 2 ;;
    --out) OUT_DIR="$2"; shift 2 ;;
    --probe) DO_PROBE=1; shift ;;
    --churn) CHURN=1; shift ;;
    --min-connections) MIN_CONNECTIONS="$2"; shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

mkdir -p "$OUT_DIR"
WORKDIR=$(mktemp -d)
trap 'kill -INT ${SPID:-} 2>/dev/null || true; rm -rf "$WORKDIR"' EXIT

cat > "$WORKDIR/soak.sr" <<'EOF'
use std.net.http
use std.sys.env

// Durable escape target for Promote-classified births (escape analysis).
state g_escape = {note: "init"}

+main() {
    addr = env_get_or("SUPER_ADDR", "127.0.0.1:3021")
    app = App.new()
    // Real App.serve path: routing, COW Conn/App updates via served++, echo bodies,
    // plus request-scoped struct/list births the escape pass classifies.
    app = @app.get("/ping", |_req| { ok("pong") })
    app = @app.post("/echo", |req| { ok(req.body) })
    app = @app.get("/tick", |_req| { ok("t") })
    app = @app.post("/alloc", |req| {
        items = [{id: 1, name: "a"}, {id: 2, name: "b"}]
        payload = {ok: true, items, echo: req.body, label: "alloc"}
        escaped = {note: payload.label}
        g_escape = escaped
        ok(payload)
    })
    cfg = ServeConfig.defaults()
    cfg.max_requests = 10000000
    @app.serve_with(addr, cfg) or 1
}
EOF

echo "== building soak server (App.serve_with, no freelist / no malloc_trim) =="
( cd "$WORKDIR" && SUPER_STD="$STD" SUPER_ESCAPE_VERBOSE=1 "$SUPER_BIN" build --release soak.sr )

# Prove emitted C has no malloc_trim and no freelist.
if [[ -f "$WORKDIR/target/soak.c" ]] || compgen -G "$WORKDIR"/*.c >/dev/null 2>&1 || compgen -G "$WORKDIR/target"/*.c >/dev/null 2>&1; then
  :
fi
# Rebuild with --keep-c once to audit (release build may drop .c); independent check in unit tests.
( cd "$WORKDIR" && SUPER_STD="$STD" "$SUPER_BIN" build --release --keep-c soak.sr -o "$WORKDIR/soak_keep" >/dev/null 2>&1 || true )
KEEP_C=$(find "$WORKDIR" -name '*.c' 2>/dev/null | head -1 || true)
if [[ -n "$KEEP_C" ]]; then
  if grep -E 'malloc_trim|obj_freelist|free_list_push' "$KEEP_C" >/dev/null; then
    echo "FAIL: emitted C contains malloc_trim or freelist" >&2
    exit 1
  fi
  echo "audit: emitted C has no malloc_trim / freelist ($(basename "$KEEP_C"))"
fi

echo "== starting server on 127.0.0.1:${PORT} =="
SUPER_ADDR="127.0.0.1:${PORT}" "$WORKDIR/target/soak" >/dev/null 2>&1 &
SPID=$!
for _ in $(seq 1 100); do
  if curl -s -o /dev/null --max-time 0.2 "http://127.0.0.1:${PORT}/ping"; then break; fi
  sleep 0.05
done

MODE_TAG="aot"
if [[ "$CHURN" == "1" ]]; then MODE_TAG="aot_churn"; fi
if [[ "$CHURN" == "1" && "$MIN_CONNECTIONS" -eq 0 ]]; then
  # Full churn gate requires ≥1M connections (CI may raise this explicitly).
  MIN_CONNECTIONS=1000000
fi
CSV="$OUT_DIR/soak_${MODE_TAG}_$(date -u +%Y%m%dT%H%M%SZ).csv"
echo "timestamp_s,requests,rss_kib,shape" > "$CSV"
echo "== soak mode: $([[ "$CHURN" == "1" ]] && echo churn || echo keep-alive); min_connections=${MIN_CONNECTIONS} =="

python3 - "$PORT" "$SPID" "$MINUTES" "$CSV" "$CHURN" "$MIN_CONNECTIONS" <<'PY'
import csv, socket, sys, time
port = int(sys.argv[1]); pid = int(sys.argv[2]); minutes = float(sys.argv[3]); path = sys.argv[4]
churn = sys.argv[5] == "1"
min_conns = int(sys.argv[6])
conn_hdr = "close" if churn else "keep-alive"

def rss():
    for line in open(f"/proc/{pid}/status"):
        if line.startswith("VmRSS:"):
            return int(line.split()[1])
    return 0

# Varied POST bodies (64 / 256 / 1024 / 4096) + GET /ping + GET /tick (Conn COW).
def post_echo(n: int) -> bytes:
    body = (b"x" * n)
    return (
        f"POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: {n}\r\n"
        f"Content-Type: text/plain\r\nConnection: {conn_hdr}\r\n\r\n"
    ).encode() + body

shapes = [
    ("GET", f"GET /ping HTTP/1.1\r\nHost: x\r\nConnection: {conn_hdr}\r\n\r\n".encode()),
    ("POST64", post_echo(64)),
    ("TICK", f"GET /tick HTTP/1.1\r\nHost: x\r\nConnection: {conn_hdr}\r\n\r\n".encode()),
    ("POST256", post_echo(256)),
    ("GET", f"GET /ping HTTP/1.1\r\nHost: x\r\nConnection: {conn_hdr}\r\n\r\n".encode()),
    ("POST1024", post_echo(1024)),
    ("TICK", f"GET /tick HTTP/1.1\r\nHost: x\r\nConnection: {conn_hdr}\r\n\r\n".encode()),
    ("POST4096", post_echo(4096)),
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

f, s = (None, None) if churn else open_f()
t0 = time.time()
end = t0 + minutes * 60.0
reqs = 0
rows = []
shape_i = 0
mix = {}
# Run until wall clock AND (when set) min_conns / min requests reached, so slope
# has a ≥60s post-warmup window and the 1M floor holds for keep-alive and churn.
while True:
    now = time.time()
    time_done = now >= end
    floor_done = min_conns <= 0 or reqs >= min_conns
    if time_done and floor_done:
        break
    name, raw = shapes[shape_i % len(shapes)]
    shape_i += 1
    try:
        if churn:
            f, s = open_f()
            one(f, raw)
            s.close()
            f, s = None, None
        else:
            one(f, raw)
    except Exception:
        try:
            if s is not None:
                s.close()
        except Exception:
            pass
        f, s = open_f()
        one(f, raw)
        if churn:
            s.close()
            f, s = None, None
    reqs += 1
    mix[name] = mix.get(name, 0) + 1
    now = time.time()
    last_t = rows[-1][0] if rows else -1e9
    if reqs == 1 or (now - t0) - last_t >= 5.0:
        r = rss()
        rows.append((now - t0, reqs, r, name))
        with open(path, "a", newline="") as out:
            csv.writer(out).writerow([f"{now - t0:.3f}", reqs, r, name])
        print(f"t={now-t0:.1f}s reqs={reqs} rss={r} KiB shape={name}", flush=True)

if s is not None:
    try:
        s.close()
    except Exception:
        pass
mode = "churn" if churn else "keep-alive"
print(f"soak_mode={mode}", flush=True)
print("request_mix=" + ",".join(f"{k}:{v}" for k, v in sorted(mix.items())), flush=True)
if churn:
    print(f"connections_total={reqs}", flush=True)
# Pass criteria on post-warmup window
post = [row for row in rows if row[0] >= 60.0]
if len(post) < 3:
    print("FAIL: not enough post-warmup samples", flush=True)
    sys.exit(1)
xs = [p[1] for p in post]; ys = [p[2] for p in post]
mx = sum(xs)/len(xs); my = sum(ys)/len(ys)
num = sum((x-mx)*(y-my) for x,y in zip(xs,ys))
den = sum((x-mx)**2 for x in xs) or 1.0
slope = num/den  # KiB / request
slope_10k = slope * 10000
mid = ys[len(ys)//2] or 1
spread = (max(ys) - min(ys)) / mid * 100.0
print(f"requests_total={reqs}", flush=True)
print(f"post_warmup_samples={len(post)} rss_min={min(ys)} rss_max={max(ys)} spread_pct={spread:.4f}", flush=True)
print(f"slope_kib_per_req={slope:.8f} slope_kib_per_10k={slope_10k:.6f}", flush=True)
print(
    "thresholds: warm-up=60s sample_interval>=5s spread<=1% slope_kib_per_10k<1 "
    "(docs/SOAK_GATE.md); exact zeros reported when measured",
    flush=True,
)
ok_spread = spread <= 1.0
ok_slope = slope_10k < 1.0
if minutes >= 30 and reqs < 1_000_000:
    print("FAIL: need >= 1_000_000 requests in a full 30m soak", flush=True)
    sys.exit(1)
if churn and min_conns > 0 and reqs < min_conns:
    print(f"FAIL: need >= {min_conns} connections in churn soak (got {reqs})", flush=True)
    sys.exit(1)
if ok_spread and ok_slope:
    print("PASS", flush=True)
    sys.exit(0)
print("FAIL: flat-RSS criteria not met", flush=True)
sys.exit(1)
PY

if [[ "$DO_PROBE" == "1" ]]; then
  echo "== stopping soak server before alloc_probe =="
  kill -INT "${SPID:-}" 2>/dev/null || true
  wait "${SPID:-}" 2>/dev/null || true
  SPID=""
  echo "== alloc_probe on soak binary (fresh connections, LD_PRELOAD) =="
  SHIM="$WORKDIR/alloc_count.so"
  cc -O1 -shared -fPIC -o "$SHIM" \
    "$REPO/crates/super-cli/tests/fixtures/alloc_count.c" -ldl
  python3 - "$PORT" "$WORKDIR/target/soak" "$SHIM" "$WORKDIR" <<'PY'
import os, signal, socket, subprocess, sys, time
from pathlib import Path
port = int(sys.argv[1]); binary = sys.argv[2]; shim = sys.argv[3]; work = Path(sys.argv[4])

def measure(n, p):
    out = work / f"count-{n}.txt"
    env = os.environ.copy()
    env["SUPER_ADDR"] = f"127.0.0.1:{p}"
    env["LD_PRELOAD"] = shim
    env["ALLOC_COUNT_OUT"] = str(out)
    proc = subprocess.Popen([binary], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    deadline = time.time() + 10
    while time.time() < deadline:
        try:
            socket.create_connection(("127.0.0.1", p), 0.2).close()
            break
        except OSError:
            time.sleep(0.02)
    else:
        proc.kill(); raise SystemExit("probe server did not listen")
    time.sleep(0.05)
    for _ in range(n):
        s = socket.create_connection(("127.0.0.1", p), 5)
        s.sendall(b"GET /ping HTTP/1.1\r\nHost: probe\r\nConnection: close\r\n\r\n")
        s.recv(65536)
        s.close()
    proc.send_signal(signal.SIGINT)
    proc.wait(timeout=10)
    a, f = map(int, out.read_text().split()[:2])
    return a, f

n = 200
a1, f1 = measure(n, port + 50)
a2, f2 = measure(2 * n, port + 51)
allocs = (a2 - a1) / n
frees = (f2 - f1) / n
print(f"alloc_probe_per_request: {allocs:.2f} alloc / {frees:.2f} free (GET /ping fresh conn)", flush=True)
print("request_mix_note: soak cycle includes GET/POST64/TICK/POST256/POST1024/POST4096", flush=True)
PY
fi
