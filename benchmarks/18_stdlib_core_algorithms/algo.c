/* 18_stdlib_core_algorithms — one checksum from four std-style kernels:
   (A) KMP-DFA "regex" field extraction over 3000 generated log lines,
   (B) strict UTF-8 validation/counting over 60 documents (some corrupted),
   (C) SQL-ish filter / group-by / order-by-limit / join over 6000 rows x 24 queries,
   (D) HM-style type unification (union-find, iterative) over 8504 type nodes. */
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define MOD 1000000007LL

static int64_t rng = 18018;
static int64_t next_rand(void) { return rng = rng * 48271 % 2147483647; }
static int64_t cs = 0;
static void fold(int64_t v) { cs = (cs * 131 + v) % MOD; }

/* ---------- (A) log extraction ---------- */
#define NL 3000
static uint8_t line[160];
static int pos = 0;

static void put_lit(const char *s) { while (*s) line[pos++] = (uint8_t)*s++; }
static void put_uint(int64_t v) {
  char t[24];
  int n = 0;
  do { t[n++] = (char)('0' + v % 10); v /= 10; } while (v > 0);
  while (n > 0) line[pos++] = (uint8_t)t[--n];
}
static void put2(int64_t v) { line[pos++] = (uint8_t)('0' + v / 10); line[pos++] = (uint8_t)('0' + v % 10); }

/* KMP automaton: dfa[state * 256 + byte] */
static void build_dfa(const char *needle, int len, int *dfa) {
  memset(dfa, 0, sizeof(int) * len * 256);
  dfa[(uint8_t)needle[0]] = 1;
  int x = 0;
  for (int j = 1; j < len; j++) {
    for (int c = 0; c < 256; c++) dfa[j * 256 + c] = dfa[x * 256 + c];
    dfa[j * 256 + (uint8_t)needle[j]] = j + 1;
    x = dfa[x * 256 + (uint8_t)needle[j]];
  }
}

/* first `<needle><digits+>` match in line[0..e) as an integer; -1 when absent */
static int64_t extract(int e, const int *dfa, int len) {
  int st = 0, i = 0;
  while (i < e) {
    st = dfa[st * 256 + line[i]];
    i++;
    if (st == len) {
      int64_t v = 0;
      int nd = 0;
      while (i < e && line[i] >= '0' && line[i] <= '9') { v = v * 10 + (line[i] - '0'); nd++; i++; }
      if (nd > 0) return v;
      st = 0;
    }
  }
  return -1;
}

static void part_a(void) {
  static const char *LEVELS[4] = {"INFO", "WARN", "ERROR", "DEBUG"};
  static const int CODES[9] = {200, 201, 204, 301, 400, 401, 404, 500, 503};
  static int d_status[7 * 256], d_took[5 * 256], d_user[5 * 256];
  build_dfa("status=", 7, d_status);
  build_dfa("took=", 5, d_took);
  build_dfa("user=", 5, d_user);
  int64_t cls[8] = {0}, took_sum = 0, took_max = 0, took_n = 0, user_sum = 0, bad = 0, bytes = 0;
  for (int i = 0; i < NL; i++) {
    int64_t mo = 1 + next_rand() % 12, dd = 1 + next_rand() % 28, hh = next_rand() % 24;
    int64_t mi = next_rand() % 60, ss = next_rand() % 60, lv = next_rand() % 4;
    int64_t svc = next_rand() % 100, uid = next_rand() % 100000, sidx = next_rand() % 9, ms = next_rand() % 3000;
    pos = 0;
    put_lit("2026-"); put2(mo); put_lit("-"); put2(dd); put_lit(" ");
    put2(hh); put_lit(":"); put2(mi); put_lit(":"); put2(ss); put_lit(" ");
    put_lit(LEVELS[lv]); put_lit(" svc-"); put2(svc); put_lit(" user="); put_uint(uid);
    if (i % 13 == 5) put_lit(" status=abc");
    else { put_lit(" status="); put_uint(CODES[sidx]); }
    if (i % 17 != 3) { put_lit(" took="); put_uint(ms); put_lit("ms"); }
    int64_t st = extract(pos, d_status, 7), tk = extract(pos, d_took, 5), us = extract(pos, d_user, 5);
    if (st < 0) bad++; else cls[st / 100]++;
    if (tk >= 0) { took_sum += tk; took_n++; if (tk > took_max) took_max = tk; }
    user_sum += us;
    bytes += pos + 1;
  }
  for (int c = 0; c < 8; c++) fold(cls[c]);
  fold(took_sum); fold(took_max); fold(took_n); fold(user_sum); fold(bad); fold(bytes);
}

/* ---------- (B) UTF-8 ---------- */
#define NDOC 60
#define PERDOC 200

static int utf8_valid(const uint8_t *d, int n) {
  int i = 0;
  while (i < n) {
    int b0 = d[i];
    if (b0 < 128) { i++; continue; }
    if (b0 < 194) return 0;
    if (b0 < 224) {
      if (i + 1 >= n || (d[i + 1] & 0xC0) != 0x80) return 0;
      i += 2;
    } else if (b0 < 240) {
      if (i + 2 >= n || (d[i + 1] & 0xC0) != 0x80 || (d[i + 2] & 0xC0) != 0x80) return 0;
      int cp = (b0 - 224) * 4096 + (d[i + 1] - 128) * 64 + (d[i + 2] - 128);
      if (cp < 2048 || (cp >= 55296 && cp <= 57343)) return 0;
      i += 3;
    } else if (b0 < 245) {
      if (i + 3 >= n || (d[i + 1] & 0xC0) != 0x80 || (d[i + 2] & 0xC0) != 0x80 || (d[i + 3] & 0xC0) != 0x80) return 0;
      int cp = (b0 - 240) * 262144 + (d[i + 1] - 128) * 4096 + (d[i + 2] - 128) * 64 + (d[i + 3] - 128);
      if (cp < 65536 || cp > 1114111) return 0;
      i += 4;
    } else return 0;
  }
  return 1;
}

static int utf8_count(const uint8_t *d, int n) {
  int c = 0;
  for (int i = 0; i < n; i++) if ((d[i] & 0xC0) != 0x80) c++;
  return c;
}

static int encode(int64_t cp, uint8_t *o) {
  if (cp < 128) { o[0] = (uint8_t)cp; return 1; }
  if (cp < 2048) { o[0] = (uint8_t)(192 + cp / 64); o[1] = (uint8_t)(128 + cp % 64); return 2; }
  if (cp < 65536) { o[0] = (uint8_t)(224 + cp / 4096); o[1] = (uint8_t)(128 + cp / 64 % 64); o[2] = (uint8_t)(128 + cp % 64); return 3; }
  o[0] = (uint8_t)(240 + cp / 262144); o[1] = (uint8_t)(128 + cp / 4096 % 64);
  o[2] = (uint8_t)(128 + cp / 64 % 64); o[3] = (uint8_t)(128 + cp % 64);
  return 4;
}

static void part_b(void) {
  static const int BADB[6] = {0x80, 0xC0, 0xC1, 0xF5, 0xFF, 0xE0};
  static uint8_t doc[PERDOC * 4];
  int64_t valid_docs = 0, total_cp = 0;
  for (int d = 0; d < NDOC; d++) {
    int n = 0;
    for (int k = 0; k < PERDOC; k++) {
      int64_t cat = next_rand() % 4, r = next_rand(), cp;
      if (cat == 0) cp = r % 128;
      else if (cat == 1) cp = 128 + r % 1920;
      else if (cat == 2) cp = 2048 + r % 53248;
      else cp = 65536 + r % 1048576;
      n += encode(cp, doc + n);
    }
    if (d % 5 == 2) {
      int64_t p = next_rand() % n, b = next_rand() % 6;
      doc[p] = (uint8_t)BADB[b];
    }
    int ok = utf8_valid(doc, n);
    int cnt = ok ? utf8_count(doc, n) : 0;
    fold(ok); fold(cnt); fold(n);
    valid_docs += ok;
    total_cp += cnt;
  }
  fold(valid_docs); fold(total_cp);
}

/* ---------- (C) SQL-ish ---------- */
#define NR 6000
#define NQ 24
static void part_c(void) {
  static int dept[NR], age[NR], sal[NR], act[NR];
  for (int i = 0; i < NR; i++) {
    dept[i] = (int)(next_rand() % 16);
    age[i] = (int)(18 + next_rand() % 50);
    sal[i] = (int)(30000 + next_rand() % 90000);
    act[i] = next_rand() % 10 != 0;
  }
  for (int q = 0; q < NQ; q++) {
    int64_t age_lo = 18 + next_rand() % 30, sal_max = 40000 + next_rand() % 80000, need = next_rand() % 2;
    int64_t cnt[16] = {0}, sum[16] = {0}, mx[16] = {0};
    for (int i = 0; i < NR; i++) {
      if (age[i] >= age_lo && sal[i] < sal_max && act[i] >= need) {
        int g = dept[i];
        cnt[g]++;
        sum[g] += sal[i];
        if (age[i] > mx[g]) mx[g] = age[i];
      }
    }
    int taken[16] = {0};
    for (int pick = 0; pick < 3; pick++) {
      int best = -1;
      for (int g = 0; g < 16; g++)
        if (cnt[g] >= 5 && !taken[g] && (best < 0 || sum[g] > sum[best])) best = g;
      if (best >= 0) {
        taken[best] = 1;
        fold(best); fold(cnt[best]); fold(sum[best]); fold(mx[best]);
        fold(sum[best] / ((best + 1) * 1000));
      } else fold(999);
    }
  }
}

/* ---------- (D) HM unification ---------- */
#define NVARS 2500
#define NCOMP 6000
#define NTOT (NVARS + 4 + NCOMP)
#define NCON 5000
static int kind[NTOT], na[NTOT], nb[NTOT], parent[NTOT];
static int stk_a[2 * NTOT + 16], stk_b[2 * NTOT + 16];
static int64_t unions = 0;

static int find(int x) {
  while (parent[x] != x) { parent[x] = parent[parent[x]]; x = parent[x]; }
  return x;
}

static int unify(int x, int y) {
  int sp = 0;
  stk_a[sp] = x; stk_b[sp] = y; sp++;
  while (sp > 0) {
    sp--;
    int rx = find(stk_a[sp]), ry = find(stk_b[sp]);
    if (rx == ry) continue;
    int kx = kind[rx], ky = kind[ry];
    if (kx == 0) { parent[rx] = ry; unions++; }
    else if (ky == 0) { parent[ry] = rx; unions++; }
    else if (kx == 1 || ky == 1) {
      if (kx == 1 && ky == 1 && na[rx] == na[ry]) { parent[ry] = rx; unions++; }
      else return 0;
    } else {
      parent[ry] = rx; unions++;
      stk_a[sp] = na[rx]; stk_b[sp] = na[ry]; sp++;
      stk_a[sp] = nb[rx]; stk_b[sp] = nb[ry]; sp++;
    }
  }
  return 1;
}

static void part_d(void) {
  for (int i = 0; i < NTOT; i++) { parent[i] = i; kind[i] = 0; na[i] = -1; nb[i] = -1; }
  for (int c = 0; c < 4; c++) { kind[NVARS + c] = 1; na[NVARS + c] = c; }
  for (int i = NVARS + 4; i < NTOT; i++) {
    kind[i] = 2;
    na[i] = (int)(next_rand() % i);
    nb[i] = (int)(next_rand() % i);
  }
  int64_t fails = 0;
  for (int c = 0; c < NCON; c++) {
    int64_t t = next_rand() % 2;
    int x = (int)(t == 0 ? next_rand() % NVARS : next_rand() % NTOT);
    int y = (int)(next_rand() % NTOT);
    if (!unify(x, y)) fails++;
  }
  int64_t classes = 0;
  fold(fails); fold(unions);
  for (int i = 0; i < NTOT; i++) {
    int r = find(i);
    if (r == i) classes++;
    fold((int64_t)r * 3 + kind[r]);
  }
  fold(classes);
}

int main(void) {
  part_a();
  part_b();
  part_c();
  part_d();
  printf("CHECKSUM=%lld\n", (long long)cs);
  return 0;
}
