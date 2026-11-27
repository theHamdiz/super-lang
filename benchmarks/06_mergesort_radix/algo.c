/* 06_mergesort_radix — sort 50000 u64, verify + checksum */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define N 50000

#define LCG_MOD 2147483648ULL
static uint64_t rng = 395799ULL;
static uint32_t next_u(void) {
  rng = (rng * 1103515245ULL + 12345ULL) % LCG_MOD;
  return (uint32_t)rng;
}

static void merge(uint64_t *a, uint64_t *tmp, int lo, int mid, int hi) {
  int i = lo, j = mid, k = lo;
  while (i < mid && j < hi) {
    if (a[i] <= a[j]) tmp[k++] = a[i++];
    else tmp[k++] = a[j++];
  }
  while (i < mid) tmp[k++] = a[i++];
  while (j < hi) tmp[k++] = a[j++];
  for (i = lo; i < hi; i++) a[i] = tmp[i];
}

static void mergesort(uint64_t *a, uint64_t *tmp, int lo, int hi) {
  if (hi - lo <= 1) return;
  int mid = lo + (hi - lo) / 2;
  mergesort(a, tmp, lo, mid);
  mergesort(a, tmp, mid, hi);
  merge(a, tmp, lo, mid, hi);
}

int main(void) {
  uint64_t *a = (uint64_t *)malloc(sizeof(uint64_t) * N);
  uint64_t *tmp = (uint64_t *)malloc(sizeof(uint64_t) * N);
  for (int i = 0; i < N; i++) a[i] = next_u();
  mergesort(a, tmp, 0, N);
  long long cs = 0;
  int ok = 1;
  for (int i = 0; i < N; i++) {
    cs ^= (long long)(a[i] & 0xffffffffULL);
    cs = (cs * 131) + (long long)(a[i] >> 32);
    if (i > 0 && a[i] < a[i - 1]) ok = 0;
  }
  printf("CHECKSUM=%lld\n", cs ^ (ok ? 1 : 0));
  free(a);
  free(tmp);
  return 0;
}
