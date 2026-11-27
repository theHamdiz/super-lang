# VM vs JIT vs C23 checksum equivalence

Protocol 2.0.0 timed runs require identical algorithm checksums across Super backends unless classified under the Stage 0 drift policy (non-associative float, etc.).

Gate: `LAW-BENCH-001` + harness Gate B. See `benchmarks/laws/LAW-BENCH-001.md`.
