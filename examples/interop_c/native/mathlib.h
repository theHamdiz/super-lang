/* C library that Super imports through host.mathlib_* binds. */
#ifndef INTEROP_MATHLIB_H
#define INTEROP_MATHLIB_H

#ifdef __cplusplus
extern "C" {
#endif

/** Linear interpolation: a + (b - a) * t */
double mathlib_lerp(double a, double b, double t);

/** Clamp x into [lo, hi]. */
double mathlib_clamp(double x, double lo, double hi);

#ifdef __cplusplus
}
#endif

#endif
