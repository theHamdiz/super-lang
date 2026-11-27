#!/usr/bin/env bash
set -euo pipefail
ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
"$ROOT/scripts/run_examples.sh"
echo "OK: doc_examples (same as run_examples for 0.1.0)"