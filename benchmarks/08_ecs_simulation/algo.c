/* 08_ecs — 2000 entities × 20 ticks, spatial grid neighbor queries */
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define N 2000
#define TICKS 20
#define CELL 10
#define GRID 64

#define LCG_MOD 2147483648ULL
static uint64_t rng = 15482888ULL;
static uint32_t next_u(void) {
  rng = (rng * 1103515245ULL + 12345ULL) % LCG_MOD;
  return (uint32_t)rng;
}
static double next_f(void) { return (double)next_u() / 2147483648.0; }

int main(void) {
  double *px = (double *)malloc(sizeof(double) * N);
  double *py = (double *)malloc(sizeof(double) * N);
  double *vx = (double *)malloc(sizeof(double) * N);
  double *vy = (double *)malloc(sizeof(double) * N);
  int *hp = (int *)malloc(sizeof(int) * N);
  for (int i = 0; i < N; i++) {
    px[i] = (next_f() * 600.0);
    py[i] = (next_f() * 600.0);
    vx[i] = (double)((next_f() - 0.5) * 2.0);
    vy[i] = (double)((next_f() - 0.5) * 2.0);
    hp[i] = 10 + (int)(next_u() % 20ULL);
  }
  int *heads = (int *)malloc(sizeof(int) * GRID * GRID);
  int *next = (int *)malloc(sizeof(int) * N);

  for (int t = 0; t < TICKS; t++) {
    for (int i = 0; i < N; i++) {
      px[i] += vx[i];
      py[i] += vy[i];
      if (px[i] < 0 || px[i] > 600) vx[i] = -vx[i];
      if (py[i] < 0 || py[i] > 600) vy[i] = -vy[i];
    }
    memset(heads, 0xff, sizeof(int) * GRID * GRID);
    for (int i = 0; i < N; i++) {
      int cx = (int)(px[i] / CELL);
      int cy = (int)(py[i] / CELL);
      if (cx < 0) cx = 0;
      if (cy < 0) cy = 0;
      if (cx >= GRID) cx = GRID - 1;
      if (cy >= GRID) cy = GRID - 1;
      int cell = cy * GRID + cx;
      next[i] = heads[cell];
      heads[cell] = i;
    }
    for (int i = 0; i < N; i++) {
      int cx = (int)(px[i] / CELL);
      int cy = (int)(py[i] / CELL);
      if (cx < 0) cx = 0;
      if (cy < 0) cy = 0;
      if (cx >= GRID) cx = GRID - 1;
      if (cy >= GRID) cy = GRID - 1;
      int hits = 0;
      for (int oy = -1; oy <= 1; oy++) {
        for (int ox = -1; ox <= 1; ox++) {
          int nx = cx + ox, ny = cy + oy;
          if (nx < 0 || ny < 0 || nx >= GRID || ny >= GRID) continue;
          int id = heads[ny * GRID + nx];
          while (id >= 0) {
            if (id != i) {
              float dx = px[i] - px[id], dy = py[i] - py[id];
              if (dx * dx + dy * dy < 4.0) hits++;
            }
            id = next[id];
          }
        }
      }
      if (hits > 3) hp[i] -= 1;
    }
  }
  long long cs = 0;
  for (int i = 0; i < N; i++) {
    cs += hp[i];
    cs += (long long)floor(px[i] * 10.0);
    cs += (long long)floor(py[i] * 10.0);
  }
  printf("CHECKSUM=%lld\n", cs);
  free(px);
  free(py);
  free(vx);
  free(vy);
  free(hp);
  free(heads);
  free(next);
  return 0;
}
