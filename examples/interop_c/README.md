# C17 ↔ Super interop

Self-contained example. Headers live in `include/super.h` (copied from the repo
`include/` by `make headers`).

## Directions

1. **C hosts Super** — `main.c` embeds `libsuper_ffi`, shares a zero-copy
   `Particle[]` slice, hot-reloads `src/physics.sr`.
2. **Super calls C** — `native/mathlib.{h,c}` is a real C library. The host binds
   `mathlib_lerp` / `mathlib_clamp` as `host.mathlib_*`; Super calls them from
   `blend` / `limit`.

```sh
make test
```
