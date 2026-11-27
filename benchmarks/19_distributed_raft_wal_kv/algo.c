/* 19_distributed_raft_wal_kv — single-process Raft log replication (leader + 2 followers):
   CRC32-protected WAL records, lossy/corrupting virtual links, majority commit, KV state machine.
   No real networking: every packet fate comes from a deterministic LCG. */
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define MOD 1000000007LL
#define TICKS 6000
#define DRAIN 200
#define REC 20            /* idx, term, key, val, crc32 — 4 bytes each, big-endian */
#define BATCH 4
#define KEYS 512
#define DROP_PCT 35
#define ACK_DROP_PCT 15
#define CORRUPT_PCT 3
#define CAP (TICKS * REC)

static int64_t rng = 19019;
static int64_t next_rand(void) { return rng = rng * 48271 % 2147483647; }
static int64_t cs = 0;
static void fold(int64_t v) { cs = (cs * 131 + v) % MOD; }

static uint32_t crc_tbl[256];
static void crc_init(void) {
  for (uint32_t i = 0; i < 256; i++) {
    uint32_t c = i;
    for (int k = 0; k < 8; k++) c = (c & 1) ? (c >> 1) ^ 0xEDB88320u : c >> 1;
    crc_tbl[i] = c;
  }
}
static uint32_t crc32(const uint8_t *d, size_t n) {
  uint32_t c = 0xFFFFFFFFu;
  for (size_t i = 0; i < n; i++) c = crc_tbl[(c ^ d[i]) & 255] ^ (c >> 8);
  return c ^ 0xFFFFFFFFu;
}

static void put32(uint8_t *p, uint32_t v) { p[0] = v >> 24; p[1] = v >> 16; p[2] = v >> 8; p[3] = v; }
static uint32_t get32(const uint8_t *p) { return ((uint32_t)p[0] << 24) | ((uint32_t)p[1] << 16) | ((uint32_t)p[2] << 8) | p[3]; }

static uint8_t wal[3][CAP];
static int64_t lens[3], applied[3], kv[3][KEYS];
static int64_t next_idx[3] = {1, 1, 1}, match_idx[3];
static int64_t commit_index, sent, dropped, crc_rejects, acks_lost;

/* apply WAL entries of node n up to `upto` into its KV map (re-verifying each CRC) */
static void apply_upto(int n, int64_t upto) {
  while (applied[n] < upto) {
    const uint8_t *r = wal[n] + applied[n] * REC;
    if (crc32(r, 16) == get32(r + 16)) kv[n][get32(r + 8) % KEYS] = get32(r + 12);
    applied[n]++;
  }
}

int main(void) {
  crc_init();
  for (int t = 0; t < TICKS + DRAIN; t++) {
    if (t < TICKS) {
      uint8_t *r = wal[0] + lens[0] * REC;
      put32(r, (uint32_t)(lens[0] + 1));
      put32(r + 4, 1);
      put32(r + 8, (uint32_t)(next_rand() % KEYS));
      put32(r + 12, (uint32_t)(next_rand() % 1000000));
      put32(r + 16, crc32(r, 16));
      lens[0]++;
    }
    int64_t leader_commit = commit_index;
    for (int f = 1; f <= 2; f++) {
      int64_t nxt = next_idx[f];
      if (nxt > lens[0]) continue;
      int64_t cnt = lens[0] - nxt + 1 < BATCH ? lens[0] - nxt + 1 : BATCH;
      int64_t r1 = next_rand() % 100, r2 = next_rand() % 100, r3 = next_rand() % 100;
      sent++;
      if (r1 < DROP_PCT) { dropped++; continue; }
      uint8_t msg[BATCH * REC];
      memcpy(msg, wal[0] + (nxt - 1) * REC, (size_t)(cnt * REC));
      if (r2 < CORRUPT_PCT) msg[next_rand() % (cnt * REC)] ^= 0x5A;
      for (int64_t j = 0; j < cnt; j++) {
        const uint8_t *r = msg + j * REC;
        if (crc32(r, 16) != get32(r + 16)) { crc_rejects++; break; }
        int64_t idx = get32(r);
        if (idx == lens[f] + 1) {
          memcpy(wal[f] + lens[f] * REC, r, REC);
          lens[f]++;
        } else if (idx > lens[f] + 1) break;
      }
      apply_upto(f, leader_commit < lens[f] ? leader_commit : lens[f]);
      if (r3 < ACK_DROP_PCT) { acks_lost++; continue; }
      if (lens[f] > match_idx[f]) match_idx[f] = lens[f];
      next_idx[f] = lens[f] + 1;
    }
    /* majority commit = median of (leader_len, match1, match2) */
    int64_t a = lens[0], b = match_idx[1], c = match_idx[2], med;
    if (a > b) { int64_t x = a; a = b; b = x; }
    if (b > c) { int64_t x = b; b = c; c = x; }
    if (a > b) { int64_t x = a; a = b; b = x; }
    med = b;
    if (med > commit_index) commit_index = med;
    apply_upto(0, commit_index);
  }
  fold(lens[0]); fold(lens[1]); fold(lens[2]);
  fold(commit_index); fold(applied[0]); fold(applied[1]); fold(applied[2]);
  fold(sent); fold(dropped); fold(crc_rejects); fold(acks_lost);
  for (int n = 0; n < 3; n++) {
    int64_t d = 0;
    for (int k = 0; k < KEYS; k++) d = (d * 131 + kv[n][k]) % MOD;
    fold(d);
  }
  fold(crc32(wal[0], (size_t)(lens[0] * REC)));
  fold(crc32(wal[1], (size_t)(lens[1] * REC)));
  printf("CHECKSUM=%lld\n", (long long)cs);
  return 0;
}
