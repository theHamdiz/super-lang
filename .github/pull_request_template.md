## Summary

<!-- What changed and why (not a file list). -->

## Testing

- [ ] `cargo test --workspace` (or narrower scope: ___)
- [ ] `super test` on touched examples/stdlib paths: ___
- [ ] `python3 scripts/generate_site_docs.py` if site sources changed
- [ ] `python3 scripts/check_windows_aot_net.py` if platform/networking docs changed

## Docs / honesty

- [ ] No changes to frozen `docs/REPORT.md`, `docs/results.json`, or `benchmarks/data/*`
- [ ] Windows networking wording uses: **Windows C23 AOT sockets: Winsock2 (WSAPoll) in 0.1.0.** when relevant

## Checklist

- [ ] API Style Guide in CONTRIBUTING respected for public `.sr` names
- [ ] No secrets or `.env` files committed