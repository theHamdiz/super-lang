# C++20 ↔ Super interop

Headers: `include/super.h` and `include/super.hpp` (vendored by `make headers`).

## Directions

1. **C++ hosts Super** — `main.cpp` uses `super::Engine`, `bind` lambdas, and
   `bind_span` for zero-copy entities.
2. **Super calls C++** — `native/damage.cpp` is a C++ library with an
   `extern "C"` surface (`damage_c.h`). Super calls `host.damage_apply_hit`
   from `hit_through_cpp`.

```sh
make test
```
