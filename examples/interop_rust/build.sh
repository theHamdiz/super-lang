#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
cargo test
cargo run -- "$(pwd)/src/ai.sr"
