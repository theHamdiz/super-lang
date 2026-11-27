/* 14_http_router_throughput — 30k simulated HTTP requests: parse bytes -> radix route match ->
   HMAC-SHA256 bearer-token check -> JSON body validation -> status code checksum */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define NREQ 30000
#define NOW 1700000000LL
#define BUF 512
#define STRIDE 9
#define MAXNODES 64
#define POOL 256

static const uint32_t K[64] = {
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2};

static uint32_t rotr(uint32_t x, int n) { return (x >> n) | (x << (32 - n)); }

static void sha256(const uint8_t *msg, size_t len, uint8_t out[32]) {
  uint32_t h[8] = {0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19};
  uint8_t buf[256];
  size_t total = ((len + 9 + 63) / 64) * 64;
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
    uint32_t a = h[0], b = h[1], c = h[2], d = h[3], e = h[4], f = h[5], g = h[6], hh = h[7];
    for (int i = 0; i < 64; i++) {
      uint32_t t1 = hh + (rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)) + ((e & f) ^ (~e & g)) + K[i] + w[i];
      uint32_t t2 = (rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)) + ((a & b) ^ (a & c) ^ (b & c));
      hh = g; g = f; f = e; e = d + t1; d = c; c = b; b = a; a = t1 + t2;
    }
    h[0] += a; h[1] += b; h[2] += c; h[3] += d; h[4] += e; h[5] += f; h[6] += g; h[7] += hh;
  }
  for (int i = 0; i < 8; i++) {
    out[4 * i] = (uint8_t)(h[i] >> 24); out[4 * i + 1] = (uint8_t)(h[i] >> 16);
    out[4 * i + 2] = (uint8_t)(h[i] >> 8); out[4 * i + 3] = (uint8_t)h[i];
  }
}

static uint8_t KEY[32];

static void hmac(const uint8_t *msg, size_t len, uint8_t out[32]) {
  uint8_t inner[64 + 64], outer[64 + 32], digest[32];
  for (int i = 0; i < 64; i++) {
    uint8_t k = i < 32 ? KEY[i] : 0;
    inner[i] = k ^ 0x36;
    outer[i] = k ^ 0x5c;
  }
  memcpy(inner + 64, msg, len);
  sha256(inner, 64 + len, digest);
  memcpy(outer + 64, digest, 32);
  sha256(outer, 64 + 32, out);
}

static int64_t rng = 140014;
static int64_t lcg(void) {
  rng = (rng * 1103515245 + 12345) % 2147483648;
  return rng;
}
static int64_t draw(int64_t m) { return lcg() / 256 % m; }

/* ---------------- request builder (client side) ---------------- */
static int put_lit(uint8_t *b, int n, const char *s) {
  while (*s) b[n++] = (uint8_t)*s++;
  return n;
}
static int put_dec(uint8_t *b, int n, int64_t v) {
  uint8_t t[24];
  int k = 0;
  if (v == 0) t[k++] = '0';
  while (v > 0) { t[k++] = (uint8_t)('0' + v % 10); v /= 10; }
  while (k > 0) b[n++] = t[--k];
  return n;
}
static int put_chars(uint8_t *b, int n, int cnt) {
  for (int i = 0; i < cnt; i++) b[n++] = (uint8_t)('a' + draw(26));
  return n;
}
static int put_id(uint8_t *b, int n) {
  int64_t bad = draw(50), idv = 1 + draw(99999);
  return bad == 0 ? put_lit(b, n, "abc") : put_dec(b, n, idv);
}

static const char HEX[] = "0123456789abcdef";

static int build_request(uint8_t *r) {
  int n = 0;
  uint8_t path[96], body[160], tok[64];
  int pl = 0, bl = 0, tl = 0, method = 0, has_body = 0;
  int64_t a = draw(100);
  if (a < 5) { pl = put_lit(path, pl, "/health"); }
  else if (a < 15) { pl = put_lit(path, pl, "/users"); }
  else if (a < 35) { pl = put_lit(path, pl, "/users/"); pl = put_id(path, pl); }
  else if (a < 45) { pl = put_lit(path, pl, "/users/"); pl = put_id(path, pl); pl = put_lit(path, pl, "/orders"); }
  else if (a < 55) { method = 1; has_body = 1; pl = put_lit(path, pl, "/orders"); }
  else if (a < 65) { pl = put_lit(path, pl, "/orders/"); pl = put_id(path, pl); }
  else if (a < 70) { method = 2; has_body = 1; pl = put_lit(path, pl, "/orders/"); pl = put_id(path, pl); }
  else if (a < 75) { method = 3; pl = put_lit(path, pl, "/orders/"); pl = put_id(path, pl); }
  else if (a < 85) {
    pl = put_lit(path, pl, "/static/a"); pl = put_dec(path, pl, draw(10));
    pl = put_lit(path, pl, "/f"); pl = put_dec(path, pl, draw(100)); pl = put_lit(path, pl, ".js");
  } else if (a < 90) { method = 1; has_body = 1; pl = put_lit(path, pl, "/auth/login"); }
  else if (a < 95) {
    pl = put_lit(path, pl, "/v1/items/"); pl = put_id(path, pl);
    pl = put_lit(path, pl, "/tags/t"); pl = put_dec(path, pl, draw(50));
  } else if (a < 98) { pl = put_lit(path, pl, "/nope/"); pl = put_id(path, pl); }
  else { method = 3; pl = put_lit(path, pl, "/users"); }

  /* bearer token */
  int64_t t = draw(20), uid = 1 + draw(9999), ex = draw(1000);
  int has_tok = t >= 2;
  if (has_tok) {
    int64_t exp = t == 4 ? NOW - 1 - ex : NOW + 1000 + ex;
    uint8_t sb[40], mac[32];
    int sn = put_dec(sb, 0, uid);
    sb[sn++] = '.';
    sn = put_dec(sb, sn, exp);
    hmac(sb, sn, mac);
    if (t < 4) mac[0] ^= 1;
    memcpy(tok, sb, sn);
    tl = sn;
    tok[tl++] = '.';
    for (int i = 0; i < 4; i++) { tok[tl++] = (uint8_t)HEX[mac[i] >> 4]; tok[tl++] = (uint8_t)HEX[mac[i] & 15]; }
  }

  /* JSON body */
  if (has_body) {
    int64_t v = draw(10), ws = draw(2);
    const char *sp = ws ? " " : "";
    if (a >= 45 && a < 55) { /* POST /orders */
      int64_t user = 1 + draw(9999), qty = 1 + draw(100);
      if (v == 0) qty = 0;
      if (v == 3) user = 10000;
      bl = put_lit(body, bl, "{\"user\":"); bl = put_lit(body, bl, sp); bl = put_dec(body, bl, user);
      bl = put_lit(body, bl, ","); bl = put_lit(body, bl, sp);
      bl = put_lit(body, bl, "\"qty\":"); bl = put_lit(body, bl, sp); bl = put_dec(body, bl, qty);
      /* sku characters are drawn even when the field is omitted, keeping the stream aligned */
      uint8_t sku[4];
      for (int i = 0; i < 4; i++) sku[i] = (uint8_t)('a' + draw(26));
      if (v != 1) {
        bl = put_lit(body, bl, ","); bl = put_lit(body, bl, sp);
        bl = put_lit(body, bl, "\"sku\":"); bl = put_lit(body, bl, sp); body[bl++] = '"';
        for (int i = 0; i < 4; i++) body[bl++] = sku[i];
        body[bl++] = '"';
      }
      body[bl++] = '}';
      if (v == 2) bl--;
    } else if (a >= 65 && a < 70) { /* PUT /orders/:id */
      int64_t qty = 1 + draw(100), nl = 3 + draw(5);
      if (v == 0) qty = 101;
      bl = put_lit(body, bl, "{");
      if (v != 1) {
        bl = put_lit(body, bl, "\"qty\":"); bl = put_lit(body, bl, sp); bl = put_dec(body, bl, qty);
        bl = put_lit(body, bl, ","); bl = put_lit(body, bl, sp);
      }
      bl = put_lit(body, bl, "\"note\":"); bl = put_lit(body, bl, sp); body[bl++] = '"';
      bl = put_chars(body, bl, (int)nl);
      body[bl++] = '"'; body[bl++] = '}';
      if (v == 2) bl--;
    } else { /* POST /auth/login */
      int64_t ul = 4 + draw(6);
      uint8_t un[16];
      for (int i = 0; i < ul; i++) un[i] = (uint8_t)('a' + draw(26));
      int64_t pwl = 6 + draw(7);
      uint8_t pw[16];
      for (int i = 0; i < pwl; i++) pw[i] = (uint8_t)('a' + draw(26));
      if (v == 0) pwl = 0;
      bl = put_lit(body, bl, "{");
      if (v != 1) {
        bl = put_lit(body, bl, "\"user\":"); bl = put_lit(body, bl, sp); body[bl++] = '"';
        for (int i = 0; i < ul; i++) body[bl++] = un[i];
        body[bl++] = '"'; bl = put_lit(body, bl, ","); bl = put_lit(body, bl, sp);
      }
      bl = put_lit(body, bl, "\"pass\":"); bl = put_lit(body, bl, sp); body[bl++] = '"';
      for (int i = 0; i < pwl; i++) body[bl++] = pw[i];
      body[bl++] = '"'; body[bl++] = '}';
      if (v == 2) body[bl++] = 'x';
    }
  }

  /* assemble */
  n = put_lit(r, n, method == 0 ? "GET " : method == 1 ? "POST " : method == 2 ? "PUT " : "DELETE ");
  memcpy(r + n, path, pl); n += pl;
  n = put_lit(r, n, " HTTP/1.1\r\nHost: api.local\r\n");
  if (has_tok) { n = put_lit(r, n, "Authorization: Bearer "); memcpy(r + n, tok, tl); n += tl; n = put_lit(r, n, "\r\n"); }
  if (has_body) { n = put_lit(r, n, "Content-Length: "); n = put_dec(r, n, bl); n = put_lit(r, n, "\r\n"); }
  n = put_lit(r, n, "\r\n");
  memcpy(r + n, body, bl); n += bl;
  return n;
}

/* ---------------- radix router ---------------- */
static int64_t trie[MAXNODES * STRIDE];
static uint8_t pool[POOL];
static int nnodes = 0, poolen = 0;

static int new_node(int kind, int off, int len, int next) {
  int n = nnodes++;
  int64_t *t = &trie[n * STRIDE];
  t[0] = kind; t[1] = off; t[2] = len; t[3] = -1; t[4] = next;
  t[5] = t[6] = t[7] = t[8] = -1;
  return n;
}

static void add_route(const char *tpl, int method, int rid) {
  int cur = 0, i = 1, n = (int)strlen(tpl);
  while (i <= n) {
    int e = i;
    while (e < n && tpl[e] != '/') e++;
    int kind = tpl[i] == ':' ? 1 : tpl[i] == '*' ? 2 : 0;
    int len = e - i, found = -1;
    for (int c = (int)trie[cur * STRIDE + 3]; c >= 0 && found < 0; c = (int)trie[c * STRIDE + 4]) {
      if (trie[c * STRIDE] != kind) continue;
      if (kind != 0) { found = c; break; }
      if (trie[c * STRIDE + 2] == len && memcmp(pool + trie[c * STRIDE + 1], tpl + i, len) == 0) found = c;
    }
    if (found < 0) {
      int off = poolen;
      if (kind == 0) { memcpy(pool + poolen, tpl + i, len); poolen += len; }
      found = new_node(kind, off, kind == 0 ? len : 0, (int)trie[cur * STRIDE + 3]);
      trie[cur * STRIDE + 3] = found;
    }
    cur = found;
    i = e + 1;
  }
  trie[cur * STRIDE + 5 + method] = rid;
}

static const int AUTH[11] = {0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 1};

/* ---------------- server side ---------------- */
static int is_digit(uint8_t c) { return c >= '0' && c <= '9'; }

static int starts_with(const uint8_t *b, int p, int n, const char *lit) {
  int l = (int)strlen(lit);
  if (p + l > n) return 0;
  return memcmp(b + p, lit, l) == 0;
}

/* parse decimal in b[p..e): returns -1 when empty / non-digit / over 10 digits */
static int64_t parse_digits(const uint8_t *b, int p, int e) {
  if (e <= p || e - p > 10) return -1;
  int64_t v = 0;
  for (int i = p; i < e; i++) {
    if (!is_digit(b[i])) return -1;
    v = v * 10 + (b[i] - '0');
  }
  return v;
}

/* returns 1 and sets *uid when token is well-formed, MAC matches and unexpired */
static int verify_token(const uint8_t *b, int off, int len, int64_t *uid) {
  int e = off + len, d1 = -1, d2 = -1;
  for (int i = off; i < e; i++) {
    if (b[i] == '.') { if (d1 < 0) d1 = i; else if (d2 < 0) d2 = i; else return 0; }
  }
  if (d1 < 0 || d2 < 0 || e - d2 - 1 != 8) return 0;
  int64_t u = parse_digits(b, off, d1), ex = parse_digits(b, d1 + 1, d2);
  if (u < 0 || ex < 0) return 0;
  uint8_t mac[32];
  hmac(b + off, d2 - off, mac);
  for (int i = 0; i < 4; i++) {
    if (b[d2 + 1 + 2 * i] != (uint8_t)HEX[mac[i] >> 4] || b[d2 + 2 + 2 * i] != (uint8_t)HEX[mac[i] & 15]) return 0;
  }
  if (ex <= NOW) return 0;
  *uid = u;
  return 1;
}

static void skip_ws(const uint8_t *b, int *p, int e) {
  while (*p < e && (b[*p] == ' ' || b[*p] == '\t' || b[*p] == '\n' || b[*p] == '\r')) (*p)++;
}

static int key_id(const uint8_t *b, int p, int e) {
  int l = e - p;
  if (l == 4 && !memcmp(b + p, "user", 4)) return 1;
  if (l == 3 && !memcmp(b + p, "qty", 3)) return 2;
  if (l == 3 && !memcmp(b + p, "sku", 3)) return 3;
  if (l == 4 && !memcmp(b + p, "note", 4)) return 4;
  if (l == 4 && !memcmp(b + p, "pass", 4)) return 5;
  return 0;
}

/* f[id*3] present, f[id*3+1] type (1 int, 2 str), f[id*3+2] value / string length */
static int parse_json(const uint8_t *b, int off, int len, int64_t *f) {
  int p = off, e = off + len;
  for (int i = 0; i < 18; i++) f[i] = 0;
  skip_ws(b, &p, e);
  if (p >= e || b[p] != '{') return 0;
  p++;
  skip_ws(b, &p, e);
  if (p < e && b[p] == '}') { p++; skip_ws(b, &p, e); return p == e; }
  for (;;) {
    if (p >= e || b[p] != '"') return 0;
    int ks = ++p;
    while (p < e && b[p] != '"') { if (b[p] == '\\') return 0; p++; }
    if (p >= e) return 0;
    int id = key_id(b, ks, p);
    p++;
    skip_ws(b, &p, e);
    if (p >= e || b[p] != ':') return 0;
    p++;
    skip_ws(b, &p, e);
    if (p >= e) return 0;
    int64_t type, val;
    if (b[p] == '"') {
      int vs = ++p;
      while (p < e && b[p] != '"') { if (b[p] == '\\') return 0; p++; }
      if (p >= e) return 0;
      type = 2; val = p - vs;
      p++;
    } else {
      int neg = 0;
      if (b[p] == '-') { neg = 1; p++; }
      int ds = p;
      while (p < e && is_digit(b[p])) p++;
      int64_t v = parse_digits(b, ds, p);
      if (v < 0) return 0;
      type = 1; val = neg ? -v : v;
    }
    if (id > 0) { f[id * 3] = 1; f[id * 3 + 1] = type; f[id * 3 + 2] = val; }
    skip_ws(b, &p, e);
    if (p >= e) return 0;
    if (b[p] == ',') { p++; skip_ws(b, &p, e); continue; }
    if (b[p] == '}') { p++; break; }
    return 0;
  }
  skip_ws(b, &p, e);
  return p == e;
}

static int has_int(const int64_t *f, int id, int64_t lo, int64_t hi) {
  return f[id * 3] && f[id * 3 + 1] == 1 && f[id * 3 + 2] >= lo && f[id * 3 + 2] <= hi;
}
static int has_str(const int64_t *f, int id, int64_t lo, int64_t hi) {
  return f[id * 3] && f[id * 3 + 1] == 2 && f[id * 3 + 2] >= lo && f[id * 3 + 2] <= hi;
}

static int body_valid(int rid, const uint8_t *b, int off, int len) {
  int64_t f[18];
  if (!parse_json(b, off, len, f)) return 0;
  if (rid == 4) return has_int(f, 1, 1, 9999) && has_int(f, 2, 1, 100) && has_str(f, 3, 4, 4);
  if (rid == 6) return has_int(f, 2, 1, 100) && (!f[4 * 3] || has_str(f, 4, 0, 1000));
  return has_str(f, 1, 1, 64) && has_str(f, 5, 1, 64);
}

static int handle(const uint8_t *b, int n, int64_t *pid_out, int *route_out) {
  /* request line */
  int p = 0, method;
  if (starts_with(b, 0, n, "GET ")) { method = 0; p = 4; }
  else if (starts_with(b, 0, n, "POST ")) { method = 1; p = 5; }
  else if (starts_with(b, 0, n, "PUT ")) { method = 2; p = 4; }
  else if (starts_with(b, 0, n, "DELETE ")) { method = 3; p = 7; }
  else return 400;
  int po = p;
  while (p < n && b[p] != ' ') p++;
  int pe = p;
  while (p < n && b[p] != '\r') p++;
  p += 2;
  /* headers */
  int tok_off = -1, tok_len = 0;
  int64_t clen = -1;
  while (p < n) {
    if (b[p] == '\r') { p += 2; break; }
    int le = p;
    while (le < n && b[le] != '\r') le++;
    if (starts_with(b, p, n, "Authorization: Bearer ")) { tok_off = p + 22; tok_len = le - tok_off; }
    else if (starts_with(b, p, n, "Content-Length: ")) { clen = parse_digits(b, p + 16, le); }
    p = le + 2;
  }
  int body_off = p, body_len = n - p;
  if (clen >= 0 && clen != body_len) return 400;

  /* route match */
  int plen = pe - po, cur = 0, found = 1, nparam = 0, bad_id = 0;
  int64_t pid = -1;
  if (plen < 1 || b[po] != '/') found = 0;
  int i = 1;
  while (found && i <= plen) {
    int e = i;
    while (e < plen && b[po + e] != '/') e++;
    int sl = e - i;
    if (sl == 0) { found = 0; break; }
    int st = -1, pm = -1, wc = -1;
    for (int c = (int)trie[cur * STRIDE + 3]; c >= 0; c = (int)trie[c * STRIDE + 4]) {
      int k = (int)trie[c * STRIDE];
      if (k == 0) {
        if (trie[c * STRIDE + 2] == sl && memcmp(pool + trie[c * STRIDE + 1], b + po + i, sl) == 0) { st = c; break; }
      } else if (k == 1) pm = c;
      else wc = c;
    }
    if (st >= 0) cur = st;
    else if (pm >= 0) {
      cur = pm;
      if (nparam == 0) {
        pid = parse_digits(b, po + i, po + e);
        if (pid < 0) bad_id = 1;
      }
      nparam++;
    } else if (wc >= 0) { cur = wc; break; }
    else { found = 0; break; }
    i = e + 1;
  }
  int rid = -1;
  if (found) {
    rid = (int)trie[cur * STRIDE + 5 + method];
    if (rid < 0) {
      int any = trie[cur * STRIDE + 5] >= 0 || trie[cur * STRIDE + 6] >= 0 || trie[cur * STRIDE + 7] >= 0 || trie[cur * STRIDE + 8] >= 0;
      return any ? 405 : 404;
    }
  } else return 404;
  *route_out = rid;
  *pid_out = bad_id ? 0 : (pid < 0 ? 0 : pid);

  int64_t uid = 0;
  if (AUTH[rid]) {
    if (tok_off < 0 || !verify_token(b, tok_off, tok_len, &uid)) return 401;
    if ((rid == 6 || rid == 7) && uid % 13 == 0) return 403;
  }
  if (bad_id) return 400;
  if ((rid == 4 || rid == 6 || rid == 9) && !body_valid(rid, b, body_off, body_len)) return 400;
  switch (rid) {
    case 2: return pid > 90000 ? 404 : 200;
    case 4: return 201;
    case 5: return pid % 7 == 0 ? 404 : 200;
    case 7: return 204;
    default: return 200;
  }
}

int main(void) {
  for (int i = 0; i < 32; i++) KEY[i] = (uint8_t)((i * 37 + 11) % 256);
  new_node(0, 0, 0, -1);
  add_route("/health", 0, 0);
  add_route("/users", 0, 1);
  add_route("/users/:id", 0, 2);
  add_route("/users/:id/orders", 0, 3);
  add_route("/orders", 1, 4);
  add_route("/orders/:id", 0, 5);
  add_route("/orders/:id", 2, 6);
  add_route("/orders/:id", 3, 7);
  add_route("/static/*path", 0, 8);
  add_route("/auth/login", 1, 9);
  add_route("/v1/items/:id/tags/:tag", 0, 10);

  int64_t cs = 0, sum_pid = 0, ok2xx = 0;
  int64_t by_status[600] = {0};
  uint8_t req[BUF];
  for (int i = 0; i < NREQ; i++) {
    int n = build_request(req);
    int64_t pid = 0;
    int rid = -1;
    int st = handle(req, n, &pid, &rid);
    by_status[st]++;
    cs += (int64_t)st * (i % 97 + 1);
    sum_pid += pid;
    if (st < 300) ok2xx++;
  }
  int64_t mix = 0;
  for (int s = 0; s < 600; s++) mix += by_status[s] * (s % 53 + 1) * (s / 100 + 1);
  cs += sum_pid + 5 * ok2xx + 7 * mix;
  printf("CHECKSUM=%lld\n", (long long)cs);
  return 0;
}
