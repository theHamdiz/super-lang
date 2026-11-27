/* 08_ecs — 2000 entities × 20 ticks, spatial grid neighbor queries */
const N = 2000;
const TICKS = 20;
const CELL = 10;
const GRID = 64;

const f32 = (x) => (x);

const LCG_MOD = 2147483648n;
let rng = 15482888n;
function nextU() {
  rng = (rng * 1103515245n + 12345n) % LCG_MOD;
  return Number(rng);
}
function nextF() {
  return nextU() / 2147483648.0;
}

const px = new Float64Array(N);
const py = new Float64Array(N);
const vx = new Float64Array(N);
const vy = new Float64Array(N);
const hp = new Int32Array(N);

for (let i = 0; i < N; i++) {
  px[i] = (nextF() * 600.0);
  py[i] = (nextF() * 600.0);
  vx[i] = ((nextF() - 0.5) * 2.0);
  vy[i] = ((nextF() - 0.5) * 2.0);
  hp[i] = 10 + (nextU() % 20);
}

const heads = new Int32Array(GRID * GRID);
const next = new Int32Array(N);

for (let t = 0; t < TICKS; t++) {
  for (let i = 0; i < N; i++) {
    px[i] = (px[i] + vx[i]);
    py[i] = (py[i] + vy[i]);
    if (px[i] < 0 || px[i] > 600) vx[i] = (-vx[i]);
    if (py[i] < 0 || py[i] > 600) vy[i] = (-vy[i]);
  }
  heads.fill(-1);
  for (let i = 0; i < N; i++) {
    let cx = (px[i] / CELL) | 0;
    let cy = (py[i] / CELL) | 0;
    if (cx < 0) cx = 0;
    if (cy < 0) cy = 0;
    if (cx >= GRID) cx = GRID - 1;
    if (cy >= GRID) cy = GRID - 1;
    const cell = cy * GRID + cx;
    next[i] = heads[cell];
    heads[cell] = i;
  }
  for (let i = 0; i < N; i++) {
    let cx = (px[i] / CELL) | 0;
    let cy = (py[i] / CELL) | 0;
    if (cx < 0) cx = 0;
    if (cy < 0) cy = 0;
    if (cx >= GRID) cx = GRID - 1;
    if (cy >= GRID) cy = GRID - 1;
    let hits = 0;
    for (let oy = -1; oy <= 1; oy++) {
      for (let ox = -1; ox <= 1; ox++) {
        const nx = cx + ox,
          ny = cy + oy;
        if (nx < 0 || ny < 0 || nx >= GRID || ny >= GRID) continue;
        let id = heads[ny * GRID + nx];
        while (id >= 0) {
          if (id !== i) {
            const dx = (px[i] - px[id]);
            const dy = (py[i] - py[id]);
            if ((dx * dx + dy * dy) < 4.0) hits++;
          }
          id = next[id];
        }
      }
    }
    if (hits > 3) hp[i] -= 1;
  }
}

let cs = 0;
for (let i = 0; i < N; i++) {
  cs += hp[i];
  cs += Math.floor((px[i] * 10.0));
  cs += Math.floor((py[i] * 10.0));
}
console.log(`CHECKSUM=${cs}`);