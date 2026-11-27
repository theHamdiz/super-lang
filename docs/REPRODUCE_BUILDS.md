# Reproducing Super release builds (0.1.0)

## Recipe

```bash
export SOURCE_DATE_EPOCH="$(git log -1 --format=%ct)"
export SUPER_VERSION=0.1.0
export SUPER_TARGET=x86_64-unknown-linux-gnu   # or x86_64-unknown-linux-musl
export LC_ALL=C LANG=C TZ=UTC
./scripts/package_release.sh
sha256sum -c "dist/super-${SUPER_VERSION}-${SUPER_TARGET}.sha256"
```

`package_release.sh` remaps rustc/C absolute paths, builds with
`CARGO_BUILD_JOBS=1`, and packs with sorted tar members (GNU tar or Python
fallback), fixed owner/mtime, and `gzip -n`.

## Two-environment check

```bash
scripts/repro_check.sh --target x86_64-unknown-linux-gnu
scripts/repro_check.sh --target x86_64-unknown-linux-musl   # Alpine / musl-gcc
```

Writes `docs/repro-<triple>.txt` with side-by-side SHA256 values.

## Musl CI image (pinned digest)

```text
rust:1-alpine@sha256:8d0e82abfdf9a38071e07cabc5afba4ddbe077631a80e4e742a760c38beadc33
```

## Claim table (Round 8)

| Target | Reproducible | Evidence |
|---|---|---|
| `x86_64-unknown-linux-gnu` | **yes** | `docs/repro-x86_64-unknown-linux-gnu.txt` — A=B=`86f5608570a5533241436ad898dd1c2892fe6ef7633a2b56413e563bd83e067c` |
| `x86_64-unknown-linux-musl` | **yes** | `docs/repro-x86_64-unknown-linux-musl.txt` — A=B=`6dcdeb01febcc285b8ae7e91e34fcdfaf4c929b9e7f1555d644be1d9b79dc773` (pinned Alpine image) |
| `x86_64-unknown-freebsd` | **not reproducible** | cross sysroot / linker script drift — no byte-identical claim |
| `x86_64-apple-darwin` / `aarch64-apple-darwin` | **not reproducible** | codesign / linker timestamps — no byte-identical claim |
| `x86_64-pc-windows-gnu` | **not reproducible** | PE / mingw timestamps — no byte-identical claim |

Plainly: **macOS, FreeBSD, and Windows release archives are not reproducible** in 0.1.0.

Toolchain note (gnu host proof): `rustc 1.99.1 (58cf8f9c3 2026-10-13)`. Match `rustc -V` and the Alpine digest above for bit-identity across machines.
