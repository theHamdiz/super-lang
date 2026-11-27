/* 15_physics2d_broadphase_solver - 6k circles/boxes: uniform spatial-hash broadphase,
   circle/box manifolds, sequential-impulse solver (4 iterations x 10 steps).
   Mirrors std.game.physics2d (grid build, candidate order, narrowphase, resolve). */
const COLS = 100, ROWS = 60, N_DYN = COLS * ROWS;
const FLOOR_TILES = 62, WALL_TILES = 40;
const STEPS = 10, ITERS = 4;
const MOD = 1000000007;

let rng = 150015;
function draw(m) {
  rng = (Math.imul(rng, 1103515245) + 12345) & 0x7fffffff;
  return Math.floor(rng / 256) % m;
}

const N = FLOOR_TILES + 2 * WALL_TILES + N_DYN;
// x y vx vy inv_mass shape r hw hh (e = 0.2, mu = 0.4 for every body)
const X = new Float64Array(N), Y = new Float64Array(N), VX = new Float64Array(N), VY = new Float64Array(N);
const IM = new Float64Array(N), R = new Float64Array(N), HW = new Float64Array(N), HH = new Float64Array(N);
const SH = new Int32Array(N);
let n = 0;

function add(x, y, vx, vy, mass, shape, r, hw, hh) {
  X[n] = x; Y[n] = y; VX[n] = vx; VY[n] = vy;
  IM[n] = mass <= 0.0 ? 0.0 : 1.0 / mass;
  SH[n] = shape; R[n] = r; HW[n] = hw; HH[n] = hh;
  n++;
}

const sign = (v) => (v < 0.0 ? -1.0 : 1.0);
const clamp = (v, lo, hi) => (v < lo ? lo : v > hi ? hi : v);

// narrowphase result lands in these (hit flag returned)
let cnx = 0, cny = 0, cdepth = 0;

function circleCircle(a, b) {
  const dx = X[b] - X[a], dy = Y[b] - Y[a];
  const rr = R[a] + R[b];
  const d2 = dx * dx + dy * dy;
  if (d2 >= rr * rr) return false;
  if (d2 === 0.0) { cnx = 1.0; cny = 0.0; cdepth = rr; return true; }
  const d = Math.sqrt(d2);
  cnx = dx / d; cny = dy / d; cdepth = rr - d;
  return true;
}

function boxBox(a, b) {
  const dx = X[b] - X[a], dy = Y[b] - Y[a];
  const ox = HW[a] + HW[b] - Math.abs(dx);
  const oy = HH[a] + HH[b] - Math.abs(dy);
  if (ox <= 0.0 || oy <= 0.0) return false;
  if (ox < oy) { cnx = sign(dx); cny = 0.0; cdepth = ox; return true; }
  cnx = 0.0; cny = sign(dy); cdepth = oy;
  return true;
}

function circleBox(a, b) {
  const lx = X[a] - X[b], ly = Y[a] - Y[b];
  const cx = clamp(lx, 0.0 - HW[b], HW[b]);
  const cy = clamp(ly, 0.0 - HH[b], HH[b]);
  const fx = lx - cx, fy = ly - cy;
  const d2 = fx * fx + fy * fy;
  if (d2 > 0.0) {
    if (d2 >= R[a] * R[a]) return false;
    const d = Math.sqrt(d2);
    cnx = (0.0 - fx) / d; cny = (0.0 - fy) / d; cdepth = R[a] - d;
    return true;
  }
  const px = HW[b] - Math.abs(lx), py = HH[b] - Math.abs(ly);
  if (px < py) { cnx = 0.0 - sign(lx); cny = 0.0; cdepth = px + R[a]; return true; }
  cnx = 0.0; cny = 0.0 - sign(ly); cdepth = py + R[a];
  return true;
}

function collide(a, b) {
  const sa = SH[a], sb = SH[b];
  if (sa === 0 && sb === 0) return circleCircle(a, b);
  if (sa === 1 && sb === 1) return boxBox(a, b);
  if (sa === 0 && sb === 1) return circleBox(a, b);
  if (!circleBox(b, a)) return false;
  cnx = 0.0 - cnx; cny = 0.0 - cny;
  return true;
}

function resolve(a, b, nx, ny, depth) {
  const ia = IM[a], ib = IM[b], isum = ia + ib;
  if (isum === 0.0) return;
  const rvx = VX[b] - VX[a], rvy = VY[b] - VY[a];
  const vn = rvx * nx + rvy * ny;
  if (vn < 0.0) {
    const e = 0.2;
    const j = (0.0 - (1.0 + e)) * vn / isum;
    VX[a] = VX[a] - j * ia * nx;
    VY[a] = VY[a] - j * ia * ny;
    VX[b] = VX[b] + j * ib * nx;
    VY[b] = VY[b] + j * ib * ny;
    let tx = rvx - vn * nx, ty = rvy - vn * ny;
    const tl = Math.sqrt(tx * tx + ty * ty);
    if (tl > 0.000001) {
      tx = tx / tl;
      ty = ty / tl;
      let jt = (0.0 - (rvx * tx + rvy * ty)) / isum;
      const mu = Math.sqrt(0.4 * 0.4);
      const lim = mu * j;
      jt = clamp(jt, 0.0 - lim, lim);
      VX[a] = VX[a] - jt * ia * tx;
      VY[a] = VY[a] - jt * ia * ty;
      VX[b] = VX[b] + jt * ib * tx;
      VY[b] = VY[b] + jt * ib * ty;
    }
  }
  const corr = Math.max(depth - 0.01, 0.0) / isum * 0.8;
  X[a] = X[a] - corr * ia * nx;
  Y[a] = Y[a] - corr * ia * ny;
  X[b] = X[b] + corr * ib * nx;
  Y[b] = Y[b] + corr * ib * ny;
}

const bucket = (cx, cy, mask) => (Math.imul(cx, 73856093) ^ Math.imul(cy, 19349663)) & mask;

let heads = null, eb = null, ex = null, ey = null, en = null;
const mark = new Int32Array(N);
let cands = new Int32Array(N);

function solveGrid() {
  let s = 0.0, big = 0.0;
  for (let i = 0; i < n; i++) {
    const e = Math.max(HW[i], HH[i]);
    s += e;
    if (e > big) big = e;
  }
  let cell = Math.max(2.0 * s / n, big / 8.0);
  if (cell < 0.000001) cell = 1.0;
  const inv = 1.0 / cell;
  const pad = cell * 0.1;
  let size = 16;
  while (size < n * 2) size *= 2;
  const mask = size - 1;
  heads = new Int32Array(size).fill(-1);
  eb = []; ex = []; ey = []; en = [];
  for (let i = 0; i < n; i++) {
    const cx0 = Math.floor((X[i] - HW[i] - pad) * inv), cx1 = Math.floor((X[i] + HW[i] + pad) * inv);
    const cy0 = Math.floor((Y[i] - HH[i] - pad) * inv), cy1 = Math.floor((Y[i] + HH[i] + pad) * inv);
    for (let cy = cy0; cy < cy1 + 1; cy++) {
      for (let cx = cx0; cx < cx1 + 1; cx++) {
        const slot = bucket(cx, cy, mask);
        en.push(heads[slot]);
        heads[slot] = eb.length;
        eb.push(i); ex.push(cx); ey.push(cy);
      }
    }
  }
  mark.fill(0);
  const qpad = 0.1 / inv;
  let last = 0;
  for (let i = 0; i < n; i++) {
    const cx0 = Math.floor((X[i] - HW[i] - qpad) * inv), cx1 = Math.floor((X[i] + HW[i] + qpad) * inv);
    const cy0 = Math.floor((Y[i] - HH[i] - qpad) * inv), cy1 = Math.floor((Y[i] + HH[i] + qpad) * inv);
    let cnt = 0;
    for (let cy = cy0; cy < cy1 + 1; cy++) {
      for (let cx = cx0; cx < cx1 + 1; cx++) {
        for (let e = heads[bucket(cx, cy, mask)]; e >= 0; e = en[e]) {
          if (ex[e] === cx && ey[e] === cy) {
            const j = eb[e];
            if (j > i && mark[j] !== i + 1) {
              mark[j] = i + 1;
              cands[cnt++] = j;
            }
          }
        }
      }
    }
    cands.subarray(0, cnt).sort();
    for (let k = 0; k < cnt; k++) {
      const j = cands[k];
      if (IM[i] + IM[j] > 0.0 &&
          !(Math.abs(X[j] - X[i]) >= HW[i] + HW[j]) && !(Math.abs(Y[j] - Y[i]) >= HH[i] + HH[j]) &&
          collide(i, j)) {
        resolve(i, j, cnx, cny, cdepth);
        last++;
      }
    }
  }
  return last;
}

const posmod = (v) => ((v % MOD) + MOD) % MOD;
const fl = Math.floor;

for (let k = 0; k < FLOOR_TILES; k++) add(1.0 + 2.0 * k, -1.0, 0.0, 0.0, 0.0, 1, 0.0, 1.0, 1.0);
for (let k = 0; k < WALL_TILES; k++) add(-1.0, 1.0 + 2.0 * k, 0.0, 0.0, 0.0, 1, 0.0, 1.0, 1.0);
for (let k = 0; k < WALL_TILES; k++) add(122.0, 1.0 + 2.0 * k, 0.0, 0.0, 0.0, 1, 0.0, 1.0, 1.0);
for (let i = 0; i < N_DYN; i++) {
  const col = i % COLS, row = Math.floor(i / COLS);
  const kind = draw(3);
  const sw = 0.35 + draw(400001) * 0.000001;
  const sh = 0.35 + draw(400001) * 0.000001;
  const mass = 1.0 + draw(4);
  const jx = (draw(600001) - 300000) * 0.000001;
  const jy = (draw(600001) - 300000) * 0.000001;
  const vx = (draw(8000001) - 4000000) * 0.000001;
  const vy = (draw(8000001) - 4000000) * 0.000001;
  const x = 0.75 + col * 1.2 + jx;
  const y = 1.75 + row * 1.2 + jy;
  if (kind === 0) add(x, y, vx, vy, mass, 1, 0.0, sw, sh);
  else add(x, y, vx, vy, mass, 0, sw, sw, sw);
}

let cs = 0;
const dt = 1.0 / 60.0;
const damp = 1.0 / (1.0 + dt * 0.0);
for (let s = 0; s < STEPS; s++) {
  for (let i = 0; i < n; i++) {
    if (IM[i] > 0.0) {
      VX[i] = (VX[i] + 0.0 * dt) * damp;
      VY[i] = (VY[i] + -9.81 * dt) * damp;
      X[i] = X[i] + VX[i] * dt;
      Y[i] = Y[i] + VY[i] * dt;
    }
  }
  let last = 0;
  for (let it = 0; it < ITERS; it++) last = solveGrid();
  cs += last * (s + 1);
}
let h = 0, ke = 0.0;
for (let i = 0; i < n; i++) {
  h = (h * 131 + posmod(fl(X[i] * 1000.0))) % MOD;
  h = (h * 131 + posmod(fl(Y[i] * 1000.0))) % MOD;
  h = (h * 131 + posmod(fl(VX[i] * 1000.0))) % MOD;
  h = (h * 131 + posmod(fl(VY[i] * 1000.0))) % MOD;
  if (IM[i] > 0.0) ke += 0.5 * (VX[i] * VX[i] + VY[i] * VY[i]) / IM[i];
}
cs += h + fl(ke * 10.0);
console.log(`CHECKSUM=${cs}`);
