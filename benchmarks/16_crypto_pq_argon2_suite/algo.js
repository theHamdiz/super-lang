/* 16_crypto_pq_argon2_suite — Argon2id-like KDF (m=8KiB t=2 p=1) x32, SHA-256 Merkle of tags,
   toy lattice negacyclic NTT poly-mul (q=12289, n=256) */
import { createHash } from "node:crypto";

const MOD = 1000000007;
const NH = 32, BW = 64, NB = 32, PASSES = 2;
const Q = 12289, N = 256, PAIRS = 6;

let rng = 16001;
const nextRand = () => (rng = (rng * 48271) % 2147483647);

const sha256 = (buf) => createHash("sha256").update(buf).digest();
const rot32 = (x, r) => ((x >>> r) | (x << (32 - r))) >>> 0;
const blamka = (x, y) => (x + y + 2 * (x & 0xffff) * (y & 0xffff)) >>> 0;

function gMix(z, ia, ib, ic, id) {
  let a = z[ia], b = z[ib], c = z[ic], d = z[id];
  a = blamka(a, b); d = rot32((d ^ a) >>> 0, 16);
  c = blamka(c, d); b = rot32((b ^ c) >>> 0, 12);
  a = blamka(a, b); d = rot32((d ^ a) >>> 0, 8);
  c = blamka(c, d); b = rot32((b ^ c) >>> 0, 7);
  z[ia] = a; z[ib] = b; z[ic] = c; z[id] = d;
}

function pRound(z, base, stride) {
  const v = [0, 1, 2, 3, 4, 5, 6, 7].map((k) => base + k * stride);
  gMix(z, v[0], v[2], v[4], v[6]);
  gMix(z, v[1], v[3], v[5], v[7]);
  gMix(z, v[0], v[3], v[4], v[7]);
  gMix(z, v[1], v[2], v[5], v[6]);
}

function compress(x, y, out) {
  const r = new Uint32Array(BW);
  for (let i = 0; i < BW; i++) r[i] = x[i] ^ y[i];
  out.set(r);
  for (let row = 0; row < 8; row++) pRound(out, row * 8, 1);
  for (let col = 0; col < 8; col++) pRound(out, col, 8);
  for (let i = 0; i < BW; i++) out[i] ^= r[i];
}

function initBlock(h0, j) {
  const d = sha256(Buffer.concat([h0, Buffer.from([j])]));
  const w = new Uint32Array(BW);
  for (let k = 0; k < 8; k++) w[k] = d.readUInt32BE(4 * k);
  for (let k = 8; k < BW; k++) w[k] = w[k - 8] + (w[k - 3] & 0xffff) * 40503 + (w[k - 1] >>> 16) * 69069;
  return w;
}

function argon2Like(idxPw) {
  const msg = Buffer.alloc(36);
  msg[0] = idxPw; msg[1] = (NB * 256) / 1024; msg[2] = PASSES; msg[3] = 1;
  for (let i = 4; i < 36; i++) msg[i] = nextRand() & 255;
  const h0 = sha256(msg);
  const B = Array.from({ length: NB }, () => new Uint32Array(BW));
  B[0] = initBlock(h0, 0);
  B[1] = initBlock(h0, 1);
  const nw = new Uint32Array(BW);
  for (let pass = 0; pass < PASSES; pass++) {
    for (let idx = pass === 0 ? 2 : 0; idx < NB; idx++) {
      const prev = B[(idx + NB - 1) % NB];
      const r = pass === 0 && idx < NB / 2 ? ((((pass * 131 + idx) * 40503 + 12345) * 69069) % 4294967296) : prev[0];
      const r16 = r >>> 16;
      const x = (r16 * r16) >>> 16;
      let ref;
      if (pass === 0) {
        const area = idx - 1;
        ref = area - 1 - ((area * x) >>> 16);
      } else {
        const area = NB - 2;
        ref = (idx + 1 + (area - 1 - ((area * x) >>> 16))) % NB;
      }
      compress(prev, B[ref], nw);
      if (pass === 0) B[idx].set(nw);
      else for (let i = 0; i < BW; i++) B[idx][i] ^= nw[i];
    }
  }
  const bytes = Buffer.alloc(BW * 4);
  for (let i = 0; i < BW; i++) bytes.writeUInt32BE(B[NB - 1][i], 4 * i);
  return sha256(bytes);
}

function powMod(b, e) {
  let r = 1;
  b %= Q;
  while (e > 0) {
    if (e & 1) r = (r * b) % Q;
    b = (b * b) % Q;
    e >>= 1;
  }
  return r;
}

function ntt(a, w) {
  for (let i = 1, j = 0; i < N; i++) {
    let bit = N >> 1;
    for (; j & bit; bit >>= 1) j ^= bit;
    j ^= bit;
    if (i < j) { const t = a[i]; a[i] = a[j]; a[j] = t; }
  }
  for (let len = 2; len <= N; len <<= 1) {
    const wl = powMod(w, N / len), half = len / 2;
    for (let i = 0; i < N; i += len) {
      let wn = 1;
      for (let j = 0; j < half; j++) {
        const u = a[i + j], v = (a[i + j + half] * wn) % Q;
        a[i + j] = (u + v) % Q;
        a[i + j + half] = (u + Q - v) % Q;
        wn = (wn * wl) % Q;
      }
    }
  }
}

function polymul(a0, b0, psi) {
  const psiInv = powMod(psi, Q - 2), omega = (psi * psi) % Q, nInv = powMod(N, Q - 2);
  const a = new Array(N), b = new Array(N), c = new Array(N);
  let p = 1;
  for (let i = 0; i < N; i++) { a[i] = (a0[i] * p) % Q; b[i] = (b0[i] * p) % Q; p = (p * psi) % Q; }
  ntt(a, omega);
  ntt(b, omega);
  for (let i = 0; i < N; i++) c[i] = (a[i] * b[i]) % Q;
  ntt(c, powMod(omega, Q - 2));
  p = nInv;
  for (let i = 0; i < N; i++) { c[i] = (c[i] * p) % Q; p = (p * psiInv) % Q; }
  return c;
}

function schoolbook(a, b) {
  const c = new Array(N).fill(0);
  for (let i = 0; i < N; i++) {
    for (let j = 0; j < N; j++) {
      const pr = (a[i] * b[j]) % Q, k = i + j;
      if (k >= N) c[k - N] = (c[k - N] + Q - pr) % Q;
      else c[k] = (c[k] + pr) % Q;
    }
  }
  return c;
}

let level = [];
for (let i = 0; i < NH; i++) level.push(argon2Like(i));
while (level.length > 1) {
  const next = [];
  for (let i = 0; i < level.length; i += 2) {
    next.push(sha256(Buffer.concat([level[i], level[i + 1 < level.length ? i + 1 : i]])));
  }
  level = next;
}
let cs = 0;
for (const byte of level[0]) cs = (cs * 131 + byte) % MOD;

const psi = powMod(11, (Q - 1) / (2 * N));
let ok = powMod(psi, N) === Q - 1 ? 1 : 0;
for (let pr = 0; pr < PAIRS; pr++) {
  const a = Array.from({ length: N }, () => nextRand() % Q);
  const b = Array.from({ length: N }, () => nextRand() % Q);
  const c = polymul(a, b, psi);
  if (pr < 2) {
    const ref = schoolbook(a, b);
    for (let i = 0; i < N; i++) if (ref[i] !== c[i]) ok = 0;
  }
  for (const v of c) cs = (cs * 131 + v) % MOD;
}
cs = (cs * 131 + ok) % MOD;
console.log(`CHECKSUM=${cs}`);
