/* 07_crc32 — IEEE CRC32 on expanded LCG-scrambled block (parity with py/js/sr) */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static uint32_t crc_table[256];

static void init_crc(void) {
  for (uint32_t i = 0; i < 256; i++) {
    uint32_t c = i;
    for (int k = 0; k < 8; k++)
      c = (c & 1u) ? (0xEDB88320u ^ (c >> 1)) : (c >> 1);
    crc_table[i] = c;
  }
}

static uint32_t crc32(const uint8_t *data, size_t n) {
  uint32_t crc = 0xFFFFFFFFu;
  for (size_t i = 0; i < n; i++)
    crc = crc_table[(crc ^ data[i]) & 0xFFu] ^ (crc >> 8);
  return crc ^ 0xFFFFFFFFu;
}

int main(void) {
  init_crc();
  const char *seed = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ..";
  size_t seed_len = strlen(seed);
  size_t block_len = seed_len;
  for (int e = 0; e < 6; e++) block_len *= 2;
  uint8_t *block = (uint8_t *)malloc(block_len);
  if (!block) return 1;
  memcpy(block, seed, seed_len);
  size_t cur = seed_len;
  for (int e = 0; e < 6; e++) {
    memcpy(block + cur, block, cur);
    cur *= 2;
  }

  uint64_t rng = 8137778ULL;
  int64_t acc = 0;
  for (int i = 0; i < 4096; i++) {
    rng = (rng * 1103515245ULL + 12345ULL) % 2147483648ULL;
    acc += (int64_t)(rng % 255ULL) + (int64_t)block[i % block_len];
  }

  size_t payload_len = block_len * 9;
  uint8_t *payload = (uint8_t *)malloc(payload_len);
  if (!payload) {
    free(block);
    return 1;
  }
  for (int r = 0; r < 9; r++)
    memcpy(payload + (size_t)r * block_len, block, block_len);

  uint32_t c1 = crc32(payload, payload_len);
  uint32_t c2 = crc32(block, block_len);
  int64_t cs = (int64_t)c1 + (int64_t)c2 + (acc % 1000);
  printf("CHECKSUM=%lld\n", (long long)cs);

  free(payload);
  free(block);
  return 0;
}
