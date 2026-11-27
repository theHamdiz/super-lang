#!/usr/bin/env bash
# Short RSS soak for release-bundle CI (AOT hello server `new.sr`).
# Full flat-RSS gate: `scripts/soak_http.sh` (Super driver + multi-route mix).
#
# Env:
#   SUPER_SOAK_REQUESTS (default 10000)
#   SUPER_SOAK_FULL=1     → 1_000_000 requests
#   SUPER_SOAK_FALLBACK=1 → VM `super run` (skips spread assert)
#   SUPER_SOAK_RSS_SPREAD_KIB (default 524288)
#   SUPER_BIN, SUPER_STD
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
SUPER_BIN="${SUPER_BIN:-$REPO/target/release/super}"
STD="${SUPER_STD:-$REPO/stdlib}"

if [[ "${SUPER_SOAK_FULL:-}" =~ ^(1|true|yes)$ ]]; then
  N=1000000
else
  N="${SUPER_SOAK_REQUESTS:-10000}"
fi
SPREAD_BOUND="${SUPER_SOAK_RSS_SPREAD_KIB:-524288}"
FALLBACK="${SUPER_SOAK_FALLBACK:-}"

WORKDIR=$(mktemp -d)
trap 'rm -rf "$WORKDIR"' EXIT

cp "$REPO/benchmarks/server-builder/new.sr" "$WORKDIR/server.sr"
SAMPLES="$WORKDIR/rss_samples.tsv"
PORT=$((3275 + ($$ % 400)))

python3 - "$REPO" "$SUPER_BIN" "$STD" "$WORKDIR" "$N" "$SPREAD_BOUND" "$FALLBACK" "$PORT" "$SAMPLES" <<'PY'
import os, signal, socket, subprocess, sys, time
from pathlib import Path

repo, super_bin, std, work = Path(sys.argv[1]), sys.argv[2], sys.argv[3], Path(sys.argv[4])
n = int(sys.argv[5])
spread_bound = int(sys.argv[6])
fallback = sys.argv[7].lower() in ("1", "true", "yes")
port = int(sys.argv[8])
samples_path = Path(sys.argv[9])

sample_every = max(1, n // 20)

def rss(pid: int) -> int:
    try:
        text = Path(f"/proc/{pid}/status").read_text()
    except OSError:
        return 0
    for line in text.splitlines():
        if line.startswith("VmRSS:"):
            return int(line.split()[1])
    return 0

def wait_listen() -> None:
    deadline = time.time() + 30
    while time.time() < deadline:
        try:
            socket.create_connection(("127.0.0.1", port), 0.2).close()
            time.sleep(0.1)
            return
        except OSError:
            time.sleep(0.02)
    raise SystemExit(f"server did not listen on {port}")

def ping() -> None:
    s = socket.create_connection(("127.0.0.1", port), 5)
    s.sendall(b"GET /ping HTTP/1.1\r\nHost: soak\r\nConnection: close\r\n\r\n")
    data = s.recv(65536)
    s.close()
    if not data.startswith(b"HTTP/1.1 200"):
        raise SystemExit(f"bad ping: {data[:80]!r}")

if fallback:
  print("http_rss_soak: SUPER_SOAK_FALLBACK=1 — VM run path (RSS growth expected; PARTIAL)", flush=True)
  env = os.environ.copy()
  env["SUPER_ADDR"] = f"127.0.0.1:{port}"
  env["SUPER_STD"] = std
  proc = subprocess.Popen(
      [super_bin, "run", str(work / "server.sr")],
      env=env,
      stdout=subprocess.DEVNULL,
      stderr=subprocess.DEVNULL,
  )
else:
  build = subprocess.run(
      [super_bin, "build", "--release", "server.sr"],
      cwd=work,
      env={**os.environ, "SUPER_STD": std},
      capture_output=True,
      text=True,
  )
  if build.returncode != 0:
    print(build.stderr, file=sys.stderr)
    raise SystemExit("AOT build failed")
  binary = work / "target" / "server"
  proc = subprocess.Popen(
      [str(binary)],
      env={**os.environ, "SUPER_ADDR": f"127.0.0.1:{port}"},
      stdout=subprocess.DEVNULL,
      stderr=subprocess.DEVNULL,
  )

wait_listen()
pid = proc.pid
rows: list[tuple[int, int]] = []
with samples_path.open("w") as out:
    out.write("request\trss_kib\n")
    for i in range(1, n + 1):
        ping()
        if i % sample_every == 0 or i == 1 or i == n:
            r = rss(pid)
            rows.append((i, r))
            out.write(f"{i}\t{r}\n")
proc.send_signal(signal.SIGINT)
proc.wait(timeout=30)

print(f"SUPER_SOAK_SAMPLES={samples_path}", flush=True)
discard = min(max(len(rows) * 10 // 100, 1), len(rows))
post = rows[discard:]
if not post:
    raise SystemExit("not enough RSS samples after warmup discard")
mn = min(r for _, r in post)
mx = max(r for _, r in post)
spread = mx - mn
print(
    f"http_rss_soak: n={n} samples={len(rows)} post_warmup={len(post)} "
    f"spread={spread} KiB (bound {spread_bound})",
    flush=True,
)
if fallback:
    print("http_rss_soak: skipping spread assertion on fallback run path (PARTIAL)", flush=True)
    sys.exit(0)
if spread > spread_bound:
    raise SystemExit(
        f"VmRSS spread {spread} KiB exceeds bound {spread_bound} KiB "
        f"(min={mn} max={mx}); samples at {samples_path}"
    )
PY