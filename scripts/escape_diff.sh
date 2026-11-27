#!/usr/bin/env bash
# Differential oracle: SUPER_PROMOTE_EVERYTHING=1 vs normal analysis must yield
# identical HTTP response bodies for the allocating /alloc route.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
SUPER_BIN="${SUPER_BIN:-$REPO/target/release/super}"
STD="${SUPER_STD:-$REPO/stdlib}"
PORT_A="${PORT_A:-3181}"
PORT_B="${PORT_B:-3182}"
# Optional: `ESCAPE_FIXTURE=examples/31_escape_service/src/main.sr` (must expose POST /alloc).
ESCAPE_FIXTURE="${ESCAPE_FIXTURE:-}"
WORKDIR=$(mktemp -d)
trap 'kill -INT ${SPID_A:-} ${SPID_B:-} 2>/dev/null || true; rm -rf "$WORKDIR"' EXIT

if [[ -n "$ESCAPE_FIXTURE" ]]; then
  cp "$REPO/$ESCAPE_FIXTURE" "$WORKDIR/app.sr"
else
cat > "$WORKDIR/app.sr" <<'EOF'
use std.net.http
use std.sys.env

state g_escape = {note: "init"}

+main() {
    addr = env_get_or("SUPER_ADDR", "127.0.0.1:3181")
    app = App.new()
    app = @app.get("/ping", |_req| { ok("pong") })
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
fi

echo "== build analysis-on server =="
( cd "$WORKDIR" && SUPER_STD="$STD" "$SUPER_BIN" build --release app.sr -o "$WORKDIR/app_on" )

echo "== build promote-everything oracle server =="
( cd "$WORKDIR" && SUPER_STD="$STD" "$SUPER_BIN" build --release --keep-c app.sr -o "$WORKDIR/app_oracle_tmp" )
# Same binary; oracle is a runtime env flag on the analysis-on build.
cp "$WORKDIR/app_on" "$WORKDIR/app_off"

echo "== start servers =="
SUPER_ADDR="127.0.0.1:${PORT_A}" "$WORKDIR/app_on" >/dev/null 2>&1 &
SPID_A=$!
SUPER_PROMOTE_EVERYTHING=1 SUPER_ADDR="127.0.0.1:${PORT_B}" "$WORKDIR/app_off" >/dev/null 2>&1 &
SPID_B=$!
for _ in $(seq 1 200); do
  curl -s -o /dev/null --max-time 0.2 "http://127.0.0.1:${PORT_A}/ping" && \
    curl -s -o /dev/null --max-time 0.2 "http://127.0.0.1:${PORT_B}/ping" && break
  sleep 0.05
done

BODY='{"msg":"diff-oracle"}'
A=$(curl -s --max-time 2 -X POST -d "$BODY" "http://127.0.0.1:${PORT_A}/alloc")
B=$(curl -s --max-time 2 -X POST -d "$BODY" "http://127.0.0.1:${PORT_B}/alloc")

echo "analysis_on_body=${A}"
echo "promote_everything_body=${B}"
if [[ "$A" == "$B" && -n "$A" ]]; then
  echo "PASS escape differential: identical /alloc bodies"
  exit 0
fi
echo "FAIL escape differential: bodies differ or empty" >&2
exit 1
