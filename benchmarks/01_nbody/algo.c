/* 01_nbody — 64 bodies × 500 steps, soft gravity, SoA */
#include <math.h>
#include <stdint.h>
#include <stdio.h>

#define N 64
#define STEPS 500
#define DT 0.001
#define SOFT 0.01

#define LCG_MOD 2147483648ULL
static uint64_t rng = 12648430ULL;
static uint32_t next_u(void) {
  rng = (rng * 1103515245ULL + 12345ULL) % LCG_MOD;
  return (uint32_t)rng;
}
static double next_f(void) { return (double)next_u() / 2147483648.0; }

int main(void) {
  static double x[N], y[N], vx[N], vy[N], m[N];
  for (int i = 0; i < N; i++) {
    x[i] = next_f() * 2.0 - 1.0;
    y[i] = next_f() * 2.0 - 1.0;
    vx[i] = (next_f() - 0.5) * 0.01;
    vy[i] = (next_f() - 0.5) * 0.01;
    m[i] = 0.5 + next_f();
  }
  for (int step = 0; step < STEPS; step++) {
    for (int i = 0; i < N; i++) {
      double ax = 0.0, ay = 0.0;
      for (int j = 0; j < N; j++) {
        if (i == j) continue;
        double dx = x[j] - x[i];
        double dy = y[j] - y[i];
        double d2 = dx * dx + dy * dy + SOFT;
        double inv = 1.0 / sqrt(d2);
        double inv3 = inv * inv * inv;
        ax += m[j] * dx * inv3;
        ay += m[j] * dy * inv3;
      }
      vx[i] += ax * DT;
      vy[i] += ay * DT;
    }
    for (int i = 0; i < N; i++) {
      x[i] += vx[i] * DT;
      y[i] += vy[i] * DT;
    }
  }
  long long cs = 0;
  for (int i = 0; i < N; i++) {
    cs += (long long)floor(x[i] * 1000.0);
    cs += (long long)floor(y[i] * 1000.0);
    cs += (long long)floor(vx[i] * 1e6);
  }
  printf("CHECKSUM=%lld\n", cs);
  return 0;
}
