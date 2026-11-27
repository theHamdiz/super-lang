#include "mathlib.h"

double mathlib_lerp(double a, double b, double t) {
  return a + (b - a) * t;
}

double mathlib_clamp(double x, double lo, double hi) {
  if (x < lo) {
    return lo;
  }
  if (x > hi) {
    return hi;
  }
  return x;
}
