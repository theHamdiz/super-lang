/* 20_end_to_end_lakehouse_service — in-process pipeline:
   ingest JSONL customers + CSV order batches -> filter/join/group -> fixed-point GD linear fit on the
   per-product aggregates -> serve HTTP-like request bytes (bearer-token check via SHA-256, Merkle root of
   the group table in every response) -> checksum of response bytes. */
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define MOD 1000000007LL
#define NCUST 500
#define NPROD 50
#define NREG 8
#define NTIER 4
#define NGRP (NREG * NTIER)
#define BATCHES 6
#define ROWS_PB 4000
#define NREQ 3000
#define NCLIENT 8
#define ITERS 400

static int64_t rng = 20020;
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

/* ---- tiny text helpers ---- */
static int put_str(uint8_t *b, int pos, const char *s) {
  while (*s) b[pos++] = (uint8_t)*s++;
  return pos;
}
static int put_int(uint8_t *b, int pos, int64_t v) { /* v >= 0 */
  char tmp[24];
  int n = 0;
  do { tmp[n++] = (char)('0' + v % 10); v /= 10; } while (v > 0);
  while (n > 0) b[pos++] = (uint8_t)tmp[--n];
  return pos;
}
static int put_hex(uint8_t *b, int pos, const uint8_t *d, int n) {
  for (int i = 0; i < n; i++) {
    int hi = d[i] >> 4, lo = d[i] & 15;
    b[pos++] = (uint8_t)(hi < 10 ? '0' + hi : 'a' + hi - 10);
    b[pos++] = (uint8_t)(lo < 10 ? '0' + lo : 'a' + lo - 10);
  }
  return pos;
}
/* parse a decimal run starting at p (stops at first non-digit); returns -1 if no digits */
static int64_t parse_int(const uint8_t *b, int *p) {
  int64_t v = 0;
  int have = 0;
  while (b[*p] >= '0' && b[*p] <= '9') { v = v * 10 + (b[*p] - '0'); (*p)++; have = 1; }
  return have ? v : -1;
}
/* index of first occurrence of pat in b[from..len), or -1 */
static int find(const uint8_t *b, int len, const char *pat, int from) {
  int m = (int)strlen(pat);
  for (int i = from; i + m <= len; i++) {
    int ok = 1;
    for (int j = 0; j < m; j++) if (b[i + j] != (uint8_t)pat[j]) { ok = 0; break; }
    if (ok) return i;
  }
  return -1;
}
static int64_t sdiv(int64_t a, int64_t d) { return a >= 0 ? a / d : -((-a) / d); }

static int64_t region_of[NCUST], tier_of[NCUST];
static int64_t grp_cnt[NGRP], grp_qty[NGRP], grp_rev[NGRP];
static int64_t prod_cnt[NPROD], prod_qty[NPROD], prod_rev[NPROD], prod_price[NPROD];
static int64_t reg_prod_rev[NREG * NPROD];
static uint8_t cbuf[NCUST * 48], obuf[ROWS_PB * 40 + 128];
static uint8_t tokens[NCLIENT][16];
static uint8_t root_hex[64];
static int64_t pred_cents[NPROD];

int main(void) {
  crc_init();
  /* ---- ingest customers (JSONL) ---- */
  int clen = 0;
  for (int i = 0; i < NCUST; i++) {
    int64_t id = (int64_t)i * 7919 % NCUST, r = next_rand() % NREG, t = next_rand() % NTIER;
    clen = put_str(cbuf, clen, "{\"id\":");
    clen = put_int(cbuf, clen, id);
    clen = put_str(cbuf, clen, ",\"region\":");
    clen = put_int(cbuf, clen, r);
    clen = put_str(cbuf, clen, ",\"tier\":");
    clen = put_int(cbuf, clen, t);
    clen = put_str(cbuf, clen, "}\n");
  }
  cbuf[clen] = 0;
  for (int64_t i = 0; i < NCUST; i++) region_of[i] = tier_of[i] = -1;
  int ncust = 0;
  for (int p = 0; p < clen;) {
    while (p < clen && !(cbuf[p] >= '0' && cbuf[p] <= '9')) p++;
    int64_t id = parse_int(cbuf, &p);
    while (cbuf[p] < '0' || cbuf[p] > '9') p++;
    int64_t r = parse_int(cbuf, &p);
    while (cbuf[p] < '0' || cbuf[p] > '9') p++;
    int64_t t = parse_int(cbuf, &p);
    region_of[id] = r; tier_of[id] = t; ncust++;
    while (p < clen && cbuf[p] != '\n') p++;
    p++;
  }
  /* ---- ingest + filter/join/group order batches (CSV) ---- */
  int64_t oid = 0, rows = 0, null_cust = 0, bad_qty = 0, valid = 0;
  for (int b = 0; b < BATCHES; b++) {
    int olen = put_str(obuf, 0, "order_id,cust_id,product,qty,price_cents\n");
    for (int i = 0; i < ROWS_PB; i++) {
      int64_t nf = next_rand() % 100, c = next_rand() % NCUST, p = next_rand() % NPROD, q = next_rand() % 9,
              pr = 100 + p * 37 + next_rand() % 200;
      olen = put_int(obuf, olen, oid++);
      obuf[olen++] = ',';
      if (nf != 0) olen = put_int(obuf, olen, c);
      obuf[olen++] = ',';
      olen = put_int(obuf, olen, p);
      obuf[olen++] = ',';
      olen = put_int(obuf, olen, q);
      obuf[olen++] = ',';
      olen = put_int(obuf, olen, pr);
      obuf[olen++] = '\n';
    }
    int pos = 0;
    while (obuf[pos] != '\n') pos++;
    pos++;
    while (pos < olen) {
      int64_t f[5];
      for (int k = 0; k < 5; k++) {
        f[k] = parse_int(obuf, &pos);
        pos++; /* skip ',' or '\n' */
      }
      rows++;
      if (f[1] < 0) { null_cust++; continue; }
      if (f[3] <= 0) { bad_qty++; continue; }
      valid++;
      int64_t rev = f[3] * f[4], g = region_of[f[1]] * NTIER + tier_of[f[1]];
      grp_cnt[g]++; grp_qty[g] += f[3]; grp_rev[g] += rev;
      prod_cnt[f[2]]++; prod_qty[f[2]] += f[3]; prod_rev[f[2]] += rev; prod_price[f[2]] += f[4];
      reg_prod_rev[region_of[f[1]] * NPROD + f[2]] += rev;
    }
  }
  /* ---- fit y = w*x + b (w in Q16) by gradient descent on per-product aggregates ---- */
  int64_t xs[NPROD], ys[NPROD];
  int n = 0;
  for (int p = 0; p < NPROD; p++)
    if (prod_cnt[p] > 0) { xs[n] = prod_price[p] / prod_cnt[p] / 16; ys[n] = prod_rev[p] / prod_cnt[p] / 16; n++; }
  int64_t w = 0, bias = 0;
  for (int it = 0; it < ITERS; it++) {
    int64_t gw = 0, gb = 0;
    for (int i = 0; i < n; i++) {
      int64_t err = sdiv(w * xs[i], 65536) + bias - ys[i];
      gw += err * xs[i];
      gb += err;
    }
    w -= sdiv(gw * 65536, (int64_t)n * 8000);
    bias -= sdiv(gb, (int64_t)n * 4);
  }
  int64_t loss = 0;
  for (int i = 0; i < n; i++) {
    int64_t err = sdiv(w * xs[i], 65536) + bias - ys[i];
    loss += err * err;
  }
  for (int p = 0; p < NPROD; p++) {
    int64_t x = prod_cnt[p] > 0 ? prod_price[p] / prod_cnt[p] / 16 : 0;
    int64_t y = sdiv(w * x, 65536) + bias;
    pred_cents[p] = (y > 0 ? y : 0) * 16;
  }
  /* ---- merkle root over the region x tier group table ---- */
  uint8_t level[NGRP][32];
  for (int g = 0; g < NGRP; g++) {
    uint8_t leaf[32];
    int64_t vals[4] = {g, grp_cnt[g], grp_qty[g], grp_rev[g]};
    for (int k = 0; k < 4; k++)
      for (int j = 0; j < 8; j++) leaf[k * 8 + j] = (uint8_t)((uint64_t)vals[k] >> (56 - 8 * j));
    sha256(leaf, 32, level[g]);
  }
  for (int m = NGRP; m > 1; m /= 2)
    for (int i = 0; i < m / 2; i++) {
      uint8_t cat[64];
      memcpy(cat, level[2 * i], 32);
      memcpy(cat + 32, level[2 * i + 1], 32);
      sha256(cat, 64, level[i]);
    }
  put_hex(root_hex, 0, level[0], 32);
  /* ---- client tokens ---- */
  for (int k = 0; k < NCLIENT; k++) {
    uint8_t m[5] = {'k', 'e', 'y', '-', (uint8_t)('0' + k)}, d[32];
    sha256(m, 5, d);
    put_hex(tokens[k], 0, d, 8);
  }
  /* ---- serve ---- */
  int64_t ok200 = 0, un401 = 0, nf404 = 0, total_bytes = 0, resp_cs = 0;
  uint8_t req[512], body[512], resp[1024];
  for (int i = 0; i < NREQ; i++) {
    int64_t k = next_rand() % NCLIENT, p = next_rand() % 60, rg = next_rand() % NREG, bad = next_rand() % 100,
            at = next_rand() % 16;
    int rl = put_str(req, 0, "GET /v1/product/");
    rl = put_int(req, rl, p);
    rl = put_str(req, rl, "?region=");
    rl = put_int(req, rl, rg);
    rl = put_str(req, rl, " HTTP/1.1\r\nHost: lake\r\nX-Client: ");
    rl = put_int(req, rl, k);
    rl = put_str(req, rl, "\r\nAuthorization: Bearer ");
    int tpos = rl;
    memcpy(req + rl, tokens[k], 16);
    rl += 16;
    if (bad < 8) req[tpos + at] = 'z';
    rl = put_str(req, rl, "\r\n\r\n");
    /* server side: parse request bytes */
    int status = 400;
    int bl = 0;
    int64_t pp = -1, rr = -1, kk = -1;
    if (rl > 16 && memcmp(req, "GET /v1/product/", 16) == 0) {
      int q = 16;
      pp = parse_int(req, &q);
      q += 8; /* "?region=" */
      rr = parse_int(req, &q);
      int xc = find(req, rl, "X-Client: ", q);
      int bt = find(req, rl, "Bearer ", q);
      if (xc >= 0 && bt >= 0) {
        int a = xc + 10;
        kk = parse_int(req, &a);
        if (kk >= 0 && kk < NCLIENT && bt + 7 + 16 <= rl && memcmp(req + bt + 7, tokens[kk], 16) == 0) {
          if (pp >= 0 && pp < NPROD && rr >= 0 && rr < NREG && prod_cnt[pp] > 0) status = 200;
          else status = 404;
        } else status = 401;
      }
    }
    if (status == 200) {
      bl = put_str(body, 0, "{\"product\":");
      bl = put_int(body, bl, pp);
      bl = put_str(body, bl, ",\"region\":");
      bl = put_int(body, bl, rr);
      bl = put_str(body, bl, ",\"orders\":");
      bl = put_int(body, bl, prod_cnt[pp]);
      bl = put_str(body, bl, ",\"revenue\":");
      bl = put_int(body, bl, prod_rev[pp]);
      bl = put_str(body, bl, ",\"region_revenue\":");
      bl = put_int(body, bl, reg_prod_rev[rr * NPROD + pp]);
      bl = put_str(body, bl, ",\"forecast\":");
      bl = put_int(body, bl, pred_cents[pp]);
      bl = put_str(body, bl, ",\"root\":\"");
      bl = put_hex(body, bl, level[0], 32);
      bl = put_str(body, bl, "\"}");
      ok200++;
    } else if (status == 404) {
      bl = put_str(body, 0, "{\"error\":\"not found\"}");
      nf404++;
    } else {
      bl = put_str(body, 0, status == 401 ? "{\"error\":\"unauthorized\"}" : "{\"error\":\"bad request\"}");
      if (status == 401) un401++;
    }
    int ol = put_str(resp, 0, status == 200 ? "HTTP/1.1 200 OK\r\nContent-Length: "
                              : status == 404 ? "HTTP/1.1 404 Not Found\r\nContent-Length: "
                              : status == 401 ? "HTTP/1.1 401 Unauthorized\r\nContent-Length: "
                                              : "HTTP/1.1 400 Bad Request\r\nContent-Length: ");
    ol = put_int(resp, ol, bl);
    ol = put_str(resp, ol, "\r\n\r\n");
    memcpy(resp + ol, body, (size_t)bl);
    ol += bl;
    total_bytes += ol;
    resp_cs = (resp_cs * 131 + crc32(resp, (size_t)ol)) % MOD;
  }
  /* ---- checksum ---- */
  fold(ncust); fold(rows); fold(null_cust); fold(bad_qty); fold(valid);
  for (int g = 0; g < NGRP; g++) { fold(grp_cnt[g]); fold(grp_qty[g]); fold(grp_rev[g]); }
  for (int p = 0; p < NPROD; p++) { fold(prod_cnt[p]); fold(prod_rev[p]); }
  fold(w); fold(bias); fold(loss);
  for (int i = 0; i < 32; i++) fold(level[0][i]);
  fold(ok200); fold(un401); fold(nf404); fold(total_bytes); fold(resp_cs);
  printf("CHECKSUM=%lld\n", (long long)cs);
  return 0;
}
