/* 12_sparse_fem_fft — 128x128 reaction-Poisson CSR, Jacobi-PCG x20, 2D FFT low-pass bank */
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define G 128
#define N 16384
#define ITERS 20
#define PI 3.141592653589793

static int *indptr, *indices;
static double *values;

static void build_system(double *diag) {
  indptr = malloc((N + 1) * sizeof(int));
  indices = malloc(5 * N * sizeof(int));
  values = malloc(5 * N * sizeof(double));
  int nz = 0;
  indptr[0] = 0;
  for (int i = 0; i < N; i++) {
    int row = i / G, col = i % G;
    double d = 4.0 + (i * 37 % 11) * 0.25;
    diag[i] = d;
    if (row > 0) { indices[nz] = i - G; values[nz++] = -1.0; }
    if (col > 0) { indices[nz] = i - 1; values[nz++] = -1.0; }
    indices[nz] = i; values[nz++] = d;
    if (col < G - 1) { indices[nz] = i + 1; values[nz++] = -1.0; }
    if (row < G - 1) { indices[nz] = i + G; values[nz++] = -1.0; }
    indptr[i + 1] = nz;
  }
}

static void matvec(const double *x, double *out) {
  for (int r = 0; r < N; r++) {
    double s = 0.0;
    for (int k = indptr[r]; k < indptr[r + 1]; k++) s += values[k] * x[indices[k]];
    out[r] = s;
  }
}

static double dot(const double *a, const double *b) {
  double s = 0.0;
  for (int i = 0; i < N; i++) s += a[i] * b[i];
  return s;
}

static double pcg(const double *diag, const double *b, double *x) {
  double *r = malloc(N * sizeof(double)), *z = malloc(N * sizeof(double));
  double *p = malloc(N * sizeof(double)), *ap = malloc(N * sizeof(double));
  for (int i = 0; i < N; i++) { x[i] = 0.0; r[i] = b[i]; z[i] = r[i] / diag[i]; p[i] = z[i]; }
  double rz = dot(r, z);
  for (int it = 0; it < ITERS; it++) {
    matvec(p, ap);
    double alpha = rz / dot(p, ap);
    for (int i = 0; i < N; i++) {
      x[i] += alpha * p[i];
      r[i] -= alpha * ap[i];
      z[i] = r[i] / diag[i];
    }
    double rz_new = dot(r, z);
    double beta = rz_new / rz;
    for (int i = 0; i < N; i++) p[i] = z[i] + beta * p[i];
    rz = rz_new;
  }
  double rr = dot(r, r);
  free(r); free(z); free(p); free(ap);
  return rr;
}

static int bit_reverse(int i, int bits) {
  int r = 0;
  for (int b = 0; b < bits; b++) { r = (r << 1) | (i & 1); i >>= 1; }
  return r;
}

/* Radix-2 FFT of length G (same butterflies and twiddles as std.math.fft). */
static void fft1(double *re, double *im, int inv) {
  double tre[G], tim[G];
  if (inv) for (int i = 0; i < G; i++) im[i] = 0.0 - im[i];
  for (int i = 0; i < G; i++) { int s = bit_reverse(i, 7); tre[i] = re[s]; tim[i] = im[s]; }
  for (int span = 2; span <= G; span *= 2) {
    int half = span / 2;
    double ang = 0.0 - 2.0 * PI / (double)span;
    for (int start = 0; start < G; start += span) {
      for (int k = 0; k < half; k++) {
        double wr = cos(ang * (double)k), wi = sin(ang * (double)k);
        int a = start + k, b = a + half;
        double tr = tre[b] * wr - tim[b] * wi;
        double ti = tre[b] * wi + tim[b] * wr;
        tre[b] = tre[a] - tr; tim[b] = tim[a] - ti;
        tre[a] = tre[a] + tr; tim[a] = tim[a] + ti;
      }
    }
  }
  if (inv) {
    double s = 1.0 / (double)G;
    for (int i = 0; i < G; i++) { re[i] = tre[i] * s; im[i] = (0.0 - tim[i]) * s; }
  } else {
    memcpy(re, tre, sizeof(tre));
    memcpy(im, tim, sizeof(tim));
  }
}

static void fft2(double *re, double *im, int inv) {
  double lr[G], li[G];
  for (int r = 0; r < G; r++) {
    for (int c = 0; c < G; c++) { lr[c] = re[r * G + c]; li[c] = im[r * G + c]; }
    fft1(lr, li, inv);
    for (int c = 0; c < G; c++) { re[r * G + c] = lr[c]; im[r * G + c] = li[c]; }
  }
  for (int c = 0; c < G; c++) {
    for (int r = 0; r < G; r++) { lr[r] = re[r * G + c]; li[r] = im[r * G + c]; }
    fft1(lr, li, inv);
    for (int r = 0; r < G; r++) { re[r * G + c] = lr[r]; im[r * G + c] = li[r]; }
  }
}

int main(void) {
  static double b[N], diag[N], x[N], sre[N], sim[N], fre[N], fim[N];
  for (int i = 0; i < N; i++) b[i] = ((i * 7 + (i / G) * 13) % 17 - 8) / 8.0;
  build_system(diag);
  double res = sqrt(pcg(diag, b, x));

  memcpy(sre, x, sizeof(x));
  memset(sim, 0, sizeof(sim));
  fft2(sre, sim, 0);

  const int cuts[3] = {64, 256, 1024};
  double energy = 0.0, corner = 0.0;
  for (int q = 0; q < 3; q++) {
    memcpy(fre, sre, sizeof(sre));
    memcpy(fim, sim, sizeof(sim));
    for (int k = 0; k < N; k++) {
      int ky = k / G, kx = k % G;
      int fy = ky <= G / 2 ? ky : G - ky;
      int fx = kx <= G / 2 ? kx : G - kx;
      if (fx * fx + fy * fy > cuts[q]) { fre[k] = 0.0; fim[k] = 0.0; }
    }
    fft2(fre, fim, 1);
    for (int k = 0; k < N; k++) energy += fre[k] * fre[k];
    corner += fre[0] + fre[N / 2 + G / 2] + fre[N - 1];
  }

  double xs = dot(x, x);
  long long cs = (long long)floor(sqrt(xs) * 1000000.0) + (long long)floor(energy * 1000.0) +
                 (long long)floor(corner * 1000000.0) + (long long)floor(res * 1000000000.0);
  printf("CHECKSUM=%lld\n", cs);
  return 0;
}
