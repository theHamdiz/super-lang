# Verifying Super releases (0.1.0)

## Checksums

Release bundles ship `SHA256SUMS` beside each archive on GitHub Releases.

```bash
curl -fsSLO "https://github.com/theHamdiz/super-lang/releases/download/v0.1.0/SHA256SUMS"
curl -fsSLO "https://github.com/theHamdiz/super-lang/releases/download/v0.1.0/super-0.1.0-x86_64-unknown-linux-gnu.tar.gz"
sha256sum -c SHA256SUMS
```

The installer scripts (`install.sh`, `install.ps1`) perform the same check before extracting into `~/.super/`.

Each unpacked bundle also contains `build-info.txt` (commit SHA, `rustc -V`, UTC build stamp, and `SOURCE_DATE_EPOCH` when set).

## GitHub build attestations

Tag releases from workflow `release-bundle.yml` attach **SLSA-style build provenance**
attestations for the aggregated `SHA256SUMS` manifest **and** each of the seven
release archives (Round 9 Item 6).

```bash
gh release download v0.1.0 -D /tmp/super-verify
cd /tmp/super-verify
gh attestation verify SHA256SUMS --owner theHamdiz --repo super-lang
# Each of the seven shipping archives is attested individually (Round 9 Item 6):
for f in super-*.tar.gz super-*.zip; do
  gh attestation verify "$f" --owner theHamdiz --repo super-lang
done
```

## Supported version

Security fixes are accepted for **0.1.0** only until a newer stable line is announced — see [SECURITY.md](../SECURITY.md).

## Reproducibility

Release packaging sets `SOURCE_DATE_EPOCH`, remaps absolute paths, sorts tar
members, and uses `gzip -n`. Full recipe and per-target claim table:
[`REPRODUCE_BUILDS.md`](REPRODUCE_BUILDS.md).

```bash
export SOURCE_DATE_EPOCH="$(git log -1 --format=%ct)"
export SUPER_VERSION=0.1.0
export SUPER_TARGET=x86_64-unknown-linux-gnu
./scripts/package_release.sh
scripts/repro_check.sh --target x86_64-unknown-linux-gnu
```

**Status (Round 7):** both claimed Linux targets are byte-identical across two
isolated trees:

| Target | Dual-build SHA256 |
|---|---|
| `x86_64-unknown-linux-gnu` | `86f5608570a5533241436ad898dd1c2892fe6ef7633a2b56413e563bd83e067c` |
| `x86_64-unknown-linux-musl` | `6dcdeb01febcc285b8ae7e91e34fcdfaf4c929b9e7f1555d644be1d9b79dc773` |

Musl builds use pinned `rust:1-alpine@sha256:8d0e82ab…`. macOS / Windows /
FreeBSD are **not** claimed as byte-reproducible.

## What this does not prove

Checksum verification does not audit compiler correctness, sandbox boundaries, or crypto implementations — see [SAFETY_CLAIMS.md](SAFETY_CLAIMS.md). Attestations prove which workflow produced `SHA256SUMS`, not that the Super language semantics are correct.