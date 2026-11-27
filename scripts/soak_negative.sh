#!/usr/bin/env bash
# Negative gates: plant throwaway faults in a TEMP copy of emitted C, prove the
# soak / sanitizer gates FAIL. Never modifies product sources.
#
# Usage:
#   scripts/soak_negative.sh durable-leak          # expect soak FAIL (4 KiB/req)
#   scripts/soak_negative.sh slow-leak-keepalive   # expect soak FAIL (~1 KiB/10k, keep-alive)
#   scripts/soak_negative.sh slow-leak-churn       # expect soak FAIL (~1 KiB/10k, churn)
#   scripts/soak_negative.sh arena-free            # expect ASan FAIL
#   scripts/soak_negative.sh conn-leak             # expect churn FAIL (4 KiB/conn)
#   scripts/soak_negative.sh all                   # all negatives (default)
#
# Slow-leak soak wall clock: SLOW_SOAK_SECONDS (default 300). Plant rate is fixed at
# 1024 bytes malloc+memset every 10_000 requests or connections (temp .c only).
#
# Exit 0 only when the planted fault is detected (gate correctly fails).
set -euo pipefail

MODE="${1:-all}"
REPO="$(cd "$(dirname "$0")/.." && pwd)"
SUPER_BIN="${SUPER_BIN:-$REPO/target/release/super}"
STD="${SUPER_STD:-$REPO/stdlib}"
PORT_BASE="${PORT_BASE:-3310}"
SLOW_LEAK_PERIOD="${SLOW_LEAK_PERIOD:-10000}"
SLOW_LEAK_BYTES="${SLOW_LEAK_BYTES:-1024}"
SLOW_SOAK_SECONDS="${SLOW_SOAK_SECONDS:-300}"

# Shared post-warmup evaluator (same thresholds as soak_http.sh; never weakened).
run_flat_rss_eval() {
  local port=$1 pid=$2 churn=$3 seconds=$4 tag=$5
  python3 - "$port" "$pid" "$churn" "$seconds" "$tag" <<'PY'
import socket, sys, time
port = int(sys.argv[1]); pid = int(sys.argv[2])
churn = sys.argv[3] == "1"
seconds = float(sys.argv[4]); tag = sys.argv[5]

def rss():
    for line in open(f"/proc/{pid}/status"):
        if line.startswith("VmRSS:"):
            return int(line.split()[1])
    return 0

conn_hdr = "close" if churn else "keep-alive"
raw = f"GET /ping HTTP/1.1\r\nHost: x\r\nConnection: {conn_hdr}\r\n\r\n".encode()

def open_f():
    s = socket.create_connection(("127.0.0.1", port), 5)
    return s.makefile("rwb", buffering=0), s

def one(f):
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
end = t0 + seconds
reqs = 0
rows = []
rss0 = rss()
unit = "conns" if churn else "reqs"
while time.time() < end:
    try:
        if churn:
            f, s = open_f()
            one(f)
            s.close()
            f, s = None, None
        else:
            one(f)
    except Exception:
        if churn:
            pass
        else:
            try: s.close()
            except Exception: pass
            f, s = open_f()
            one(f)
    reqs += 1
    now = time.time()
    last_t = rows[-1][0] if rows else -1e9
    if reqs == 1 or (now - t0) - last_t >= 5.0:
        r = rss()
        rows.append((now - t0, reqs, r))
        print(f"t={now-t0:.1f}s {unit}={reqs} rss={r} KiB", flush=True)
if s is not None:
    try: s.close()
    except Exception: pass
rss1 = rss()
post = [r for r in rows if r[0] >= 60.0]
if len(post) < 3:
    print(f"FAIL: not enough post-warmup samples ({tag})", flush=True)
    sys.exit(2)
xs = [p[1] for p in post]; ys = [p[2] for p in post]
mx = sum(xs)/len(xs); my = sum(ys)/len(ys)
num = sum((x-mx)*(y-my) for x,y in zip(xs,ys))
den = sum((x-mx)**2 for x in xs) or 1.0
slope_10k = (num/den) * 10000
mid = ys[len(ys)//2] or 1
spread = (max(ys) - min(ys)) / mid * 100.0
delta = rss1 - rss0
print(
    f"{tag} {unit}_total={reqs} rss0={rss0} rss1={rss1} "
    f"delta_kib={delta} spread_pct={spread:.4f} slope_kib_per_10k={slope_10k:.6f}",
    flush=True,
)
ok_spread = spread <= 1.0
ok_slope = slope_10k < 1.0
if ok_spread and ok_slope:
    print(f"UNEXPECTED_PASS: soak gate stayed green ({tag})", flush=True)
    print("FAIL", flush=True)
    sys.exit(1)
print(f"EXPECTED_FAIL: soak gate caught planted leak ({tag})", flush=True)
print("FAIL", flush=True)
sys.exit(0)
PY
}

run_durable_leak() {
  local work port
  work=$(mktemp -d)
  port=$((PORT_BASE + 1))
  trap 'kill -INT ${SPID:-} 2>/dev/null || true; rm -rf "$work"' RETURN
  cat > "$work/neg.sr" <<'EOF'
use std.net.http
use std.sys.env
+main() {
    addr = env_get_or("SUPER_ADDR", "127.0.0.1:3311")
    app = App.new()
    app = @app.get("/ping", |_req| { ok("pong") })
    app = @app.post("/echo", |req| { ok(req.body) })
    app = @app.get("/tick", |_req| { ok("t") })
    cfg = ServeConfig.defaults()
    cfg.max_requests = 10000000
    @app.serve_with(addr, cfg) or 1
}
EOF
  echo "== durable-leak: build with --keep-c =="
  ( cd "$work" && SUPER_STD="$STD" "$SUPER_BIN" build --release --keep-c neg.sr -o "$work/neg" )
  local cfile
  cfile=$(find "$work" -name '*.c' | head -1)
  [[ -n "$cfile" ]] || { echo "FAIL: no keep-c output"; return 1; }
  # Plant ONLY in this temp .c: leak 1 KiB durable malloc on every request leave.
  python3 - "$cfile" <<'PY'
import pathlib, sys
p = pathlib.Path(sys.argv[1])
text = p.read_text()
# Plant at the top of leave (before the NESTED early-return). Nested leaves are
# still invoked on the App.serve path; skipping them keeps RSS flat and makes
# the gate look blind. Touch pages with memset so glibc commits RSS.
plant_body = (
    "  /* NEGATIVE-TEST ONLY — durable leak plant; not in product sources */\n"
    "  {\n"
    "    static volatile void *neg_leak_sink;\n"
    "    void *leak = malloc(4096);\n"
    "    if (leak) { memset(leak, 0xA5, 4096); neg_leak_sink = leak; }\n"
    "  }\n"
)
needle = "static inline void sup_req_leave(size_t mark) {"
if needle not in text:
    raise SystemExit("cannot find sup_req_leave to plant leak")
text = text.replace(needle, needle + "\n" + plant_body, 1)
p.write_text(text)
print(f"planted durable malloc(4096)+memset (pre-NESTED) in {p.name}")
PY
  echo "== recompile planted binary =="
  ( cd "$(dirname "$cfile")" && cc -std=gnu23 -O0 -g -o "$work/neg_leaky" "$(basename "$cfile")" -lm -lpthread )
  echo "== short soak against leaky binary (must FAIL slope / RSS growth) =="
  SUPER_ADDR="127.0.0.1:${port}" "$work/neg_leaky" >/dev/null 2>&1 &
  SPID=$!
  for _ in $(seq 1 200); do
    if curl -s -o /dev/null --max-time 0.2 "http://127.0.0.1:${port}/ping"; then break; fi
    if ! kill -0 "$SPID" 2>/dev/null; then
      echo "FAIL: leaky server exited early" >&2
      return 1
    fi
    sleep 0.05
  done
  set +e
  python3 - "$port" "$SPID" <<'PY'
import socket, sys, time
port = int(sys.argv[1]); pid = int(sys.argv[2])

def rss():
    for line in open(f"/proc/{pid}/status"):
        if line.startswith("VmRSS:"):
            return int(line.split()[1])
    return 0

raw = b"GET /ping HTTP/1.1\r\nHost: x\r\nConnection: keep-alive\r\n\r\n"

def open_f():
    s = socket.create_connection(("127.0.0.1", port), 5)
    return s.makefile("rwb", buffering=0), s

def one(f):
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
t0 = time.time()
# Same thresholds as soak_http.sh (60s warm-up, ≥5s samples, spread≤1%, slope<1 KiB/10k).
end = t0 + 90.0
reqs = 0
rows = []
rss0 = rss()
while time.time() < end:
    try:
        one(f)
    except Exception:
        try: s.close()
        except Exception: pass
        f, s = open_f()
        one(f)
    reqs += 1
    now = time.time()
    last_t = rows[-1][0] if rows else -1e9
    if reqs == 1 or (now - t0) - last_t >= 5.0:
        r = rss()
        rows.append((now - t0, reqs, r))
        print(f"t={now-t0:.1f}s reqs={reqs} rss={r} KiB", flush=True)
        # Early stop once growth is unambiguous (saves CI RAM/time).
        if r - rss0 > 64 * 1024 and len([x for x in rows if x[0] >= 60.0]) >= 3:
            break
s.close()
rss1 = rss()
post = [r for r in rows if r[0] >= 60.0]
if len(post) < 3:
    # If we grew hard before warm-up ended, still treat as gate catch.
    if rss1 - rss0 > 64 * 1024:
        print(
            f"negative_durable_leak requests_total={reqs} rss0={rss0} rss1={rss1} "
            f"delta_kib={rss1 - rss0} (early catch before full warm-up)",
            flush=True,
        )
        print("EXPECTED_FAIL: soak gate caught durable leak", flush=True)
        sys.exit(0)
    print("FAIL: not enough post-warmup samples (negative harness)", flush=True)
    sys.exit(2)
xs = [p[1] for p in post]; ys = [p[2] for p in post]
mx = sum(xs)/len(xs); my = sum(ys)/len(ys)
num = sum((x-mx)*(y-my) for x,y in zip(xs,ys))
den = sum((x-mx)**2 for x in xs) or 1.0
slope_10k = (num/den) * 10000
mid = ys[len(ys)//2] or 1
spread = (max(ys) - min(ys)) / mid * 100.0
delta = rss1 - rss0
print(
    f"negative_durable_leak requests_total={reqs} rss0={rss0} rss1={rss1} "
    f"delta_kib={delta} spread_pct={spread:.4f} slope_kib_per_10k={slope_10k:.6f}",
    flush=True,
)
ok_spread = spread <= 1.0
ok_slope = slope_10k < 1.0
# Absolute growth backup: a touched 4 KiB/req leak must move RSS by >> 1 KiB/10k.
if ok_spread and ok_slope and delta < 1024:
    print("UNEXPECTED_PASS: soak gate did not catch durable leak", flush=True)
    sys.exit(1)
print("EXPECTED_FAIL: soak gate caught durable leak", flush=True)
sys.exit(0)
PY
  local rc=$?
  set -e
  kill -INT "$SPID" 2>/dev/null || true
  wait "$SPID" 2>/dev/null || true
  if [[ $rc -eq 0 ]]; then
    echo "PASS negative durable-leak (gate failed as required)"
    return 0
  fi
  echo "FAIL negative durable-leak (gate stayed green or harness error rc=$rc)" >&2
  return 1
}

run_arena_free() {
  local work port
  work=$(mktemp -d)
  port=$((PORT_BASE + 2))
  trap 'kill -INT ${SPID:-} 2>/dev/null || true; rm -rf "$work"' RETURN
  REAL_CC="$(command -v clang || command -v gcc || command -v cc)"
  mkdir -p "$work/bin"
  cat > "$work/bin/cc" <<WRAP
#!/usr/bin/env bash
exec "$REAL_CC" -fsanitize=address,undefined -O1 -g -fno-omit-frame-pointer -fno-sanitize-recover=all "\$@"
WRAP
  chmod +x "$work/bin/cc"
  ln -sf cc "$work/bin/gcc"
  ln -sf cc "$work/bin/clang"
  export PATH="$work/bin:$PATH"
  cat > "$work/neg.sr" <<'EOF'
use std.net.http
use std.sys.env
+main() {
    addr = env_get_or("SUPER_ADDR", "127.0.0.1:3312")
    app = App.new()
    app = @app.get("/ping", |_req| { ok("pong") })
    cfg = ServeConfig.defaults()
    cfg.max_requests = 10000000
    @app.serve_with(addr, cfg) or 1
}
EOF
  echo "== arena-free: sanitized build with --keep-c =="
  ( cd "$work" && SUPER_STD="$STD" "$SUPER_BIN" build -d --keep-c neg.sr -o "$work/neg" )
  local cfile
  cfile=$(find "$work" -name '*.c' | head -1)
  [[ -n "$cfile" ]] || { echo "FAIL: no keep-c output"; return 1; }
  python3 - "$cfile" <<'PY'
import pathlib, sys
p = pathlib.Path(sys.argv[1])
text = p.read_text()
# Plant: free a request-arena pointer (ownership mix) at leave.
needle = "static inline void sup_req_leave(size_t mark) {"
plant = (
    needle
    + "\n  /* NEGATIVE-TEST ONLY — free arena pointer; not in product */\n"
    + "  if (sup_req.depth && sup_req.base) { free(sup_req.base); }"
)
if needle not in text:
    raise SystemExit("cannot find sup_req_leave")
text = text.replace(needle, plant, 1)
p.write_text(text)
print(f"planted free(sup_req.base) in {p.name}")
PY
  ( cd "$(dirname "$cfile")" && cc -std=gnu23 -fsanitize=address,undefined -O1 -g -fno-omit-frame-pointer \
      -fno-sanitize-recover=all -o "$work/neg_asan" "$(basename "$cfile")" -lm -lpthread )
  export ASAN_OPTIONS="detect_leaks=0:halt_on_error=1:abort_on_error=1:exitcode=42"
  export UBSAN_OPTIONS="halt_on_error=1:print_stacktrace=1:exitcode=42"
  echo "== drive a few requests (sanitizer must FAIL) =="
  set +e
  SUPER_ADDR="127.0.0.1:${port}" "$work/neg_asan" >"$work/server.log" 2>&1 &
  SPID=$!
  sleep 0.5
  for i in $(seq 1 20); do
    curl -s -o /dev/null --max-time 0.5 "http://127.0.0.1:${port}/ping" || true
    if ! kill -0 "$SPID" 2>/dev/null; then
      break
    fi
  done
  wait "$SPID" 2>/dev/null
  local rc=$?
  set -e
  if grep -Eiq 'AddressSanitizer|ERROR:|heap-buffer|attempting free|SEGV|Abort' "$work/server.log" \
     || [[ $rc -ne 0 ]]; then
    echo "EXPECTED_FAIL: sanitizer/arena-free plant detected (rc=$rc)"
    echo "PASS negative arena-free"
    return 0
  fi
  echo "FAIL negative arena-free: server survived planted free" >&2
  cat "$work/server.log" >&2 || true
  return 1
}

run_conn_leak() {
  # Per-connection durable leak under Connection: close churn. Must make the
  # churn soak slope/spread gate FAIL. Plant lives only in a throwaway .c copy.
  local work port
  work=$(mktemp -d)
  port=$((PORT_BASE + 3))
  trap 'kill -INT ${SPID:-} 2>/dev/null || true; rm -rf "$work"' RETURN
  cat > "$work/neg.sr" <<'EOF'
use std.net.http
use std.sys.env
+main() {
    addr = env_get_or("SUPER_ADDR", "127.0.0.1:3313")
    app = App.new()
    app = @app.get("/ping", |_req| { ok("pong") })
    app = @app.post("/echo", |req| { ok(req.body) })
    app = @app.get("/tick", |_req| { ok("t") })
    cfg = ServeConfig.defaults()
    cfg.max_requests = 10000000
    @app.serve_with(addr, cfg) or 1
}
EOF
  echo "== conn-leak: build with --keep-c =="
  ( cd "$work" && SUPER_STD="$STD" "$SUPER_BIN" build --release --keep-c neg.sr -o "$work/neg" )
  local cfile
  cfile=$(find "$work" -name '*.c' | head -1)
  [[ -n "$cfile" ]] || { echo "FAIL: no keep-c output"; return 1; }
  python3 - "$cfile" <<'PY'
import pathlib, sys
p = pathlib.Path(sys.argv[1])
text = p.read_text()
needle = "static inline size_t sup_conn_enter(void) {"
plant = (
    needle
    + "\n  /* NEGATIVE-TEST ONLY — per-connection durable leak; not in product */\n"
    + "  {\n"
    + "    static volatile void *neg_conn_sink;\n"
    + "    void *leak = malloc(4096);\n"
    + "    if (leak) { memset(leak, 0xC3, 4096); neg_conn_sink = leak; }\n"
    + "  }\n"
)
if needle not in text:
    raise SystemExit("cannot find sup_conn_enter to plant conn leak")
text = text.replace(needle, plant, 1)
p.write_text(text)
print(f"planted durable malloc(4096)+memset in {p.name} (sup_conn_enter)")
PY
  echo "== recompile planted churn binary =="
  ( cd "$(dirname "$cfile")" && cc -std=gnu23 -O0 -g -o "$work/neg_conn_leaky" "$(basename "$cfile")" -lm -lpthread )
  echo "== short churn soak against conn-leaky binary (must FAIL) =="
  SUPER_ADDR="127.0.0.1:${port}" "$work/neg_conn_leaky" >/dev/null 2>&1 &
  SPID=$!
  for _ in $(seq 1 200); do
    if curl -s -o /dev/null --max-time 0.2 "http://127.0.0.1:${port}/ping"; then break; fi
    if ! kill -0 "$SPID" 2>/dev/null; then
      echo "FAIL: conn-leaky server exited early" >&2
      return 1
    fi
    sleep 0.05
  done
  set +e
  python3 - "$port" "$SPID" <<'PY'
import socket, sys, time
port = int(sys.argv[1]); pid = int(sys.argv[2])

def rss():
    for line in open(f"/proc/{pid}/status"):
        if line.startswith("VmRSS:"):
            return int(line.split()[1])
    return 0

raw = b"GET /ping HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"

def one():
    s = socket.create_connection(("127.0.0.1", port), 5)
    f = s.makefile("rwb", buffering=0)
    f.write(raw); f.flush()
    status = f.readline(); cl = 0
    while True:
        line = f.readline()
        if line in (b"\r\n", b"\n", b""):
            break
        if line.lower().startswith(b"content-length:"):
            cl = int(line.split(b":")[1])
    f.read(cl)
    s.close()
    if not status.startswith(b"HTTP/1.1 200"):
        raise RuntimeError(status)

t0 = time.time()
end = t0 + 90.0
reqs = 0
rows = []
rss0 = rss()
while time.time() < end:
    one()
    reqs += 1
    now = time.time()
    last_t = rows[-1][0] if rows else -1e9
    if reqs == 1 or (now - t0) - last_t >= 5.0:
        r = rss()
        rows.append((now - t0, reqs, r))
        print(f"t={now-t0:.1f}s conns={reqs} rss={r} KiB", flush=True)
        if r - rss0 > 64 * 1024 and len([x for x in rows if x[0] >= 60.0]) >= 3:
            break
rss1 = rss()
post = [r for r in rows if r[0] >= 60.0]
if len(post) < 3:
    if rss1 - rss0 > 64 * 1024:
        print(
            f"negative_conn_leak connections_total={reqs} rss0={rss0} rss1={rss1} "
            f"delta_kib={rss1 - rss0} (early catch)",
            flush=True,
        )
        print("EXPECTED_FAIL: churn soak gate caught per-connection leak", flush=True)
        sys.exit(0)
    print("FAIL: not enough post-warmup samples (conn-leak harness)", flush=True)
    sys.exit(2)
xs = [p[1] for p in post]; ys = [p[2] for p in post]
mx = sum(xs)/len(xs); my = sum(ys)/len(ys)
num = sum((x-mx)*(y-my) for x,y in zip(xs,ys))
den = sum((x-mx)**2 for x in xs) or 1.0
slope_10k = (num/den) * 10000
mid = ys[len(ys)//2] or 1
spread = (max(ys) - min(ys)) / mid * 100.0
delta = rss1 - rss0
print(
    f"negative_conn_leak connections_total={reqs} rss0={rss0} rss1={rss1} "
    f"delta_kib={delta} spread_pct={spread:.4f} slope_kib_per_10k={slope_10k:.6f}",
    flush=True,
)
if spread <= 1.0 and slope_10k < 1.0 and delta < 1024:
    print("UNEXPECTED_PASS: churn gate did not catch per-connection leak", flush=True)
    sys.exit(1)
print("EXPECTED_FAIL: churn soak gate caught per-connection leak", flush=True)
sys.exit(0)
PY
  local rc=$?
  set -e
  kill -INT "$SPID" 2>/dev/null || true
  wait "$SPID" 2>/dev/null || true
  if [[ $rc -eq 0 ]]; then
    echo "PASS negative conn-leak (gate failed as required)"
    return 0
  fi
  echo "FAIL negative conn-leak (gate stayed green or harness error rc=$rc)" >&2
  return 1
}

_neg_build_server() {
  local work=$1 port=$2 addr_suffix=$3
  cat > "$work/neg.sr" <<EOF
use std.net.http
use std.sys.env
+main() {
    addr = env_get_or("SUPER_ADDR", "127.0.0.1:${addr_suffix}")
    app = App.new()
    app = @app.get("/ping", |_req| { ok("pong") })
    app = @app.post("/echo", |req| { ok(req.body) })
    app = @app.get("/tick", |_req| { ok("t") })
    cfg = ServeConfig.defaults()
    cfg.max_requests = 10000000
    @app.serve_with(addr, cfg) or 1
}
EOF
  echo "== build negative server (--keep-c) ==" >&2
  ( cd "$work" && SUPER_STD="$STD" "$SUPER_BIN" build --release --keep-c neg.sr -o "$work/neg" ) >&2
  local cfile
  cfile=$(find "$work" -name '*.c' | head -1)
  if [[ -z "$cfile" ]]; then
    echo "FAIL: no keep-c output" >&2
    return 1
  fi
  echo "$cfile"
}

run_slow_leak_keepalive() {
  local work port period bytes
  work=$(mktemp -d)
  port=$((PORT_BASE + 4))
  period="$SLOW_LEAK_PERIOD"
  bytes="$SLOW_LEAK_BYTES"
  trap 'kill -INT ${SPID:-} 2>/dev/null || true; rm -rf "$work"' RETURN
  echo "== slow-leak-keepalive: plant ${bytes} B every ${period} requests (${SLOW_SOAK_SECONDS}s soak) =="
  local cfile
  cfile=$(_neg_build_server "$work" "$port" "${port}") || return 1
  python3 - "$cfile" "$period" "$bytes" "req" <<'PY'
import pathlib, sys
p = pathlib.Path(sys.argv[1])
period = int(sys.argv[2]); nbytes = int(sys.argv[3]); kind = sys.argv[4]
text = p.read_text()
needle = "static inline void sup_req_leave(size_t mark) {"
plant = (
    needle
    + f"\n  /* NEGATIVE-TEST ONLY — ~{nbytes} B / {period} {kind}s; not in product */\n"
    + "  {\n"
    + f"    static unsigned long neg_slow_{kind}_ctr;\n"
    + f"    static volatile void *neg_slow_{kind}_sink;\n"
    + f"    neg_slow_{kind}_ctr++;\n"
    + f"    if ((neg_slow_{kind}_ctr % {period}u) == 0u) {{\n"
    + f"      void *leak = malloc({nbytes});\n"
    + f"      if (leak) {{ memset(leak, 0xB7, {nbytes}); neg_slow_{kind}_sink = leak; }}\n"
    + "    }\n"
    + "  }\n"
)
if needle not in text:
    raise SystemExit("cannot find sup_req_leave for slow keep-alive leak")
text = text.replace(needle, plant, 1)
p.write_text(text)
print(f"planted slow leak malloc({nbytes}) every {period} requests in {p.name}")
PY
  ( cd "$(dirname "$cfile")" && cc -std=gnu23 -O0 -g -o "$work/neg_slow_ka" "$(basename "$cfile")" -lm -lpthread )
  SUPER_ADDR="127.0.0.1:${port}" "$work/neg_slow_ka" >/dev/null 2>&1 &
  SPID=$!
  for _ in $(seq 1 200); do
    if curl -s -o /dev/null --max-time 0.2 "http://127.0.0.1:${port}/ping"; then break; fi
    if ! kill -0 "$SPID" 2>/dev/null; then
      echo "FAIL: slow keep-alive server exited early" >&2
      return 1
    fi
    sleep 0.05
  done
  set +e
  run_flat_rss_eval "$port" "$SPID" 0 "$SLOW_SOAK_SECONDS" "negative_slow_leak_keepalive"
  local rc=$?
  set -e
  kill -INT "$SPID" 2>/dev/null || true
  wait "$SPID" 2>/dev/null || true
  if [[ $rc -eq 0 ]]; then
    echo "PASS negative slow-leak-keepalive (gate failed as required)"
    echo "plant_rate=${bytes}_bytes_per_${period}_requests"
    return 0
  fi
  echo "FAIL negative slow-leak-keepalive (gate stayed green or harness error rc=$rc)" >&2
  return 1
}

run_slow_leak_churn() {
  local work port period bytes
  work=$(mktemp -d)
  port=$((PORT_BASE + 5))
  period="$SLOW_LEAK_PERIOD"
  bytes="$SLOW_LEAK_BYTES"
  trap 'kill -INT ${SPID:-} 2>/dev/null || true; rm -rf "$work"' RETURN
  echo "== slow-leak-churn: plant ${bytes} B every ${period} connections (${SLOW_SOAK_SECONDS}s soak) =="
  local cfile
  cfile=$(_neg_build_server "$work" "$port" "${port}") || return 1
  python3 - "$cfile" "$period" "$bytes" "conn" <<'PY'
import pathlib, sys
p = pathlib.Path(sys.argv[1])
period = int(sys.argv[2]); nbytes = int(sys.argv[3]); kind = sys.argv[4]
text = p.read_text()
needle = "static inline size_t sup_conn_enter(void) {"
plant = (
    needle
    + f"\n  /* NEGATIVE-TEST ONLY — ~{nbytes} B / {period} {kind}s; not in product */\n"
    + "  {\n"
    + f"    static unsigned long neg_slow_{kind}_ctr;\n"
    + f"    static volatile void *neg_slow_{kind}_sink;\n"
    + f"    neg_slow_{kind}_ctr++;\n"
    + f"    if ((neg_slow_{kind}_ctr % {period}u) == 0u) {{\n"
    + f"      void *leak = malloc({nbytes});\n"
    + f"      if (leak) {{ memset(leak, 0xC7, {nbytes}); neg_slow_{kind}_sink = leak; }}\n"
    + "    }\n"
    + "  }\n"
)
if needle not in text:
    raise SystemExit("cannot find sup_conn_enter for slow churn leak")
text = text.replace(needle, plant, 1)
p.write_text(text)
print(f"planted slow leak malloc({nbytes}) every {period} connections in {p.name}")
PY
  ( cd "$(dirname "$cfile")" && cc -std=gnu23 -O0 -g -o "$work/neg_slow_churn" "$(basename "$cfile")" -lm -lpthread )
  SUPER_ADDR="127.0.0.1:${port}" "$work/neg_slow_churn" >/dev/null 2>&1 &
  SPID=$!
  for _ in $(seq 1 200); do
    if curl -s -o /dev/null --max-time 0.2 "http://127.0.0.1:${port}/ping"; then break; fi
    if ! kill -0 "$SPID" 2>/dev/null; then
      echo "FAIL: slow churn server exited early" >&2
      return 1
    fi
    sleep 0.05
  done
  set +e
  run_flat_rss_eval "$port" "$SPID" 1 "$SLOW_SOAK_SECONDS" "negative_slow_leak_churn"
  local rc=$?
  set -e
  kill -INT "$SPID" 2>/dev/null || true
  wait "$SPID" 2>/dev/null || true
  if [[ $rc -eq 0 ]]; then
    echo "PASS negative slow-leak-churn (gate failed as required)"
    echo "plant_rate=${bytes}_bytes_per_${period}_connections"
    return 0
  fi
  echo "FAIL negative slow-leak-churn (gate stayed green or harness error rc=$rc)" >&2
  return 1
}

case "$MODE" in
  durable-leak) run_durable_leak ;;
  slow-leak-keepalive) run_slow_leak_keepalive ;;
  slow-leak-churn) run_slow_leak_churn ;;
  arena-free) run_arena_free ;;
  conn-leak) run_conn_leak ;;
  slow-leak-all)
    run_slow_leak_keepalive
    run_slow_leak_churn
    echo "PASS slow-leak negatives (keep-alive + churn gates failed as required)"
    ;;
  all)
    run_durable_leak
    run_slow_leak_keepalive
    run_slow_leak_churn
    run_arena_free
    run_conn_leak
    echo "PASS all negative gates (all planted faults detected)"
    ;;
  *)
    echo "usage: $0 [durable-leak|slow-leak-keepalive|slow-leak-churn|slow-leak-all|arena-free|conn-leak|all]" >&2
    exit 2
    ;;
esac
