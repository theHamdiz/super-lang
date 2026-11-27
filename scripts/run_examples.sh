#!/usr/bin/env bash
# Run documented examples/doc_* programs; exit non-zero on first failure.
set -euo pipefail
ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
SUPER="${SUPER:-$ROOT/target/release/super}"
if [[ ! -x "$SUPER" ]]; then
  echo "building super CLI…" >&2
  cargo build -p super-cli --release --manifest-path "$ROOT/Cargo.toml"
  SUPER="$ROOT/target/release/super"
fi
cd "$ROOT"
run() { echo "== $*"; "$SUPER" "$@"; }
run run examples/doc_hello/hello.sr
run run examples/doc_data_stack/main.sr
run check examples/doc_comptime/main.sr
run check examples/doc_pod/main.sr
run test examples/doc_test/
run run examples/doc_errors/main.sr
run check examples/doc_http_json/main.sr
run check examples/doc_ai/main.sr
run test examples/doc_ai/main.sr
run verify examples/04_law_verify/src/main.sr
echo "OK: all doc examples passed"