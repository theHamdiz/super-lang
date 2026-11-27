/* 09_matmul_tiled — 256×256 f64 tiled multiply */
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define N 256
#define TILE 32

int main(void) {
  static double A[N * N], B[N * N], C[N * N];
  for (int i = 0; i < N * N; i++) {
    A[i] = ((i * 17) % 100) / 100.0;
    B[i] = ((i * 31) % 100) / 100.0;
  }
  memset(C, 0, sizeof(C));
  for (int ii = 0; ii < N; ii += TILE) {
    for (int jj = 0; jj < N; jj += TILE) {
      for (int kk = 0; kk < N; kk += TILE) {
        int iimax = ii + TILE < N ? ii + TILE : N;
        int jjmax = jj + TILE < N ? jj + TILE : N;
        int kkmax = kk + TILE < N ? kk + TILE : N;
        for (int i = ii; i < iimax; i++) {
          for (int k = kk; k < kkmax; k++) {
            double aik = A[i * N + k];
            for (int j = jj; j < jjmax; j++) {
              C[i * N + j] += aik * B[k * N + j];
            }
          }
        }
      }
    }
  }
  double fro = 0.0;
  for (int i = 0; i < N * N; i++) fro += C[i] * C[i];
  long long cs = (long long)floor(sqrt(fro) * 1000.0);
  printf("CHECKSUM=%lld\n", cs);
  return 0;
}
