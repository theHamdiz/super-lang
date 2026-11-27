/* 14_http_router_throughput - 30k simulated HTTP requests: parse bytes, radix route match,
   HMAC-SHA256 bearer-token check, JSON body validation, status-code checksum */
import { createHmac } from "node:crypto";

const NREQ = 30000;
const NOW = 1700000000;
const KEY = Buffer.alloc(32);
for (let i = 0; i < 32; i++) KEY[i] = (i * 37 + 11) % 256;
const HEX = Buffer.from("0123456789abcdef");

let rng = 140014;
function draw(m) {
  rng = (Math.imul(rng, 1103515245) + 12345) & 0x7fffffff;
  return Math.floor(rng / 256) % m;
}

const macOf = (msg) => createHmac("sha256", KEY).update(msg).digest();
const B = (s) => Buffer.from(s, "latin1");
const dec = (v) => String(v);

function putId() {
  const bad = draw(50);
  const idv = 1 + draw(99999);
  return bad === 0 ? "abc" : dec(idv);
}

const METHODS = ["GET ", "POST ", "PUT ", "DELETE "];

function buildRequest() {
  let path = "";
  let method = 0;
  let hasBody = false;
  const a = draw(100);
  if (a < 5) path = "/health";
  else if (a < 15) path = "/users";
  else if (a < 35) path = "/users/" + putId();
  else if (a < 45) path = "/users/" + putId() + "/orders";
  else if (a < 55) { method = 1; hasBody = true; path = "/orders"; }
  else if (a < 65) path = "/orders/" + putId();
  else if (a < 70) { method = 2; hasBody = true; path = "/orders/" + putId(); }
  else if (a < 75) { method = 3; path = "/orders/" + putId(); }
  else if (a < 85) {
    path = "/static/a" + dec(draw(10));
    path += "/f" + dec(draw(100)) + ".js";
  } else if (a < 90) { method = 1; hasBody = true; path = "/auth/login"; }
  else if (a < 95) {
    path = "/v1/items/" + putId();
    path += "/tags/t" + dec(draw(50));
  } else if (a < 98) path = "/nope/" + putId();
  else { method = 3; path = "/users"; }

  const t = draw(20);
  const uid = 1 + draw(9999);
  const ex = draw(1000);
  const hasTok = t >= 2;
  let tok = "";
  if (hasTok) {
    const exp = t === 4 ? NOW - 1 - ex : NOW + 1000 + ex;
    const sb = dec(uid) + "." + dec(exp);
    const mac = macOf(B(sb));
    if (t < 4) mac[0] ^= 1;
    tok = sb + ".";
    for (let i = 0; i < 4; i++) tok += String.fromCharCode(HEX[mac[i] >> 4], HEX[mac[i] & 15]);
  }

  let body = "";
  if (hasBody) {
    const v = draw(10);
    const ws = draw(2);
    const sp = ws ? " " : "";
    if (a >= 45 && a < 55) {
      let user = 1 + draw(9999);
      let qty = 1 + draw(100);
      if (v === 0) qty = 0;
      if (v === 3) user = 10000;
      body = '{"user":' + sp + dec(user) + "," + sp + '"qty":' + sp + dec(qty);
      let sku = "";
      for (let i = 0; i < 4; i++) sku += String.fromCharCode(97 + draw(26));
      if (v !== 1) body += "," + sp + '"sku":' + sp + '"' + sku + '"';
      body += "}";
      if (v === 2) body = body.slice(0, -1);
    } else if (a >= 65 && a < 70) {
      let qty = 1 + draw(100);
      const nl = 3 + draw(5);
      if (v === 0) qty = 101;
      body = "{";
      if (v !== 1) body += '"qty":' + sp + dec(qty) + "," + sp;
      body += '"note":' + sp + '"';
      for (let i = 0; i < nl; i++) body += String.fromCharCode(97 + draw(26));
      body += '"}';
      if (v === 2) body = body.slice(0, -1);
    } else {
      const ul = 4 + draw(6);
      let un = "";
      for (let i = 0; i < ul; i++) un += String.fromCharCode(97 + draw(26));
      const pwl = 6 + draw(7);
      let pw = "";
      for (let i = 0; i < pwl; i++) pw += String.fromCharCode(97 + draw(26));
      if (v === 0) pw = "";
      body = "{";
      if (v !== 1) body += '"user":' + sp + '"' + un + '",' + sp;
      body += '"pass":' + sp + '"' + pw + '"}';
      if (v === 2) body += "x";
    }
  }

  let r = METHODS[method] + path + " HTTP/1.1\r\nHost: api.local\r\n";
  if (hasTok) r += "Authorization: Bearer " + tok + "\r\n";
  if (hasBody) r += "Content-Length: " + dec(body.length) + "\r\n";
  r += "\r\n" + body;
  return B(r);
}

// ---- radix router: node = { kind, lit, first, next, h:[4] } ----
const nodes = [{ kind: 0, lit: "", first: -1, next: -1, h: [-1, -1, -1, -1] }];

function addRoute(tpl, method, rid) {
  let cur = 0;
  for (const seg of tpl.slice(1).split("/")) {
    const kind = seg[0] === ":" ? 1 : seg[0] === "*" ? 2 : 0;
    const lit = kind === 0 ? seg : "";
    let found = -1;
    for (let c = nodes[cur].first; c >= 0 && found < 0; c = nodes[c].next) {
      const nd = nodes[c];
      if (nd.kind === kind && (kind !== 0 || nd.lit === lit)) found = c;
    }
    if (found < 0) {
      nodes.push({ kind, lit, first: -1, next: nodes[cur].first, h: [-1, -1, -1, -1] });
      found = nodes.length - 1;
      nodes[cur].first = found;
    }
    cur = found;
  }
  nodes[cur].h[method] = rid;
}

const AUTH = [false, true, true, true, true, true, true, true, false, false, true];

function parseDigits(b, p, e) {
  if (e <= p || e - p > 10) return -1;
  let v = 0;
  for (let i = p; i < e; i++) {
    const c = b[i];
    if (c < 48 || c > 57) return -1;
    v = v * 10 + (c - 48);
  }
  return v;
}

function startsAt(b, p, lit) {
  if (p + lit.length > b.length) return false;
  for (let i = 0; i < lit.length; i++) if (b[p + i] !== lit.charCodeAt(i)) return false;
  return true;
}

function verifyToken(b, off, ln) {
  const e = off + ln;
  let d1 = -1, d2 = -1;
  for (let i = off; i < e; i++) {
    if (b[i] === 46) {
      if (d1 < 0) d1 = i;
      else if (d2 < 0) d2 = i;
      else return -1;
    }
  }
  if (d1 < 0 || d2 < 0 || e - d2 - 1 !== 8) return -1;
  const u = parseDigits(b, off, d1);
  const ex = parseDigits(b, d1 + 1, d2);
  if (u < 0 || ex < 0) return -1;
  const mac = macOf(b.subarray(off, d2));
  for (let i = 0; i < 4; i++) {
    if (b[d2 + 1 + 2 * i] !== HEX[mac[i] >> 4] || b[d2 + 2 + 2 * i] !== HEX[mac[i] & 15]) return -1;
  }
  if (ex <= NOW) return -1;
  return u;
}

const isWs = (c) => c === 32 || c === 9 || c === 10 || c === 13;
function skipWs(b, p, e) {
  while (p < e && isWs(b[p])) p++;
  return p;
}

const KEYS = { user: 1, qty: 2, sku: 3, note: 4, pass: 5 };

// returns field table (id -> [type, val]) or null when malformed
function parseJson(b, off, ln) {
  const e = off + ln;
  const f = new Array(6).fill(null);
  let p = skipWs(b, off, e);
  if (p >= e || b[p] !== 123) return null;
  p = skipWs(b, p + 1, e);
  if (p < e && b[p] === 125) {
    p = skipWs(b, p + 1, e);
    return p === e ? f : null;
  }
  for (;;) {
    if (p >= e || b[p] !== 34) return null;
    p++;
    const ks = p;
    while (p < e && b[p] !== 34) {
      if (b[p] === 92) return null;
      p++;
    }
    if (p >= e) return null;
    const kid = KEYS[b.latin1Slice(ks, p)] ?? 0;
    p = skipWs(b, p + 1, e);
    if (p >= e || b[p] !== 58) return null;
    p = skipWs(b, p + 1, e);
    if (p >= e) return null;
    let ty, val;
    if (b[p] === 34) {
      p++;
      const vs = p;
      while (p < e && b[p] !== 34) {
        if (b[p] === 92) return null;
        p++;
      }
      if (p >= e) return null;
      ty = 2; val = p - vs;
      p++;
    } else {
      let neg = false;
      if (b[p] === 45) { neg = true; p++; }
      const ds = p;
      while (p < e && b[p] >= 48 && b[p] <= 57) p++;
      const v = parseDigits(b, ds, p);
      if (v < 0) return null;
      ty = 1; val = neg ? -v : v;
    }
    if (kid > 0) f[kid] = [ty, val];
    p = skipWs(b, p, e);
    if (p >= e) return null;
    if (b[p] === 44) { p = skipWs(b, p + 1, e); continue; }
    if (b[p] === 125) { p++; break; }
    return null;
  }
  p = skipWs(b, p, e);
  return p === e ? f : null;
}

const hasInt = (f, i, lo, hi) => f[i] !== null && f[i][0] === 1 && f[i][1] >= lo && f[i][1] <= hi;
const hasStr = (f, i, lo, hi) => f[i] !== null && f[i][0] === 2 && f[i][1] >= lo && f[i][1] <= hi;

function bodyValid(rid, b, off, ln) {
  const f = parseJson(b, off, ln);
  if (f === null) return false;
  if (rid === 4) return hasInt(f, 1, 1, 9999) && hasInt(f, 2, 1, 100) && hasStr(f, 3, 4, 4);
  if (rid === 6) return hasInt(f, 2, 1, 100) && (f[4] === null || hasStr(f, 4, 0, 1000));
  return hasStr(f, 1, 1, 64) && hasStr(f, 5, 1, 64);
}

let pidOut = 0;
function handle(b) {
  pidOut = 0;
  const n = b.length;
  let method, p;
  if (startsAt(b, 0, "GET ")) { method = 0; p = 4; }
  else if (startsAt(b, 0, "POST ")) { method = 1; p = 5; }
  else if (startsAt(b, 0, "PUT ")) { method = 2; p = 4; }
  else if (startsAt(b, 0, "DELETE ")) { method = 3; p = 7; }
  else return 400;
  const po = p;
  while (p < n && b[p] !== 32) p++;
  const pe = p;
  while (p < n && b[p] !== 13) p++;
  p += 2;

  let tokOff = -1, tokLen = 0, clen = -1;
  while (p < n) {
    if (b[p] === 13) { p += 2; break; }
    let le = p;
    while (le < n && b[le] !== 13) le++;
    if (startsAt(b, p, "Authorization: Bearer ")) { tokOff = p + 22; tokLen = le - (p + 22); }
    else if (startsAt(b, p, "Content-Length: ")) clen = parseDigits(b, p + 16, le);
    p = le + 2;
  }
  const bodyOff = p, bodyLen = n - p;
  if (clen >= 0 && clen !== bodyLen) return 400;

  const plen = pe - po;
  let cur = 0, found = true, nparam = 0, badId = false, pid = -1;
  if (plen < 1 || b[po] !== 47) found = false;
  let i = 1;
  while (found && i <= plen) {
    let e = i;
    while (e < plen && b[po + e] !== 47) e++;
    const sl = e - i;
    if (sl === 0) { found = false; break; }
    const seg = b.latin1Slice(po + i, po + e);
    let st = -1, pm = -1, wc = -1;
    for (let c = nodes[cur].first; c >= 0; c = nodes[c].next) {
      const nd = nodes[c];
      if (nd.kind === 0) {
        if (nd.lit === seg) { st = c; break; }
      } else if (nd.kind === 1) pm = c;
      else wc = c;
    }
    if (st >= 0) cur = st;
    else if (pm >= 0) {
      cur = pm;
      if (nparam === 0) {
        pid = parseDigits(b, po + i, po + e);
        if (pid < 0) badId = true;
      }
      nparam++;
    } else if (wc >= 0) { cur = wc; break; }
    else { found = false; break; }
    i = e + 1;
  }
  if (!found) return 404;
  const hs = nodes[cur].h;
  const rid = hs[method];
  if (rid < 0) return Math.max(hs[0], hs[1], hs[2], hs[3]) >= 0 ? 405 : 404;
  pidOut = badId || pid < 0 ? 0 : pid;

  if (AUTH[rid]) {
    if (tokOff < 0) return 401;
    const uid = verifyToken(b, tokOff, tokLen);
    if (uid < 0) return 401;
    if ((rid === 6 || rid === 7) && uid % 13 === 0) return 403;
  }
  if (badId) return 400;
  if ((rid === 4 || rid === 6 || rid === 9) && !bodyValid(rid, b, bodyOff, bodyLen)) return 400;
  if (rid === 2) return pid > 90000 ? 404 : 200;
  if (rid === 4) return 201;
  if (rid === 5) return pid % 7 === 0 ? 404 : 200;
  if (rid === 7) return 204;
  return 200;
}

addRoute("/health", 0, 0);
addRoute("/users", 0, 1);
addRoute("/users/:id", 0, 2);
addRoute("/users/:id/orders", 0, 3);
addRoute("/orders", 1, 4);
addRoute("/orders/:id", 0, 5);
addRoute("/orders/:id", 2, 6);
addRoute("/orders/:id", 3, 7);
addRoute("/static/*path", 0, 8);
addRoute("/auth/login", 1, 9);
addRoute("/v1/items/:id/tags/:tag", 0, 10);

let cs = 0, sumPid = 0, ok2xx = 0;
const byStatus = new Array(600).fill(0);
for (let i = 0; i < NREQ; i++) {
  const req = buildRequest();
  const st = handle(req);
  byStatus[st]++;
  cs += st * (i % 97 + 1);
  sumPid += pidOut;
  if (st < 300) ok2xx++;
}
let mix = 0;
for (let s = 0; s < 600; s++) mix += byStatus[s] * (s % 53 + 1) * (Math.floor(s / 100) + 1);
cs += sumPid + 5 * ok2xx + 7 * mix;
console.log(`CHECKSUM=${cs}`);
