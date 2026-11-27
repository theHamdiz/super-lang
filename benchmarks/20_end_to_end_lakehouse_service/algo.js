/* 20_end_to_end_lakehouse_service — in-process pipeline:
   ingest JSONL customers + CSV order batches -> filter/join/group -> fixed-point GD linear fit on the
   per-product aggregates -> serve HTTP-like request bytes (bearer-token check via SHA-256, Merkle root of
   the group table in every response) -> checksum of response bytes. */
import { createHash } from "node:crypto";

const MOD = 1000000007;
const NCUST = 500, NPROD = 50, NREG = 8, NTIER = 4, NGRP = NREG * NTIER;
const BATCHES = 6, ROWS_PB = 4000, NREQ = 3000, NCLIENT = 8, ITERS = 400;

let rng = 20020;
const nextRand = () => (rng = (rng * 48271) % 2147483647);
const sdiv = (a, d) => (a >= 0 ? Math.floor(a / d) : -Math.floor(-a / d));
const sha256 = (buf) => createHash("sha256").update(buf).digest();

const CRC = new Uint32Array(256);
for (let i = 0; i < 256; i++) {
  let c = i;
  for (let k = 0; k < 8; k++) c = c & 1 ? (c >>> 1) ^ 0xedb88320 : c >>> 1;
  CRC[i] = c >>> 0;
}
function crc32(d) {
  let c = 0xffffffff;
  for (let i = 0; i < d.length; i++) c = (CRC[(c ^ d[i]) & 255] ^ (c >>> 8)) >>> 0;
  return (c ^ 0xffffffff) >>> 0;
}

const enc = new TextEncoder();
const bytes = (s) => enc.encode(s);
function concat(...parts) {
  let n = 0;
  for (const p of parts) n += p.length;
  const out = new Uint8Array(n);
  let o = 0;
  for (const p of parts) { out.set(p, o); o += p.length; }
  return out;
}
const hex = (d) => Buffer.from(d).toString("hex");

/* Parse the decimal run at cur.p (advancing it); -1 when there are no digits. */
function scanInt(b, cur) {
  let v = 0, have = false;
  while (b[cur.p] >= 48 && b[cur.p] <= 57) { v = v * 10 + b[cur.p] - 48; cur.p++; have = true; }
  return have ? v : -1;
}
function indexOf(b, blen, pat, from) {
  for (let i = from; i + pat.length <= blen; i++) {
    let ok = true;
    for (let j = 0; j < pat.length; j++) if (b[i + j] !== pat[j]) { ok = false; break; }
    if (ok) return i;
  }
  return -1;
}

let cs = 0;
const fold = (v) => { cs = (cs * 131 + v) % MOD; };

/* ---- ingest customers (JSONL) ---- */
let cj = "";
for (let i = 0; i < NCUST; i++) {
  const id = (i * 7919) % NCUST, r = nextRand() % NREG, t = nextRand() % NTIER;
  cj += `{"id":${id},"region":${r},"tier":${t}}\n`;
}
const cb = bytes(cj + "\0");
const clen = cb.length - 1;
const regionOf = new Int32Array(NCUST).fill(-1), tierOf = new Int32Array(NCUST).fill(-1);
let ncust = 0;
const cur = { p: 0 };
const isDigit = (c) => c >= 48 && c <= 57;
while (cur.p < clen) {
  while (cur.p < clen && !isDigit(cb[cur.p])) cur.p++;
  const id = scanInt(cb, cur);
  while (!isDigit(cb[cur.p])) cur.p++;
  const r = scanInt(cb, cur);
  while (!isDigit(cb[cur.p])) cur.p++;
  const t = scanInt(cb, cur);
  regionOf[id] = r; tierOf[id] = t; ncust++;
  while (cur.p < clen && cb[cur.p] !== 10) cur.p++;
  cur.p++;
}

/* ---- ingest + filter/join/group order batches (CSV) ---- */
const grpCnt = new Array(NGRP).fill(0), grpQty = new Array(NGRP).fill(0), grpRev = new Array(NGRP).fill(0);
const prodCnt = new Array(NPROD).fill(0), prodQty = new Array(NPROD).fill(0);
const prodRev = new Array(NPROD).fill(0), prodPrice = new Array(NPROD).fill(0);
const regProdRev = new Array(NREG * NPROD).fill(0);
let oid = 0, rows = 0, nullCust = 0, badQty = 0, valid = 0;
for (let b = 0; b < BATCHES; b++) {
  const lines = ["order_id,cust_id,product,qty,price_cents\n"];
  for (let i = 0; i < ROWS_PB; i++) {
    const nf = nextRand() % 100, c = nextRand() % NCUST, p = nextRand() % NPROD, q = nextRand() % 9;
    const pr = 100 + p * 37 + (nextRand() % 200);
    lines.push(`${oid++},${nf !== 0 ? c : ""},${p},${q},${pr}\n`);
  }
  const ob = bytes(lines.join("") + "\0");
  const olen = ob.length - 1;
  const oc = { p: 0 };
  while (ob[oc.p] !== 10) oc.p++;
  oc.p++;
  const f = [0, 0, 0, 0, 0];
  while (oc.p < olen) {
    for (let k = 0; k < 5; k++) { f[k] = scanInt(ob, oc); oc.p++; }
    rows++;
    if (f[1] < 0) { nullCust++; continue; }
    if (f[3] <= 0) { badQty++; continue; }
    valid++;
    const rev = f[3] * f[4], g = regionOf[f[1]] * NTIER + tierOf[f[1]];
    grpCnt[g]++; grpQty[g] += f[3]; grpRev[g] += rev;
    prodCnt[f[2]]++; prodQty[f[2]] += f[3]; prodRev[f[2]] += rev; prodPrice[f[2]] += f[4];
    regProdRev[regionOf[f[1]] * NPROD + f[2]] += rev;
  }
}

/* ---- fit y = w*x + b (w in Q16) by gradient descent on per-product aggregates ---- */
const xs = [], ys = [];
for (let p = 0; p < NPROD; p++) {
  if (prodCnt[p] > 0) {
    xs.push(Math.floor(Math.floor(prodPrice[p] / prodCnt[p]) / 16));
    ys.push(Math.floor(Math.floor(prodRev[p] / prodCnt[p]) / 16));
  }
}
const n = xs.length;
let w = 0, bias = 0;
for (let it = 0; it < ITERS; it++) {
  let gw = 0, gb = 0;
  for (let i = 0; i < n; i++) {
    const err = sdiv(w * xs[i], 65536) + bias - ys[i];
    gw += err * xs[i];
    gb += err;
  }
  w -= sdiv(gw * 65536, n * 8000);
  bias -= sdiv(gb, n * 4);
}
let loss = 0;
for (let i = 0; i < n; i++) {
  const err = sdiv(w * xs[i], 65536) + bias - ys[i];
  loss += err * err;
}
const predCents = [];
for (let p = 0; p < NPROD; p++) {
  const x = prodCnt[p] > 0 ? Math.floor(Math.floor(prodPrice[p] / prodCnt[p]) / 16) : 0;
  const y = sdiv(w * x, 65536) + bias;
  predCents.push((y > 0 ? y : 0) * 16);
}

/* ---- merkle root over the region x tier group table ---- */
let level = [];
for (let g = 0; g < NGRP; g++) {
  const leaf = Buffer.alloc(32);
  [g, grpCnt[g], grpQty[g], grpRev[g]].forEach((v, k) => leaf.writeBigUInt64BE(BigInt(v), k * 8));
  level.push(sha256(leaf));
}
while (level.length > 1) {
  const next = [];
  for (let i = 0; i < level.length / 2; i++) next.push(sha256(Buffer.concat([level[2 * i], level[2 * i + 1]])));
  level = next;
}
const root = level[0];
const rootHex = hex(root);

/* ---- client tokens ---- */
const tokens = [];
for (let k = 0; k < NCLIENT; k++) tokens.push(bytes(hex(sha256(Buffer.from(`key-${k}`)).subarray(0, 8))));

/* ---- serve ---- */
let ok200 = 0, un401 = 0, nf404 = 0, totalBytes = 0, respCs = 0;
const P_GET = bytes("GET /v1/product/"), P_XC = bytes("X-Client: "), P_BEARER = bytes("Bearer ");
for (let i = 0; i < NREQ; i++) {
  const k = nextRand() % NCLIENT, p = nextRand() % 60, rg = nextRand() % NREG;
  const bad = nextRand() % 100, at = nextRand() % 16;
  const tok = Uint8Array.from(tokens[k]);
  if (bad < 8) tok[at] = 122;
  const req = concat(
    bytes(`GET /v1/product/${p}?region=${rg} HTTP/1.1\r\nHost: lake\r\nX-Client: ${k}\r\nAuthorization: Bearer `),
    tok, bytes("\r\n\r\n"));
  const rl = req.length;
  /* server side: parse request bytes */
  let status = 400, pp = -1, rr = -1;
  if (rl > 16 && indexOf(req, 16, P_GET, 0) === 0) {
    const q = { p: 16 };
    pp = scanInt(req, q);
    q.p += 8;
    rr = scanInt(req, q);
    const xc = indexOf(req, rl, P_XC, q.p), bt = indexOf(req, rl, P_BEARER, q.p);
    if (xc >= 0 && bt >= 0) {
      const a = { p: xc + 10 };
      const kk = scanInt(req, a);
      let good = kk >= 0 && kk < NCLIENT && bt + 7 + 16 <= rl;
      if (good) for (let j = 0; j < 16; j++) if (req[bt + 7 + j] !== tokens[kk][j]) good = false;
      if (good) status = pp >= 0 && pp < NPROD && rr >= 0 && rr < NREG && prodCnt[pp] > 0 ? 200 : 404;
      else status = 401;
    }
  }
  let body, head;
  if (status === 200) {
    body = `{"product":${pp},"region":${rr},"orders":${prodCnt[pp]},"revenue":${prodRev[pp]},` +
      `"region_revenue":${regProdRev[rr * NPROD + pp]},"forecast":${predCents[pp]},"root":"${rootHex}"}`;
    ok200++; head = "HTTP/1.1 200 OK";
  } else if (status === 404) { body = '{"error":"not found"}'; nf404++; head = "HTTP/1.1 404 Not Found"; }
  else if (status === 401) { body = '{"error":"unauthorized"}'; un401++; head = "HTTP/1.1 401 Unauthorized"; }
  else { body = '{"error":"bad request"}'; head = "HTTP/1.1 400 Bad Request"; }
  const resp = bytes(`${head}\r\nContent-Length: ${body.length}\r\n\r\n${body}`);
  totalBytes += resp.length;
  respCs = (respCs * 131 + crc32(resp)) % MOD;
}

/* ---- checksum ---- */
for (const v of [ncust, rows, nullCust, badQty, valid]) fold(v);
for (let g = 0; g < NGRP; g++) { fold(grpCnt[g]); fold(grpQty[g]); fold(grpRev[g]); }
for (let p = 0; p < NPROD; p++) { fold(prodCnt[p]); fold(prodRev[p]); }
for (const v of [w, bias, loss]) fold(v);
for (const b of root) fold(b);
for (const v of [ok200, un401, nf404, totalBytes, respCs]) fold(v);
console.log(`CHECKSUM=${cs}`);
