# Super roadmap (0.1.0 honesty)

No release dates — only priority lanes. Owners marked **open** until a maintainer is named.

## NOW (0.1.x maintenance)

| Gap | Status | Owner |
|---|---|---|
| HTTP `App.serve` request-scoped reclamation | AOT: 3 alloc / 0 free + flat RSS soak local; VM reclaim stubs; CI tip soak pending | **open** |
| Law/proof checker completeness | Real-body laws expanded; many modules still sidecar models | **open** |
| Windows C23 AOT sockets | Winsock2 implemented; Windows CI TCP job pending green on tip | **open** |
| Community benchmark submissions | Harness documented; `results/community/` intake via issue template | **open** |

## NEXT (needs design + CI before claims)

| Gap | Status | Owner |
|---|---|---|
| Windows CI network matrix (concurrent, 16 MiB, UDP, HTTP hello) | Loopback TCP smoke wired; full matrix open | **open** |
| Stdlib law coverage / real-body ratio | 40 modules with laws; ~12 real-body — raise ratio | **open** |
| Release artifact attestations (Sigstore) | `SHA256SUMS` attested on tag publish; per-archive attestations **PARTIAL** | **open** |
| Law mutation gate in required CI | Local 93.5% on priority; wire as required job | **open** |
| HTTP RSS soak in required CI | Local 30 m PASS; smoke job still open | **open** |
| Second maintainer / bus factor | Single primary maintainer today | **open** |

## LATER (explicit non-goals for 0.1.0)

| Topic | Notes |
|---|---|
| Full JIT beyond kernel-recognized numeric loops | JIT scope stays narrow in 0.1.0 |
| Independent security audit | No third-party review completed |
| Production deployments at scale | Nothing proven in production |
| Byte-reproducible macOS / Windows / FreeBSD archives | Explicitly not claimed (`docs/REPRODUCE_BUILDS.md`) |

## How to pick work

See [CONTRIBUTING.md](CONTRIBUTING.md) (Good first issues) and [docs/SAFETY_CLAIMS.md](docs/SAFETY_CLAIMS.md) before claiming safety or platform support.
