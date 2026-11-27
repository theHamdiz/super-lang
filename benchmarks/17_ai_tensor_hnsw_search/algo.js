/* 17_ai_tensor_hnsw_search — fixed-point (Q10) transformer blocks (causal attention, softmax,
   ReLU FFN, RMS norm) on 12 sequences, then brute-force top-10 cosine search over 4096 unit
   vectors of dim 32 using the pooled embeddings as queries. Integer-only => bit-exact everywhere. */
const MOD = 1000000007;
const BIAS = 2 ** 40;
const D = 32, T = 16, FF = 64, LAYERS = 2, SEQS = 12, NV = 4096, K = 10;

let rng = 17017;
const nextRand = () => (rng = (rng * 48271) % 2147483647);

// arithmetic right shift valid for any |x| < 2^53 (JS `>>` is 32-bit only)
const sar = (x, k) => Math.floor(x / 2 ** k);
const idiv = (a, b) => Math.floor(a / b); // non-negative operands only

function isqrt(n) {
  if (n === 0) return 0;
  let x = n, y = sar(x + 1, 1);
  while (y < x) { x = y; y = sar(x + idiv(n, x), 1); }
  return x;
}

function expFrac(f) {
  let term = 1024, sum = 1024;
  for (let k = 1; k <= 6; k++) {
    term = idiv(sar(term * f, 10), k);
    sum = k & 1 ? sum - term : sum + term;
  }
  return sum < 0 ? 0 : sum;
}

function expQ(x, e1) {
  const a = -x, n = sar(a, 10), f = a % 1024;
  if (n >= 16) return 0;
  let r = expFrac(f);
  for (let i = 0; i < n; i++) r = sar(r * e1, 10);
  return r;
}

function matmul(x, w, y, din, dout) {
  for (let t = 0; t < T; t++)
    for (let o = 0; o < dout; o++) {
      let s = 0;
      for (let i = 0; i < din; i++) s += x[t * din + i] * w[i * dout + o];
      y[t * dout + o] = sar(s, 10);
    }
}

function rmsNorm(h) {
  for (let t = 0; t < T; t++) {
    let ss = 0;
    for (let d = 0; d < D; d++) ss += h[t * D + d] * h[t * D + d];
    const rms = isqrt(idiv(ss, D)), inv = idiv(1 << 20, rms + 1);
    for (let d = 0; d < D; d++) h[t * D + d] = sar(h[t * D + d] * inv, 10);
  }
}

const fillW = (n) => Float64Array.from({ length: n }, () => (nextRand() % 257) - 128);

function block(ly, h, e1) {
  const [wq, wk, wv, wo, w1, w2] = ly;
  const q = new Float64Array(T * D), k = new Float64Array(T * D), v = new Float64Array(T * D);
  const at = new Float64Array(T * D), o = new Float64Array(T * D), f = new Float64Array(T * FF), g = new Float64Array(T * D);
  matmul(h, wq, q, D, D);
  matmul(h, wk, k, D, D);
  matmul(h, wv, v, D, D);
  const sc = new Float64Array(T), pr = new Float64Array(T);
  for (let t = 0; t < T; t++) {
    let mx = -(2 ** 60), sum = 0;
    for (let j = 0; j <= t; j++) {
      let s = 0;
      for (let d = 0; d < D; d++) s += q[t * D + d] * k[j * D + d];
      sc[j] = sar(s, 13);
      if (sc[j] > mx) mx = sc[j];
    }
    for (let j = 0; j <= t; j++) { pr[j] = expQ(sc[j] - mx, e1); sum += pr[j]; }
    for (let j = 0; j <= t; j++) pr[j] = idiv(pr[j] * 1024, sum);
    for (let d = 0; d < D; d++) {
      let s = 0;
      for (let j = 0; j <= t; j++) s += pr[j] * v[j * D + d];
      at[t * D + d] = sar(s, 10);
    }
  }
  matmul(at, wo, o, D, D);
  for (let i = 0; i < T * D; i++) h[i] += o[i];
  rmsNorm(h);
  matmul(h, w1, f, D, FF);
  for (let i = 0; i < T * FF; i++) if (f[i] < 0) f[i] = 0;
  matmul(f, w2, g, FF, D);
  for (let i = 0; i < T * D; i++) h[i] += g[i];
  rmsNorm(h);
}

function normalize(raw) {
  let ss = 0;
  for (let d = 0; d < D; d++) ss += raw[d] * raw[d];
  const nrm = isqrt(ss), inv = idiv(1 << 20, nrm + 1);
  return Float64Array.from(raw, (v) => sar(v * inv, 10));
}

const e1 = expFrac(1024);
const layers = Array.from({ length: LAYERS }, () => [fillW(D * D), fillW(D * D), fillW(D * D), fillW(D * D), fillW(D * FF), fillW(FF * D)]);
const db = Array.from({ length: NV }, () => normalize(Float64Array.from({ length: D }, () => (nextRand() % 2049) - 1024)));
let cs = 0;
const fold = (v) => { cs = (cs * 131 + v + BIAS) % MOD; };
for (let s = 0; s < SEQS; s++) {
  const h = Float64Array.from({ length: T * D }, () => (nextRand() % 2049) - 1024);
  for (const ly of layers) block(ly, h, e1);
  const pooled = new Float64Array(D);
  for (let d = 0; d < D; d++) {
    let sum = 0;
    for (let t = 0; t < T; t++) sum += h[t * D + d];
    pooled[d] = sar(sum, 4);
    fold(pooled[d]);
  }
  const query = normalize(pooled);
  const topS = new Float64Array(K), topI = new Float64Array(K);
  let cnt = 0;
  for (let i = 0; i < NV; i++) {
    const row = db[i];
    let sc = 0;
    for (let d = 0; d < D; d++) sc += query[d] * row[d];
    if (cnt < K || sc > topS[K - 1]) {
      let p = cnt < K ? cnt : K - 1;
      while (p > 0 && topS[p - 1] < sc) { topS[p] = topS[p - 1]; topI[p] = topI[p - 1]; p--; }
      topS[p] = sc; topI[p] = i;
      if (cnt < K) cnt++;
    }
  }
  for (let j = 0; j < K; j++) { fold(topI[j]); fold(topS[j]); }
}
console.log(`CHECKSUM=${cs}`);
