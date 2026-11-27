#!/usr/bin/env bash
# Pinned malloc/free probe for AOT App.serve (fresh GET /ping per connection).
# Invoked from CI via `cargo test --test alloc_probe` (thin wrapper) or directly.
#
# Pin: 0.00 alloc / 0.00 free per request (conn arena; not zero-alloc).
# Requires: Linux, cc, built `super` (SUPER_BIN).
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
SUPER_BIN="${SUPER_BIN:-$REPO/target/release/super}"
STD="${SUPER_STD:-$REPO/stdlib}"
PIN_ALLOCS="${ALLOC_PROBE_PIN_ALLOCS:-0}"
PIN_FREES="${ALLOC_PROBE_PIN_FREES:-0}"
TOL="${ALLOC_PROBE_TOL:-1.0}"

if [[ ! -x "$SUPER_BIN" ]]; then
  echo "FAIL: SUPER_BIN not executable: $SUPER_BIN" >&2
  exit 1
fi
if ! command -v cc >/dev/null 2>&1; then
  echo "skipping: no C compiler" >&2
  exit 0
fi

WORKDIR=$(mktemp -d)
trap 'rm -rf "$WORKDIR"' EXIT

cp "$REPO/benchmarks/server-builder/new.sr" "$WORKDIR/server.sr"
(
  cd "$WORKDIR"
  SUPER_STD="$STD" "$SUPER_BIN" build --release server.sr
)
BINARY="$WORKDIR/target/server"
SHIM="$WORKDIR/alloc_count.so"
cc -O1 -shared -fPIC -o "$SHIM" \
  "$REPO/crates/super-cli/tests/fixtures/alloc_count.c" -ldl

python3 - "$BINARY" "$SHIM" "$WORKDIR" "$PIN_ALLOCS" "$PIN_FREES" "$TOL" <<'PY'
import os, signal, socket, subprocess, sys, time
from pathlib import Path

binary = Path(sys.argv[1])
shim = sys.argv[2]
work = Path(sys.argv[3])
pin_a = float(sys.argv[4])
pin_f = float(sys.argv[5])
tol = float(sys.argv[6])

def measure(n: int, port: int) -> tuple[int, int]:
    out = work / f"count-{n}.txt"
    env = os.environ.copy()
    env["SUPER_ADDR"] = f"127.0.0.1:{port}"
    env["LD_PRELOAD"] = shim
    env["ALLOC_COUNT_OUT"] = str(out)
    proc = subprocess.Popen(
        [str(binary)],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    deadline = time.time() + 10
    while time.time() < deadline:
        try:
            socket.create_connection(("127.0.0.1", port), 0.2).close()
            break
        except OSError:
            time.sleep(0.02)
    else:
        proc.kill()
        raise SystemExit("server did not listen")
    time.sleep(0.05)
    for _ in range(n):
        s = socket.create_connection(("127.0.0.1", port), 5)
        s.sendall(b"GET /ping HTTP/1.1\r\nHost: probe\r\nConnection: close\r\n\r\n")
        buf = s.recv(65536)
        if not buf.startswith(b"HTTP/1.1 200"):
            proc.kill()
            raise SystemExit(f"bad reply: {buf[:80]!r}")
        s.close()
    proc.send_signal(signal.SIGINT)
    proc.wait(timeout=10)
    a, f = map(int, out.read_text().split()[:2])
    return a, f

n = 200
best = None
for attempt in range(3):
    p0 = 3191 + 2 * attempt
    a_n, f_n = measure(n, p0)
    a_2n, f_2n = measure(2 * n, p0 + 1)
    allocs = (a_2n - a_n) / n
    frees = (f_2n - f_n) / n
    print(f"attempt {attempt}: {allocs:.2f} allocations, {frees:.2f} frees per request", flush=True)
    if best is None or allocs < best[0]:
        best = (allocs, frees)
    if abs(allocs - pin_a) < tol:
        break

allocs, frees = best
print(f"per request: {allocs:.2f} allocations, {frees:.2f} frees", flush=True)
if abs(allocs - pin_a) >= tol:
    raise SystemExit(f"allocations per request changed: {allocs:.2f} (pinned {pin_a})")
if abs(frees - pin_f) >= tol:
    raise SystemExit(f"frees per request changed: {frees:.2f} (pinned {pin_f})")
PY