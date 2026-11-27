# Security policy

**Supported version:** **0.1.0** only. Older or unofficial forks are out of scope.

## Report privately

Use [GitHub private vulnerability reporting](https://github.com/theHamdiz/super-lang/security/advisories/new) on **super-lang**. Do not post exploit details in public issues before a fix is available.

## Response expectations

Single-maintainer project — acknowledge within **7 days** when possible; target fixes on a best-effort cadence. Critical installer or remote-code-execution issues take priority over documentation nits.

## In scope

- Compiler or runtime memory safety bugs reachable from `.sr` source
- VM/JIT sandbox escapes
- Installer integrity failures (bypass of SHA256 verification)
- Documented misuse of `stdlib/sec` primitives that contradict published API contracts

## Out of scope (0.1.0)

- Experimental law/proof checker completeness
- Self-rolled TLS labeled experimental in `docs/reports/C6-tls-verification.md`
- **Windows C23 AOT sockets: Winsock2 (WSAPoll) in 0.1.0.** — report VM/host networking bugs separately; do not expect AOT socket hardening on Windows yet
- Benchmark score disputes (use benchmark issue template)

## Hardening claims

See [docs/SAFETY_CLAIMS.md](docs/SAFETY_CLAIMS.md). Challenge marketing or spec wording via the **Security claim review** issue template.