# Rust ↔ Super interop

The embed crate is **`super-ffi`**. Import it as:

```rust
use super_ffi::{Engine, Value};
```

Do **not** name a package `super` and do **not** write `use super::{...}` for
the embed API — `super` is a Rust keyword (parent module).

## Directions

1. **Rust hosts Super** — `Engine::bind_fn_rust`, `bind_slice_mut`, load `src/ai.sr`.
2. **Super calls Rust** — `src/native_ai.rs` exposes `rarity_score`; the host binds
   it as `host.rarity_score` for Super’s `score_agent`.

```sh
cargo test
cargo run
```
