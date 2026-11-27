/* 02_mandelbrot — 1024×1024 maxiter 128, OpenMP */
#include <stdint.h>
#include <stdio.h>

#define W 1024
#define H 1024
#define MAXIT 128

int main(void) {
  long long cs = 0;
#pragma omp parallel for reduction(+ : cs) schedule(static)
  for (int y = 0; y < H; y++) {
    long long local = 0;
    double cy = ((double)y / (double)H) * 2.0 - 1.0;
    for (int x = 0; x < W; x++) {
      double cx = ((double)x / (double)W) * 3.5 - 2.5;
      double zx = 0.0, zy = 0.0;
      int it = 0;
      for (; it < MAXIT; it++) {
        double zx2 = zx * zx, zy2 = zy * zy;
        if (zx2 + zy2 > 4.0) break;
        double nzy = 2.0 * zx * zy + cy;
        zx = zx2 - zy2 + cx;
        zy = nzy;
      }
      local += it;
    }
    cs += local;
  }
  printf("CHECKSUM=%lld\n", cs);
  return 0;
}
