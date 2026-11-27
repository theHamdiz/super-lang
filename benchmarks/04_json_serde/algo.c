/* 04_json_serde — LCG field checksum (parity with peers; JSON work lives in Super) */
#include <stdint.h>
#include <stdio.h>

#define N 10000
#define LCG_MOD 2147483648ULL

int main(void) {
  uint64_t rng = 4861952ULL;
  int64_t cs = 0;
  for (int i = 0; i < N; i++) {
    rng = (rng * 1103515245ULL + 12345ULL) % LCG_MOD;
    int64_t id = (int64_t)(rng % 1000000ULL);
    rng = (rng * 1103515245ULL + 12345ULL) % LCG_MOD;
    int64_t total = (int64_t)(rng % 100000ULL);
    rng = (rng * 1103515245ULL + 12345ULL) % LCG_MOD;
    int64_t items = (int64_t)(rng % 8ULL) + 1;
    cs += id + total + items * 17;
    cs += id + total + items * 17;
  }
  printf("CHECKSUM=%lld\n", (long long)cs);
  return 0;
}
