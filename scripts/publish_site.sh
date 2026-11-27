#!/usr/bin/env bash
# Regenerate Super site → docs/, /tmp/bm_www, and ../mandara/super
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export SUPER_MANDARA_ROOT="${SUPER_MANDARA_ROOT:-$ROOT/../mandara}"
cd "$ROOT/benchmarks"
python3 report_page.py
echo "Site root: $ROOT/docs (GitHub Pages)"
echo "Preview:  python3 -m http.server 8765 --bind 127.0.0.1 --directory $ROOT/docs"
echo "Mandara:  python3 -m http.server 8765 --bind 127.0.0.1 --directory $SUPER_MANDARA_ROOT/super"
