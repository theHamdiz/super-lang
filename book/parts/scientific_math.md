# Scientific Computing & Numerical Methods (`std.math`)

Super’s scientific stack lives under `std.math` and is designed for zero-GC kernels on scoped data stacks with `@` restrict borrows, `#simd` / `par` loops, and multiple dispatch.

## Surfaces

| Module | Role |
| --- | --- |
| `algebra` | `Ring`, `AlgField`, `Matrix`, `Distribution`, compensated sums |
| `special` | gamma, erf, Bessel J/Y/I/K, zeta, Lambert W |
| `ad` | `Dual64`, `HyperDual64`, reverse tape / JVP |
| `optimize` | Brent, Newton, Nelder–Mead, BFGS, L-BFGS, AdamW, simplex LP, active-set QP |
| `integrate` | Gauss–Kronrod, tanh–sinh, RK4 / DOPRI5, Rosenbrock4 / BDF2 (stiff) |
| `interp` / `poly` | splines, barycentric, Chebyshev |
| `linalg` | dense / diagonal / CSR, LU / Cholesky / QR / CG / GMRES / BiCGSTAB, `when (a,b)` mul |
| `fft` / `stats` / `rand` / `geom` | radix-2 + Bluestein FFT, Welford stats, MRG32k3a / Philox PRNGs + Halton / Sobol, 2D geometry |

## Multiple dispatch

Structured matrix products use tuple `when` and `#symmetric` commutative helpers:

```super
when (a, b) {
    (LinOp.Dense, LinOp.Diag) => dense_diag_mul(a, b)
    (LinOp.Diag, LinOp.Dense) => diag_dense_mul(a, b)
    else => dense_dense_mul(a, b)
}
```

## Automatic differentiation

Forward mode via `Dual64.{v, d}` is the default for scalar objectives; omit analytic gradients in optimizers and they can differentiate through Duals.

## Import

```super
use std.math.linalg
use std.math.special
use std.math.ad
```
