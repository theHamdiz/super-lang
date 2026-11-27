/* 01_nbody — 64 bodies × 500 steps, soft gravity, SoA */
const N = 64;
const STEPS = 500;
const DT = 0.001;
const SOFT = 0.01;

const LCG_MOD = 2147483648n;
let rng = 12648430n;
function nextU() {
  rng = (rng * 1103515245n + 12345n) % LCG_MOD;
  return Number(rng);
}
function nextF() {
  return nextU() / 2147483648.0;
}

const x = new Float64Array(N);
const y = new Float64Array(N);
const vx = new Float64Array(N);
const vy = new Float64Array(N);
const m = new Float64Array(N);

for (let i = 0; i < N; i++) {
  x[i] = nextF() * 2.0 - 1.0;
  y[i] = nextF() * 2.0 - 1.0;
  vx[i] = (nextF() - 0.5) * 0.01;
  vy[i] = (nextF() - 0.5) * 0.01;
  m[i] = 0.5 + nextF();
}

for (let step = 0; step < STEPS; step++) {
  for (let i = 0; i < N; i++) {
    let ax = 0.0,
      ay = 0.0;
    for (let j = 0; j < N; j++) {
      if (i === j) continue;
      const dx = x[j] - x[i];
      const dy = y[j] - y[i];
      const d2 = dx * dx + dy * dy + SOFT;
      const inv = 1.0 / Math.sqrt(d2);
      const inv3 = inv * inv * inv;
      ax += m[j] * dx * inv3;
      ay += m[j] * dy * inv3;
    }
    vx[i] += ax * DT;
    vy[i] += ay * DT;
  }
  for (let i = 0; i < N; i++) {
    x[i] += vx[i] * DT;
    y[i] += vy[i] * DT;
  }
}

let cs = 0;
for (let i = 0; i < N; i++) {
  cs += Math.floor(x[i] * 1000.0);
  cs += Math.floor(y[i] * 1000.0);
  cs += Math.floor(vx[i] * 1e6);
}
console.log(`CHECKSUM=${cs}`);