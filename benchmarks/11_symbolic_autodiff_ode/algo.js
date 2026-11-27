// 11_symbolic_autodiff_ode — 20k random expressions: simplify + forward dual AD,
// then dual-RK4 sensitivities of a stiff-ish ODE
const OP_CONST = 0, OP_VAR = 1, OP_ADD = 2, OP_SUB = 3, OP_MUL = 4, OP_NEG = 5, OP_SIN = 6;
const EXPRS = 20000, POINTS = 7, TRAJ = 1024, STEPS = 1000, MAXN = 64;
const LEAF = [0.0, 1.0, 2.0, 0.5, -1.0];

let rng = 20240611;
// Low 31 bits of rng * 1103515245 + 12345 (wraps like the 64-bit LCG mod 2^31).
function lcg() {
  rng = (Math.imul(rng, 1103515245) + 12345) & 0x7fffffff;
  return rng;
}

class Tree {
  constructor() {
    this.op = new Int32Array(MAXN); this.a = new Int32Array(MAXN); this.b = new Int32Array(MAXN);
    this.cv = new Float64Array(MAXN); this.cnt = 0;
  }
}

function gen(t, depth) {
  const k = (lcg() >> 8) % 10;
  if (k < 2 || depth === 0) {
    const m = (lcg() >> 8) % 8;
    const n = t.cnt++;
    if (m < 3) { t.op[n] = OP_VAR; t.a[n] = 0; t.b[n] = 0; t.cv[n] = 0.0; }
    else { t.op[n] = OP_CONST; t.a[n] = 0; t.b[n] = 0; t.cv[n] = LEAF[m - 3]; }
    return n;
  }
  const a = gen(t, depth - 1);
  let kind = OP_SIN;
  if (k === 8) kind = OP_NEG;
  if (k >= 5 && k < 8) kind = OP_MUL;
  if (k === 4) kind = OP_SUB;
  if (k < 4) kind = OP_ADD;
  let b = 0;
  if (kind === OP_ADD || kind === OP_SUB || kind === OP_MUL) b = gen(t, depth - 1);
  const n = t.cnt++;
  t.op[n] = kind; t.a[n] = a; t.b[n] = b; t.cv[n] = 0.0;
  return n;
}

const isConst = (s, i, c) => s.op[i] === OP_CONST && s.cv[i] === c;

function push(s, kind, a, b, c) {
  const n = s.cnt++;
  s.op[n] = kind; s.a[n] = a; s.b[n] = b; s.cv[n] = c;
  return n;
}

function simpNode(s, kind, cv, l, r) {
  switch (kind) {
    case OP_CONST: return push(s, OP_CONST, 0, 0, cv);
    case OP_VAR: return push(s, OP_VAR, 0, 0, 0.0);
    case OP_ADD:
      if (isConst(s, l, 0.0)) return r;
      if (isConst(s, r, 0.0)) return l;
      if (s.op[l] === OP_CONST && s.op[r] === OP_CONST) return push(s, OP_CONST, 0, 0, s.cv[l] + s.cv[r]);
      return push(s, OP_ADD, l, r, 0.0);
    case OP_SUB:
      if (isConst(s, r, 0.0)) return l;
      if (s.op[l] === OP_CONST && s.op[r] === OP_CONST) return push(s, OP_CONST, 0, 0, s.cv[l] - s.cv[r]);
      if (l === r) return push(s, OP_CONST, 0, 0, 0.0);
      return push(s, OP_SUB, l, r, 0.0);
    case OP_MUL:
      if (isConst(s, l, 0.0) || isConst(s, r, 0.0)) return push(s, OP_CONST, 0, 0, 0.0);
      if (isConst(s, l, 1.0)) return r;
      if (isConst(s, r, 1.0)) return l;
      if (s.op[l] === OP_CONST && s.op[r] === OP_CONST) return push(s, OP_CONST, 0, 0, s.cv[l] * s.cv[r]);
      return push(s, OP_MUL, l, r, 0.0);
    case OP_NEG:
      if (s.op[l] === OP_CONST) return push(s, OP_CONST, 0, 0, -s.cv[l]);
      if (s.op[l] === OP_NEG) return s.a[l];
      return push(s, OP_NEG, l, 0, 0.0);
    default:
      if (isConst(s, l, 0.0)) return push(s, OP_CONST, 0, 0, 0.0);
      return push(s, OP_SIN, l, 0, 0.0);
  }
}

function simplify(t, s, rep) {
  s.cnt = 0;
  for (let i = 0; i < t.cnt; i++) rep[i] = simpNode(s, t.op[i], t.cv[i], rep[t.a[i]], rep[t.b[i]]);
}

function evalDual(t, x, ev, ed) {
  for (let i = 0; i < t.cnt; i++) {
    const ia = t.a[i], ib = t.b[i];
    let rv, rd;
    switch (t.op[i]) {
      case OP_CONST: rv = t.cv[i]; rd = 0.0; break;
      case OP_VAR: rv = x; rd = 1.0; break;
      case OP_ADD: rv = ev[ia] + ev[ib]; rd = ed[ia] + ed[ib]; break;
      case OP_SUB: rv = ev[ia] - ev[ib]; rd = ed[ia] - ed[ib]; break;
      case OP_MUL: rv = ev[ia] * ev[ib]; rd = ev[ia] * ed[ib] + ed[ia] * ev[ib]; break;
      case OP_NEG: rv = -ev[ia]; rd = -ed[ia]; break;
      default: rv = Math.sin(ev[ia]); rd = Math.cos(ev[ia]) * ed[ia]; break;
    }
    ev[i] = rv; ed[i] = rd;
  }
}

// rhs returns [value, derivative] through module-level scratch to avoid allocation.
let rv = 0.0, rd = 0.0;
function rhs(lam, t, v, d) {
  const s = Math.sin(t), c = Math.cos(t);
  const dv = v - s;
  const pv = lam * dv;
  const pd = lam * d + 1.0 * dv;
  rv = -pv + 0.5 * c;
  rd = -pd;
}

function rk4Final(lam, y0, h, steps) {
  let yv = y0, yd = 0.0;
  const hh = 0.5 * h, h6 = h / 6.0;
  for (let k = 0; k < steps; k++) {
    const t = k * h;
    rhs(lam, t, yv, yd); const k1v = rv, k1d = rd;
    rhs(lam, t + hh, yv + k1v * hh, yd + k1d * hh); const k2v = rv, k2d = rd;
    rhs(lam, t + hh, yv + k2v * hh, yd + k2d * hh); const k3v = rv, k3d = rd;
    rhs(lam, t + h, yv + k3v * h, yd + k3d * h); const k4v = rv, k4d = rd;
    yv = yv + (((k1v + k2v * 2.0) + k3v * 2.0) + k4v) * h6;
    yd = yd + (((k1d + k2d * 2.0) + k3d * 2.0) + k4d) * h6;
  }
  return [yv, yd];
}

const t = new Tree(), s = new Tree();
const rep = new Int32Array(MAXN);
const ev = new Float64Array(MAXN), ed = new Float64Array(MAXN);
const sev = new Float64Array(MAXN), sed = new Float64Array(MAXN);
let origNodes = 0, simpNodes = 0, mismatch = 0, accV = 0.0, accD = 0.0;
for (let e = 0; e < EXPRS; e++) {
  t.cnt = 0;
  gen(t, 4);
  const n = t.cnt;
  simplify(t, s, rep);
  const m = s.cnt;
  origNodes += n;
  simpNodes += m;
  for (let p = 0; p < POINTS; p++) {
    const x = (p - 3) / 4.0;
    evalDual(t, x, ev, ed);
    evalDual(s, x, sev, sed);
    const vo = ev[n - 1], dro = ed[n - 1], vs = sev[rep[n - 1]], ds = sed[rep[n - 1]];
    if (Math.abs(vo - vs) > 0.000000001 * (1.0 + Math.abs(vo))) mismatch++;
    if (Math.abs(dro - ds) > 0.000000001 * (1.0 + Math.abs(dro))) mismatch++;
    accV += vs;
    accD += ds;
  }
}

let sumY = 0.0, sumDy = 0.0;
for (let j = 0; j < TRAJ; j++) {
  const r = rk4Final(10.0 + j * 0.25, 1.0, 0.002, STEPS);
  sumY += r[0];
  sumDy += r[1];
}

const q = Math.floor(accV * 10000.0);
const qd = Math.floor(accD * 10000.0);
const y1 = Math.floor(sumY * 1000000.0);
const y2 = Math.floor(sumDy * 1000000000.0);
const cs = origNodes + simpNodes * 7 + mismatch * 13 + q * 3 + qd * 5 + y1 * 11 + y2 * 17;
console.log(`CHECKSUM=${cs}`);
