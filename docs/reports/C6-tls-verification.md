# C6 — TLS verification report

**Date:** 2026-11-26  
**Scope:** What `std.net.tls` implements today vs what an agent harness needs for real HTTPS.

## Findings (from implementation, not assumption)

| Capability | Status | Evidence |
|------------|--------|----------|
| TLS 1.3 record framing | Present (experimental) | `stdlib/net/tls.sr` — framing, ALPN, SNI, PEM helpers |
| Full handshake with peer | Partial / experimental | Host bindings exist; not labeled stable per A2 |
| X.509 chain validation | Partial | `stdlib/sec/x509.sr` parses certs/CSRs; chain+trust-store path incomplete for production |
| Hostname verification | Incomplete | Not a hardened stable API |
| System trust-store loading | Incomplete | No OS trust-store loader in stable tier |
| Rejection of expired / self-signed / wrong-host | Not CI-proven against real servers | No gated live-server suite |

## Decision (A2 reconciliation)

- **Self-rolled TLS stays experimental** (never marked stable).
- Stable harness HTTPS should **bind a vetted TLS library** via the C ABI (or platform stack) before claiming production readiness.
- Until that bind lands, agent examples use **mock HTTP** (`examples/agent`) and document the gap.

## Next steps

1. Choose vetted backend (system TLS / known library) behind `std.net.tls` stable façade.
2. Add CI matrix: valid Let's Encrypt host, expired cert, self-signed, wrong SAN — all must fail closed.
3. Keep framing implementation available under experimental tier for offline tests.
