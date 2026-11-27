#!/usr/bin/env bash
# Negative: plant a wrongly classified Stay (arena birth for a Promote site that
# escapes into durable `g_escape`). After request leave, reading that durable
# root must trip ASan / abort. Plant lives only in a throwaway .c copy.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
SUPER_BIN="${SUPER_BIN:-$REPO/target/release/super}"
STD="${SUPER_STD:-$REPO/stdlib}"
PORT="${PORT:-3183}"
ESCAPE_FIXTURE="${ESCAPE_FIXTURE:-}"
WORKDIR=$(mktemp -d)
trap 'kill -INT ${SPID:-} 2>/dev/null || true; rm -rf "$WORKDIR"' EXIT

if [[ -n "$ESCAPE_FIXTURE" ]]; then
  echo "FAIL: ESCAPE_FIXTURE override not supported for wrong-Stay (needs embedded g_escape route)" >&2
  exit 2
fi

REAL_CC="$(command -v clang || command -v gcc || command -v cc || true)"
[[ -n "$REAL_CC" ]] || { echo "FAIL: need cc with ASan" >&2; exit 2; }
mkdir -p "$WORKDIR/bin"
cat > "$WORKDIR/bin/cc" <<WRAP
#!/usr/bin/env bash
exec "$REAL_CC" -fsanitize=address,undefined -O1 -g -fno-omit-frame-pointer -fno-sanitize-recover=all "\$@"
WRAP
chmod +x "$WORKDIR/bin/cc"
ln -sf cc "$WORKDIR/bin/gcc"
ln -sf cc "$WORKDIR/bin/clang"
export PATH="$WORKDIR/bin:$PATH"

cat > "$WORKDIR/app.sr" <<'EOF'
use std.net.http
use std.sys.env

state g_escape = {note: "init"}

+main() {
    addr = env_get_or("SUPER_ADDR", "127.0.0.1:3183")
    app = App.new()
    app = @app.get("/ping", |_req| { ok("pong") })
    app = @app.get("/last", |_req| { ok(g_escape) })
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

echo "== wrong-Stay: sanitized keep-c build =="
( cd "$WORKDIR" && SUPER_STD="$STD" "$SUPER_BIN" build -d --keep-c app.sr -o "$WORKDIR/app" )
CFILE=$(find "$WORKDIR" -name '*.c' | head -1)
[[ -n "$CFILE" ]] || { echo "FAIL: no keep-c" >&2; exit 1; }

python3 - "$CFILE" <<'PY'
import pathlib, sys, re
p = pathlib.Path(sys.argv[1])
text = p.read_text()
# Promote site for the escaping `{note: …}` is emitted as alloc_durable(1, …).
# Force it to Stay (request arena) — the classification bug under test.
pat = re.compile(
    r"super_obj_alloc_durable\(1, 0, NULL\)",
    re.M,
)
m = pat.search(text)
if not m:
    raise SystemExit("cannot find Promote alloc_durable(1, …) to plant wrong Stay")
plant = "super_obj_alloc_region(1, 0, NULL, (uint8_t)SUP_REG_REQ)"
text = pat.sub(plant, text, count=1)
p.write_text(text)
print(f"planted wrong Stay (REQ) over Promote durable in {p.name}")
PY

( cd "$(dirname "$CFILE")" && cc -std=gnu23 -fsanitize=address,undefined -O1 -g -fno-omit-frame-pointer \
    -fno-sanitize-recover=all -o "$WORKDIR/app_asan" "$(basename "$CFILE")" -lm -lpthread )

export ASAN_OPTIONS="detect_leaks=0:halt_on_error=1:abort_on_error=1:exitcode=42"
export UBSAN_OPTIONS="halt_on_error=1:print_stacktrace=1:exitcode=42"

echo "== drive /alloc then /last (sanitizer must FAIL) =="
set +e
SUPER_ADDR="127.0.0.1:${PORT}" "$WORKDIR/app_asan" >"$WORKDIR/server.log" 2>&1 &
SPID=$!
for _ in $(seq 1 200); do
  curl -s -o /dev/null --max-time 0.2 "http://127.0.0.1:${PORT}/ping" && break
  sleep 0.05
done
curl -s --max-time 2 -X POST -d '{}' "http://127.0.0.1:${PORT}/alloc" >/dev/null || true
# Fresh connection after leave — durable root still holds arena pointer if plant worked.
curl -s --max-time 2 "http://127.0.0.1:${PORT}/last" >/dev/null || true
for _ in $(seq 1 30); do
  curl -s --max-time 0.5 -X POST -d '{}' "http://127.0.0.1:${PORT}/alloc" >/dev/null || true
  curl -s --max-time 0.5 "http://127.0.0.1:${PORT}/last" >/dev/null || true
  if ! kill -0 "$SPID" 2>/dev/null; then break; fi
done
# Bound wait: survival after the drive window is a FAIL for this negative.
kill -INT "$SPID" 2>/dev/null || true
wait "$SPID" 2>/dev/null
rc=$?
set -e
if grep -Eiq 'AddressSanitizer|ERROR:|heap-buffer|use-after|SEGV|Abort|super:.*region|arena|INVALID' "$WORKDIR/server.log" \
   || [[ $rc -ne 0 ]]; then
  echo "EXPECTED_FAIL: wrong-Stay plant detected (rc=$rc)"
  echo "PASS escape wrong-Stay ASan"
  exit 0
fi
echo "FAIL escape wrong-Stay: server survived planted Stay-on-Promote" >&2
cat "$WORKDIR/server.log" >&2 || true
exit 1
