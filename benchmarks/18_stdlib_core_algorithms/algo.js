/* 18_stdlib_core_algorithms — one checksum from four std-style kernels:
   (A) KMP-DFA "regex" field extraction over 3000 generated log lines,
   (B) strict UTF-8 validation/counting over 60 documents (some corrupted),
   (C) SQL-ish filter / group-by / order-by-limit / join over 6000 rows x 24 queries,
   (D) HM-style type unification (union-find, iterative) over 8504 type nodes. */
const MOD = 1000000007;
const NL = 3000, NDOC = 60, PERDOC = 200, NR = 6000, NQ = 24;
const NVARS = 2500, NCOMP = 6000, NCON = 5000;
const NTOT = NVARS + 4 + NCOMP;

let rng = 18018;
const nextRand = () => (rng = (rng * 48271) % 2147483647);
let cs = 0;
const fold = (v) => { cs = (cs * 131 + v) % MOD; };
const enc = new TextEncoder();

// ---------- (A) log extraction ----------
function buildDfa(needle) {
  const n = needle.length, dfa = new Int32Array(n * 256);
  dfa[needle[0]] = 1;
  let x = 0;
  for (let j = 1; j < n; j++) {
    dfa.copyWithin(j * 256, x * 256, x * 256 + 256);
    dfa[j * 256 + needle[j]] = j + 1;
    x = dfa[x * 256 + needle[j]];
  }
  return dfa;
}

function extract(line, e, dfa, n) {
  let st = 0, i = 0;
  while (i < e) {
    st = dfa[st * 256 + line[i]];
    i++;
    if (st === n) {
      let v = 0, nd = 0;
      while (i < e && line[i] >= 48 && line[i] <= 57) { v = v * 10 + line[i] - 48; nd++; i++; }
      if (nd > 0) return v;
      st = 0;
    }
  }
  return -1;
}

const pad2 = (v) => String(v).padStart(2, "0");

function partA() {
  const levels = ["INFO", "WARN", "ERROR", "DEBUG"];
  const codes = [200, 201, 204, 301, 400, 401, 404, 500, 503];
  const dStatus = buildDfa(enc.encode("status=")), dTook = buildDfa(enc.encode("took=")), dUser = buildDfa(enc.encode("user="));
  const cls = new Array(8).fill(0);
  let tookSum = 0, tookMax = 0, tookN = 0, userSum = 0, bad = 0, total = 0;
  for (let i = 0; i < NL; i++) {
    const mo = 1 + (nextRand() % 12), dd = 1 + (nextRand() % 28), hh = nextRand() % 24;
    const mi = nextRand() % 60, ss = nextRand() % 60, lv = nextRand() % 4;
    const svc = nextRand() % 100, uid = nextRand() % 100000, sidx = nextRand() % 9, ms = nextRand() % 3000;
    let s = `2026-${pad2(mo)}-${pad2(dd)} ${pad2(hh)}:${pad2(mi)}:${pad2(ss)} ${levels[lv]} svc-${pad2(svc)} user=${uid}`;
    s += i % 13 === 5 ? " status=abc" : ` status=${codes[sidx]}`;
    if (i % 17 !== 3) s += ` took=${ms}ms`;
    const line = enc.encode(s), e = line.length;
    const st = extract(line, e, dStatus, 7), tk = extract(line, e, dTook, 5), us = extract(line, e, dUser, 5);
    if (st < 0) bad++; else cls[Math.floor(st / 100)]++;
    if (tk >= 0) { tookSum += tk; tookN++; if (tk > tookMax) tookMax = tk; }
    userSum += us;
    total += e + 1;
  }
  for (const c of cls) fold(c);
  for (const v of [tookSum, tookMax, tookN, userSum, bad, total]) fold(v);
}

// ---------- (B) UTF-8 ----------
function utf8Valid(d, n) {
  let i = 0;
  while (i < n) {
    const b0 = d[i];
    if (b0 < 128) { i++; continue; }
    if (b0 < 194) return false;
    if (b0 < 224) {
      if (i + 1 >= n || (d[i + 1] & 0xc0) !== 0x80) return false;
      i += 2;
    } else if (b0 < 240) {
      if (i + 2 >= n || (d[i + 1] & 0xc0) !== 0x80 || (d[i + 2] & 0xc0) !== 0x80) return false;
      const cp = (b0 - 224) * 4096 + (d[i + 1] - 128) * 64 + (d[i + 2] - 128);
      if (cp < 2048 || (cp >= 55296 && cp <= 57343)) return false;
      i += 3;
    } else if (b0 < 245) {
      if (i + 3 >= n || (d[i + 1] & 0xc0) !== 0x80 || (d[i + 2] & 0xc0) !== 0x80 || (d[i + 3] & 0xc0) !== 0x80) return false;
      const cp = (b0 - 240) * 262144 + (d[i + 1] - 128) * 4096 + (d[i + 2] - 128) * 64 + (d[i + 3] - 128);
      if (cp < 65536 || cp > 1114111) return false;
      i += 4;
    } else return false;
  }
  return true;
}

function utf8Count(d, n) {
  let c = 0;
  for (let i = 0; i < n; i++) if ((d[i] & 0xc0) !== 0x80) c++;
  return c;
}

function encode(cp, o, at) {
  if (cp < 128) { o[at] = cp; return 1; }
  if (cp < 2048) { o[at] = 192 + Math.floor(cp / 64); o[at + 1] = 128 + (cp % 64); return 2; }
  if (cp < 65536) {
    o[at] = 224 + Math.floor(cp / 4096); o[at + 1] = 128 + (Math.floor(cp / 64) % 64); o[at + 2] = 128 + (cp % 64);
    return 3;
  }
  o[at] = 240 + Math.floor(cp / 262144); o[at + 1] = 128 + (Math.floor(cp / 4096) % 64);
  o[at + 2] = 128 + (Math.floor(cp / 64) % 64); o[at + 3] = 128 + (cp % 64);
  return 4;
}

function partB() {
  const badb = [0x80, 0xc0, 0xc1, 0xf5, 0xff, 0xe0];
  const doc = new Uint8Array(PERDOC * 4);
  let validDocs = 0, totalCp = 0;
  for (let d = 0; d < NDOC; d++) {
    let n = 0;
    for (let k = 0; k < PERDOC; k++) {
      const cat = nextRand() % 4, r = nextRand();
      const cp = cat === 0 ? r % 128 : cat === 1 ? 128 + (r % 1920) : cat === 2 ? 2048 + (r % 53248) : 65536 + (r % 1048576);
      n += encode(cp, doc, n);
    }
    if (d % 5 === 2) {
      const p = nextRand() % n, b = nextRand() % 6;
      doc[p] = badb[b];
    }
    const ok = utf8Valid(doc, n) ? 1 : 0, cnt = ok ? utf8Count(doc, n) : 0;
    fold(ok); fold(cnt); fold(n);
    validDocs += ok;
    totalCp += cnt;
  }
  fold(validDocs); fold(totalCp);
}

// ---------- (C) SQL-ish ----------
function partC() {
  const dept = new Int32Array(NR), age = new Int32Array(NR), sal = new Int32Array(NR), act = new Int32Array(NR);
  for (let i = 0; i < NR; i++) {
    dept[i] = nextRand() % 16;
    age[i] = 18 + (nextRand() % 50);
    sal[i] = 30000 + (nextRand() % 90000);
    act[i] = nextRand() % 10 !== 0 ? 1 : 0;
  }
  for (let q = 0; q < NQ; q++) {
    const ageLo = 18 + (nextRand() % 30), salMax = 40000 + (nextRand() % 80000), need = nextRand() % 2;
    const cnt = new Array(16).fill(0), sum = new Array(16).fill(0), mx = new Array(16).fill(0), taken = new Array(16).fill(0);
    for (let i = 0; i < NR; i++) {
      if (age[i] >= ageLo && sal[i] < salMax && act[i] >= need) {
        const g = dept[i];
        cnt[g]++;
        sum[g] += sal[i];
        if (age[i] > mx[g]) mx[g] = age[i];
      }
    }
    for (let pick = 0; pick < 3; pick++) {
      let best = -1;
      for (let g = 0; g < 16; g++) if (cnt[g] >= 5 && !taken[g] && (best < 0 || sum[g] > sum[best])) best = g;
      if (best >= 0) {
        taken[best] = 1;
        fold(best); fold(cnt[best]); fold(sum[best]); fold(mx[best]);
        fold(Math.floor(sum[best] / ((best + 1) * 1000)));
      } else fold(999);
    }
  }
}

// ---------- (D) HM unification ----------
function partD() {
  const kind = new Int32Array(NTOT), na = new Int32Array(NTOT).fill(-1), nb = new Int32Array(NTOT).fill(-1);
  const parent = Int32Array.from({ length: NTOT }, (_, i) => i);
  for (let c = 0; c < 4; c++) { kind[NVARS + c] = 1; na[NVARS + c] = c; }
  for (let i = NVARS + 4; i < NTOT; i++) { kind[i] = 2; na[i] = nextRand() % i; nb[i] = nextRand() % i; }
  let unions = 0;
  const find = (x) => {
    while (parent[x] !== x) { parent[x] = parent[parent[x]]; x = parent[x]; }
    return x;
  };
  const stkA = new Int32Array(2 * NTOT + 16), stkB = new Int32Array(2 * NTOT + 16);
  const unify = (x, y) => {
    let sp = 0;
    stkA[sp] = x; stkB[sp] = y; sp++;
    while (sp > 0) {
      sp--;
      const rx = find(stkA[sp]), ry = find(stkB[sp]);
      if (rx === ry) continue;
      const kx = kind[rx], ky = kind[ry];
      if (kx === 0) { parent[rx] = ry; unions++; }
      else if (ky === 0) { parent[ry] = rx; unions++; }
      else if (kx === 1 || ky === 1) {
        if (kx === 1 && ky === 1 && na[rx] === na[ry]) { parent[ry] = rx; unions++; }
        else return false;
      } else {
        parent[ry] = rx; unions++;
        stkA[sp] = na[rx]; stkB[sp] = na[ry]; sp++;
        stkA[sp] = nb[rx]; stkB[sp] = nb[ry]; sp++;
      }
    }
    return true;
  };
  let fails = 0;
  for (let c = 0; c < NCON; c++) {
    const t = nextRand() % 2;
    const x = t === 0 ? nextRand() % NVARS : nextRand() % NTOT;
    const y = nextRand() % NTOT;
    if (!unify(x, y)) fails++;
  }
  fold(fails); fold(unions);
  let classes = 0;
  for (let i = 0; i < NTOT; i++) {
    const r = find(i);
    if (r === i) classes++;
    fold(r * 3 + kind[r]);
  }
  fold(classes);
}

partA();
partB();
partC();
partD();
console.log(`CHECKSUM=${cs}`);
