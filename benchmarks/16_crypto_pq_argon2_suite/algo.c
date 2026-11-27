/* 16_crypto_pq_argon2_suite — Argon2id-like KDF (m=8KiB t=2 p=1) x16, SHA-256 Merkle of tags,
   toy lattice negacyclic NTT poly-mul (q=12289, n=256). */
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define MOD 1000000007LL
#define NH 32
#define BW 64            /* words per block (256 B) */
#define NB 32            /* blocks: 32 * 256 B = 8 KiB */
#define PASSES 2
#define Q 12289
#define N 256
#define PAIRS 6
#define M32 0xFFFFFFFFULL

static uint64_t rng = 16001;
static uint64_t next_rand(void) { return rng = rng * 48271 % 2147483647; }

static uint32_t rotr(uint32_t x, int n) { return (x >> n) | (x << (32 - n)); }

static void sha256(const uint8_t *msg, size_t len, uint8_t out[32]) {
  static const uint32_t K[64] = {
      0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
      0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
      0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
      0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
      0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
      0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
      0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
      0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
      0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
      0xc67178f2};
  uint32_t hs[8] = {0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                    0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19};
  uint8_t buf[512];
  size_t total = len + 1;
  while (total % 64 != 56) total++;
  total += 8;
  memset(buf, 0, total);
  memcpy(buf, msg, len);
  buf[len] = 0x80;
  uint64_t bits = (uint64_t)len * 8;
  for (int i = 0; i < 8; i++) buf[total - 8 + i] = (uint8_t)(bits >> (56 - 8 * i));
  for (size_t off = 0; off < total; off += 64) {
    uint32_t w[64];
    for (int i = 0; i < 16; i++)
      w[i] = ((uint32_t)buf[off + 4 * i] << 24) | ((uint32_t)buf[off + 4 * i + 1] << 16) |
             ((uint32_t)buf[off + 4 * i + 2] << 8) | (uint32_t)buf[off + 4 * i + 3];
    for (int i = 16; i < 64; i++) {
      uint32_t s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >> 3);
      uint32_t s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >> 10);
      w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }
    uint32_t a = hs[0], b = hs[1], c = hs[2], d = hs[3], e = hs[4], f = hs[5], g = hs[6], h = hs[7];
    for (int i = 0; i < 64; i++) {
      uint32_t t1 = h + (rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)) + ((e & f) ^ (~e & g)) + K[i] + w[i];
      uint32_t t2 = (rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)) + ((a & b) ^ (a & c) ^ (b & c));
      h = g; g = f; f = e; e = d + t1; d = c; c = b; b = a; a = t1 + t2;
    }
    hs[0] += a; hs[1] += b; hs[2] += c; hs[3] += d;
    hs[4] += e; hs[5] += f; hs[6] += g; hs[7] += h;
  }
  for (int i = 0; i < 8; i++) {
    out[4 * i] = (uint8_t)(hs[i] >> 24);
    out[4 * i + 1] = (uint8_t)(hs[i] >> 16);
    out[4 * i + 2] = (uint8_t)(hs[i] >> 8);
    out[4 * i + 3] = (uint8_t)hs[i];
  }
}

/* ---- Argon2id-like block function (BlaMka G over 32-bit words) ---- */
static uint64_t rot32(uint64_t x, int r) { return ((x >> r) | (x << (32 - r))) & M32; }
static uint64_t blamka(uint64_t x, uint64_t y) { return (x + y + 2 * (x & 0xFFFF) * (y & 0xFFFF)) & M32; }

static void g_mix(uint64_t *z, int ia, int ib, int ic, int id) {
  uint64_t a = z[ia], b = z[ib], c = z[ic], d = z[id];
  a = blamka(a, b); d = rot32(d ^ a, 16);
  c = blamka(c, d); b = rot32(b ^ c, 12);
  a = blamka(a, b); d = rot32(d ^ a, 8);
  c = blamka(c, d); b = rot32(b ^ c, 7);
  z[ia] = a; z[ib] = b; z[ic] = c; z[id] = d;
}

static void p_round(uint64_t *z, int base, int stride) {
  int v[8];
  for (int k = 0; k < 8; k++) v[k] = base + k * stride;
  g_mix(z, v[0], v[2], v[4], v[6]);
  g_mix(z, v[1], v[3], v[5], v[7]);
  g_mix(z, v[0], v[3], v[4], v[7]);
  g_mix(z, v[1], v[2], v[5], v[6]);
}

static void compress(const uint64_t *x, const uint64_t *y, uint64_t *out) {
  uint64_t r[BW];
  for (int i = 0; i < BW; i++) r[i] = x[i] ^ y[i];
  memcpy(out, r, sizeof r);
  for (int row = 0; row < 8; row++) p_round(out, row * 8, 1);
  for (int col = 0; col < 8; col++) p_round(out, col, 8);
  for (int i = 0; i < BW; i++) out[i] ^= r[i];
}

static void init_block(const uint8_t h0[32], int j, uint64_t *w) {
  uint8_t msg[33], d[32];
  memcpy(msg, h0, 32);
  msg[32] = (uint8_t)j;
  sha256(msg, 33, d);
  for (int k = 0; k < 8; k++)
    w[k] = ((uint64_t)d[4 * k] << 24) | ((uint64_t)d[4 * k + 1] << 16) | ((uint64_t)d[4 * k + 2] << 8) | d[4 * k + 3];
  for (int k = 8; k < BW; k++)
    w[k] = (w[k - 8] + (w[k - 3] & 0xFFFF) * 40503 + (w[k - 1] >> 16) * 69069) & M32;
}

static void argon2_like(int idx_pw, uint8_t tag[32]) {
  uint8_t msg[36], h0[32];
  msg[0] = (uint8_t)idx_pw; msg[1] = NB * 256 / 1024; msg[2] = PASSES; msg[3] = 1;
  for (int i = 4; i < 36; i++) msg[i] = (uint8_t)(next_rand() & 255);
  sha256(msg, 36, h0);
  static uint64_t B[NB][BW];
  init_block(h0, 0, B[0]);
  init_block(h0, 1, B[1]);
  uint64_t nw[BW];
  for (int pass = 0; pass < PASSES; pass++) {
    for (int idx = pass == 0 ? 2 : 0; idx < NB; idx++) {
      const uint64_t *prev = B[(idx + NB - 1) % NB];
      uint64_t r;
      if (pass == 0 && idx < NB / 2) r = (((uint64_t)(pass * 131 + idx) * 40503 + 12345) * 69069) % 4294967296ULL;
      else r = prev[0];
      uint64_t r16 = r >> 16, x = (r16 * r16) >> 16;
      int ref;
      if (pass == 0) {
        uint64_t area = (uint64_t)idx - 1;
        ref = (int)(area - 1 - ((area * x) >> 16));
      } else {
        uint64_t area = NB - 2;
        uint64_t rel = area - 1 - ((area * x) >> 16);
        ref = (int)((idx + 1 + rel) % NB);
      }
      compress(prev, B[ref], nw);
      if (pass == 0) memcpy(B[idx], nw, sizeof nw);
      else for (int i = 0; i < BW; i++) B[idx][i] ^= nw[i];
    }
  }
  uint8_t bytes[BW * 4];
  for (int i = 0; i < BW; i++) {
    bytes[4 * i] = (uint8_t)(B[NB - 1][i] >> 24);
    bytes[4 * i + 1] = (uint8_t)(B[NB - 1][i] >> 16);
    bytes[4 * i + 2] = (uint8_t)(B[NB - 1][i] >> 8);
    bytes[4 * i + 3] = (uint8_t)B[NB - 1][i];
  }
  sha256(bytes, sizeof bytes, tag);
}

/* ---- toy lattice: negacyclic NTT over Z_12289[x]/(x^256+1) ---- */
static uint64_t pw_mod(uint64_t b, uint64_t e) {
  uint64_t r = 1;
  b %= Q;
  while (e) {
    if (e & 1) r = r * b % Q;
    b = b * b % Q;
    e >>= 1;
  }
  return r;
}

static void ntt(uint64_t *a, uint64_t w) {
  for (int i = 1, j = 0; i < N; i++) {
    int bit = N >> 1;
    for (; j & bit; bit >>= 1) j ^= bit;
    j ^= bit;
    if (i < j) { uint64_t t = a[i]; a[i] = a[j]; a[j] = t; }
  }
  for (int len = 2; len <= N; len <<= 1) {
    uint64_t wl = pw_mod(w, N / len);
    for (int i = 0; i < N; i += len) {
      uint64_t wn = 1;
      for (int j = 0; j < len / 2; j++) {
        uint64_t u = a[i + j], v = a[i + j + len / 2] * wn % Q;
        a[i + j] = (u + v) % Q;
        a[i + j + len / 2] = (u + Q - v) % Q;
        wn = wn * wl % Q;
      }
    }
  }
}

static void polymul(const uint64_t *a0, const uint64_t *b0, uint64_t *c, uint64_t psi) {
  uint64_t a[N], b[N], psi_inv = pw_mod(psi, Q - 2), omega = psi * psi % Q, n_inv = pw_mod(N, Q - 2);
  uint64_t p = 1;
  for (int i = 0; i < N; i++) { a[i] = a0[i] * p % Q; b[i] = b0[i] * p % Q; p = p * psi % Q; }
  ntt(a, omega);
  ntt(b, omega);
  for (int i = 0; i < N; i++) c[i] = a[i] * b[i] % Q;
  ntt(c, pw_mod(omega, Q - 2));
  p = n_inv;
  for (int i = 0; i < N; i++) { c[i] = c[i] * p % Q; p = p * psi_inv % Q; }
}

static void schoolbook(const uint64_t *a, const uint64_t *b, uint64_t *c) {
  for (int i = 0; i < N; i++) c[i] = 0;
  for (int i = 0; i < N; i++)
    for (int j = 0; j < N; j++) {
      uint64_t pr = a[i] * b[j] % Q;
      int k = i + j;
      if (k >= N) c[k - N] = (c[k - N] + Q - pr) % Q;
      else c[k] = (c[k] + pr) % Q;
    }
}

int main(void) {
  long long cs = 0;
  uint8_t tags[NH][32];
  for (int i = 0; i < NH; i++) argon2_like(i, tags[i]);
  int n = NH;
  uint8_t level[NH][32];
  memcpy(level, tags, sizeof tags);
  while (n > 1) {
    int m = (n + 1) / 2;
    for (int i = 0; i < m; i++) {
      uint8_t cat[64];
      memcpy(cat, level[2 * i], 32);
      memcpy(cat + 32, level[2 * i + 1 < n ? 2 * i + 1 : 2 * i], 32);
      sha256(cat, 64, level[i]);
    }
    n = m;
  }
  for (int i = 0; i < 32; i++) cs = (cs * 131 + level[0][i]) % MOD;

  uint64_t psi = pw_mod(11, (Q - 1) / (2 * N));
  long long ok = pw_mod(psi, N) == Q - 1;
  uint64_t a[N], b[N], c[N], ref[N];
  for (int pr = 0; pr < PAIRS; pr++) {
    for (int i = 0; i < N; i++) a[i] = next_rand() % Q;
    for (int i = 0; i < N; i++) b[i] = next_rand() % Q;
    polymul(a, b, c, psi);
    if (pr < 2) {
      schoolbook(a, b, ref);
      for (int i = 0; i < N; i++) if (ref[i] != c[i]) ok = 0;
    }
    for (int i = 0; i < N; i++) cs = (cs * 131 + c[i]) % MOD;
  }
  cs = (cs * 131 + ok) % MOD;
  printf("CHECKSUM=%lld\n", cs);
  return 0;
}
