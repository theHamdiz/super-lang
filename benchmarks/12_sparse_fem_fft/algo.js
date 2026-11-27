// 12_sparse_fem_fft — 128x128 reaction-Poisson CSR, Jacobi-PCG x20, 2D FFT low-pass bank
const G = 128;
const N = 16384;
const ITERS = 20;

const indptr = new Int32Array(N + 1);
const indices = new Int32Array(5 * N);
const values = new Float64Array(5 * N);
const diag = new Float64Array(N);

function buildSystem() {
  let nz = 0;
  for (let i = 0; i < N; i++) {
    const row = Math.floor(i / G), col = i % G;
    const d = 4.0 + ((i * 37) % 11) * 0.25;
    diag[i] = d;
    if (row > 0) { indices[nz] = i - G; values[nz++] = -1.0; }
    if (col > 0) { indices[nz] = i - 1; values[nz++] = -1.0; }
    indices[nz] = i; values[nz++] = d;
    if (col < G - 1) { indices[nz] = i + 1; values[nz++] = -1.0; }
    if (row < G - 1) { indices[nz] = i + G; values[nz++] = -1.0; }
    indptr[i + 1] = nz;
  }
}

function matvec(x, out) {
  for (let r = 0; r < N; r++) {
    let s = 0.0;
    for (let k = indptr[r]; k < indptr[r + 1]; k++) s += values[k] * x[indices[k]];
    out[r] = s;
  }
}

function dot(a, b) {
  let s = 0.0;
  for (let i = 0; i < N; i++) s += a[i] * b[i];
  return s;
}

function pcg(b, x) {
  const r = new Float64Array(N), z = new Float64Array(N);
  const p = new Float64Array(N), ap = new Float64Array(N);
  for (let i = 0; i < N; i++) { x[i] = 0.0; r[i] = b[i]; z[i] = r[i] / diag[i]; p[i] = z[i]; }
  let rz = dot(r, z);
  for (let it = 0; it < ITERS; it++) {
    matvec(p, ap);
    const alpha = rz / dot(p, ap);
    for (let i = 0; i < N; i++) {
      x[i] += alpha * p[i];
      r[i] -= alpha * ap[i];
      z[i] = r[i] / diag[i];
    }
    const rzNew = dot(r, z);
    const beta = rzNew / rz;
    for (let i = 0; i < N; i++) p[i] = z[i] + beta * p[i];
    rz = rzNew;
  }
  return dot(r, r);
}

const BITREV = new Int32Array(G);
for (let i = 0; i < G; i++) {
  let r = 0, v = i;
  for (let b = 0; b < 7; b++) { r = (r << 1) | (v & 1); v >>= 1; }
  BITREV[i] = r;
}

const tre = new Float64Array(G), tim = new Float64Array(G);
function fft1(re, im, inv) {
  if (inv) for (let i = 0; i < G; i++) im[i] = 0.0 - im[i];
  for (let i = 0; i < G; i++) { const s = BITREV[i]; tre[i] = re[s]; tim[i] = im[s]; }
  for (let span = 2; span <= G; span *= 2) {
    const half = span / 2;
    const ang = 0.0 - 2.0 * Math.PI / span;
    for (let start = 0; start < G; start += span) {
      for (let k = 0; k < half; k++) {
        const wr = Math.cos(ang * k), wi = Math.sin(ang * k);
        const a = start + k, b = a + half;
        const tr = tre[b] * wr - tim[b] * wi;
        const ti = tre[b] * wi + tim[b] * wr;
        tre[b] = tre[a] - tr; tim[b] = tim[a] - ti;
        tre[a] = tre[a] + tr; tim[a] = tim[a] + ti;
      }
    }
  }
  if (inv) {
    const s = 1.0 / G;
    for (let i = 0; i < G; i++) { re[i] = tre[i] * s; im[i] = (0.0 - tim[i]) * s; }
  } else {
    for (let i = 0; i < G; i++) { re[i] = tre[i]; im[i] = tim[i]; }
  }
}

function fft2(re, im, inv) {
  const lr = new Float64Array(G), li = new Float64Array(G);
  for (let r = 0; r < G; r++) {
    for (let c = 0; c < G; c++) { lr[c] = re[r * G + c]; li[c] = im[r * G + c]; }
    fft1(lr, li, inv);
    for (let c = 0; c < G; c++) { re[r * G + c] = lr[c]; im[r * G + c] = li[c]; }
  }
  for (let c = 0; c < G; c++) {
    for (let r = 0; r < G; r++) { lr[r] = re[r * G + c]; li[r] = im[r * G + c]; }
    fft1(lr, li, inv);
    for (let r = 0; r < G; r++) { re[r * G + c] = lr[r]; im[r * G + c] = li[r]; }
  }
}

const b = new Float64Array(N), x = new Float64Array(N);
for (let i = 0; i < N; i++) b[i] = (((i * 7 + Math.floor(i / G) * 13) % 17) - 8) / 8.0;
buildSystem();
const res = Math.sqrt(pcg(b, x));

const sre = new Float64Array(x), sim = new Float64Array(N);
fft2(sre, sim, false);

const cuts = [64, 256, 1024];
const fre = new Float64Array(N), fim = new Float64Array(N);
let energy = 0.0, corner = 0.0;
for (let q = 0; q < 3; q++) {
  fre.set(sre); fim.set(sim);
  for (let k = 0; k < N; k++) {
    const ky = Math.floor(k / G), kx = k % G;
    const fy = ky <= G / 2 ? ky : G - ky;
    const fx = kx <= G / 2 ? kx : G - kx;
    if (fx * fx + fy * fy > cuts[q]) { fre[k] = 0.0; fim[k] = 0.0; }
  }
  fft2(fre, fim, true);
  for (let k = 0; k < N; k++) energy += fre[k] * fre[k];
  corner += fre[0] + fre[N / 2 + G / 2] + fre[N - 1];
}

const xs = dot(x, x);
const cs = Math.floor(Math.sqrt(xs) * 1000000.0) + Math.floor(energy * 1000.0) +
  Math.floor(corner * 1000000.0) + Math.floor(res * 1000000000.0);
console.log(`CHECKSUM=${cs}`);
