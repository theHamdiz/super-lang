/* 17_ai_tensor_hnsw_search — fixed-point (Q10) transformer blocks (causal attention, softmax,
   ReLU FFN, RMS norm) on 12 sequences, then brute-force top-10 cosine search over 4096 unit
   vectors of dim 32 using the pooled embeddings as queries. Integer-only => bit-exact everywhere. */
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define MOD 1000000007LL
#define BIAS (1LL << 40)
#define D 32
#define T 16
#define FF 64
#define LAYERS 2
#define SEQS 12
#define NV 4096
#define K 10

static int64_t rng = 17017;
static int64_t next_rand(void) { return rng = rng * 48271 % 2147483647; }

static int64_t isqrt(int64_t n) {
  if (n == 0) return 0;
  int64_t x = n, y = (x + 1) >> 1;
  while (y < x) { x = y; y = (x + n / x) >> 1; }
  return x;
}

/* exp(-f/1024) in Q10 for f in [0,1024] via 6-term Taylor series */
static int64_t exp_frac(int64_t f) {
  int64_t term = 1024, sum = 1024;
  for (int k = 1; k <= 6; k++) {
    term = ((term * f) >> 10) / k;
    if (k & 1) sum -= term; else sum += term;
  }
  return sum < 0 ? 0 : sum;
}

/* exp(x/1024) in Q10 for x <= 0 */
static int64_t exp_q(int64_t x, int64_t e1) {
  int64_t a = -x, n = a >> 10, f = a & 1023;
  if (n >= 16) return 0;
  int64_t r = exp_frac(f);
  for (int i = 0; i < n; i++) r = (r * e1) >> 10;
  return r;
}

static void matmul(const int64_t *x, const int64_t *w, int64_t *y, int din, int dout) {
  for (int t = 0; t < T; t++)
    for (int o = 0; o < dout; o++) {
      int64_t s = 0;
      for (int i = 0; i < din; i++) s += x[t * din + i] * w[i * dout + o];
      y[t * dout + o] = s >> 10;
    }
}

static void rms_norm(int64_t *h) {
  for (int t = 0; t < T; t++) {
    int64_t ss = 0;
    for (int d = 0; d < D; d++) ss += h[t * D + d] * h[t * D + d];
    int64_t rms = isqrt(ss / D), inv = (1 << 20) / (rms + 1);
    for (int d = 0; d < D; d++) h[t * D + d] = (h[t * D + d] * inv) >> 10;
  }
}

typedef struct { int64_t wq[D * D], wk[D * D], wv[D * D], wo[D * D], w1[D * FF], w2[FF * D]; } Layer;

static void fill_w(int64_t *w, int n) {
  for (int i = 0; i < n; i++) w[i] = next_rand() % 257 - 128;
}

static void block(const Layer *ly, int64_t *h, int64_t e1) {
  static int64_t q[T * D], k[T * D], v[T * D], at[T * D], o[T * D], f[T * FF], g[T * D];
  matmul(h, ly->wq, q, D, D);
  matmul(h, ly->wk, k, D, D);
  matmul(h, ly->wv, v, D, D);
  for (int t = 0; t < T; t++) {
    int64_t sc[T], pr[T], mx = -(1LL << 60), sum = 0;
    for (int j = 0; j <= t; j++) {
      int64_t s = 0;
      for (int d = 0; d < D; d++) s += q[t * D + d] * k[j * D + d];
      sc[j] = s >> 13;
      if (sc[j] > mx) mx = sc[j];
    }
    for (int j = 0; j <= t; j++) { pr[j] = exp_q(sc[j] - mx, e1); sum += pr[j]; }
    for (int j = 0; j <= t; j++) pr[j] = pr[j] * 1024 / sum;
    for (int d = 0; d < D; d++) {
      int64_t s = 0;
      for (int j = 0; j <= t; j++) s += pr[j] * v[j * D + d];
      at[t * D + d] = s >> 10;
    }
  }
  matmul(at, ly->wo, o, D, D);
  for (int i = 0; i < T * D; i++) h[i] += o[i];
  rms_norm(h);
  matmul(h, ly->w1, f, D, FF);
  for (int i = 0; i < T * FF; i++) if (f[i] < 0) f[i] = 0;
  {
    /* second FFN matmul has din = FF */
    for (int t = 0; t < T; t++)
      for (int od = 0; od < D; od++) {
        int64_t s = 0;
        for (int i = 0; i < FF; i++) s += f[t * FF + i] * ly->w2[i * D + od];
        g[t * D + od] = s >> 10;
      }
  }
  for (int i = 0; i < T * D; i++) h[i] += g[i];
  rms_norm(h);
}

static void normalize(const int64_t *raw, int64_t *out) {
  int64_t ss = 0;
  for (int d = 0; d < D; d++) ss += raw[d] * raw[d];
  int64_t nrm = isqrt(ss), inv = (1 << 20) / (nrm + 1);
  for (int d = 0; d < D; d++) out[d] = (raw[d] * inv) >> 10;
}

static int64_t cs = 0;
static void fold(int64_t v) { cs = (cs * 131 + (v + BIAS)) % MOD; }

int main(void) {
  int64_t e1 = exp_frac(1024);
  static Layer layers[LAYERS];
  for (int l = 0; l < LAYERS; l++) {
    fill_w(layers[l].wq, D * D); fill_w(layers[l].wk, D * D); fill_w(layers[l].wv, D * D);
    fill_w(layers[l].wo, D * D); fill_w(layers[l].w1, D * FF); fill_w(layers[l].w2, FF * D);
  }
  static int64_t db[NV][D];
  for (int i = 0; i < NV; i++) {
    int64_t raw[D];
    for (int d = 0; d < D; d++) raw[d] = next_rand() % 2049 - 1024;
    normalize(raw, db[i]);
  }
  for (int s = 0; s < SEQS; s++) {
    int64_t h[T * D], pooled[D], query[D];
    for (int i = 0; i < T * D; i++) h[i] = next_rand() % 2049 - 1024;
    for (int l = 0; l < LAYERS; l++) block(&layers[l], h, e1);
    for (int d = 0; d < D; d++) {
      int64_t sum = 0;
      for (int t = 0; t < T; t++) sum += h[t * D + d];
      pooled[d] = sum >> 4;
      fold(pooled[d]);
    }
    normalize(pooled, query);
    int64_t top_s[K], top_i[K];
    int cnt = 0;
    for (int i = 0; i < NV; i++) {
      int64_t sc = 0;
      for (int d = 0; d < D; d++) sc += query[d] * db[i][d];
      if (cnt < K || sc > top_s[K - 1]) {
        int p = cnt < K ? cnt : K - 1;
        while (p > 0 && top_s[p - 1] < sc) {
          top_s[p] = top_s[p - 1];
          top_i[p] = top_i[p - 1];
          p--;
        }
        top_s[p] = sc;
        top_i[p] = i;
        if (cnt < K) cnt++;
      }
    }
    for (int j = 0; j < K; j++) { fold(top_i[j]); fold(top_s[j]); }
  }
  printf("CHECKSUM=%lld\n", (long long)cs);
  return 0;
}
