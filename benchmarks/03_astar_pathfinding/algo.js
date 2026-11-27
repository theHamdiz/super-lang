/* 03_astar — 64×64 grid, 40 queries, binary min-heap */
const S = 64;
const CELLS = S * S;
const QUERIES = 40;
const INF = 0x3fffffff;

const LCG_MOD = 2147483648n;
let rng = 10846167n;
function nextU() {
  rng = (rng * 1103515245n + 12345n) % LCG_MOD;
  return Number(rng);
}

const walkable = new Int32Array(CELLS);
const gscore = new Int32Array(CELLS);
const came = new Int32Array(CELLS);
const openf = new Int32Array(CELLS);
const heap = new Int32Array(CELLS);
const hprio = new Int32Array(CELLS);
let hn = 0;

function heapPush(node, prio) {
  let i = hn++;
  heap[i] = node;
  hprio[i] = prio;
  while (i > 0) {
    const p = (i - 1) >> 1;
    if (hprio[i] >= hprio[p]) break;
    let tn = heap[i];
    heap[i] = heap[p];
    heap[p] = tn;
    let tp = hprio[i];
    hprio[i] = hprio[p];
    hprio[p] = tp;
    i = p;
  }
}

function heapPop() {
  const top = heap[0];
  hn--;
  heap[0] = heap[hn];
  hprio[0] = hprio[hn];
  let i = 0;
  for (;;) {
    const l = 2 * i + 1,
      r = l + 1;
    let sm = i;
    if (l < hn && hprio[l] < hprio[sm]) sm = l;
    if (r < hn && hprio[r] < hprio[sm]) sm = r;
    if (sm === i) break;
    let tn = heap[i];
    heap[i] = heap[sm];
    heap[sm] = tn;
    let tp = hprio[i];
    hprio[i] = hprio[sm];
    hprio[sm] = tp;
    i = sm;
  }
  return top;
}

function heur(a, b) {
  const ax = a % S,
    ay = (a / S) | 0,
    bx = b % S,
    by = (b / S) | 0;
  let dx = ax - bx;
  if (dx < 0) dx = -dx;
  let dy = ay - by;
  if (dy < 0) dy = -dy;
  return dx + dy;
}

const DIRS = [
  [1, 0],
  [-1, 0],
  [0, 1],
  [0, -1],
];

function astar(start, goal) {
  gscore.fill(INF);
  openf.fill(0);
  came.fill(-1);
  hn = 0;
  gscore[start] = 0;
  heapPush(start, heur(start, goal));
  openf[start] = 1;
  while (hn > 0) {
    const cur = heapPop();
    openf[cur] = 0;
    if (cur === goal) {
      let len = 0,
        c = goal;
      while (c !== start && c >= 0) {
        len++;
        c = came[c];
      }
      return len;
    }
    const cx = cur % S,
      cy = (cur / S) | 0;
    for (let d = 0; d < 4; d++) {
      const nx = cx + DIRS[d][0],
        ny = cy + DIRS[d][1];
      if (nx < 0 || ny < 0 || nx >= S || ny >= S) continue;
      const ni = ny * S + nx;
      if (!walkable[ni]) continue;
      const tent = gscore[cur] + 1;
      if (tent < gscore[ni]) {
        came[ni] = cur;
        gscore[ni] = tent;
        const f = tent + heur(ni, goal);
        if (!openf[ni]) {
          heapPush(ni, f);
          openf[ni] = 1;
        } else {
          heapPush(ni, f);
        }
      }
    }
  }
  return -1;
}

walkable.fill(1);
for (let i = 0; i < ((CELLS / 5) | 0); i++) {
  let cell = nextU() % CELLS;
  if (cell !== 0 && cell !== CELLS - 1) walkable[cell] = 0;
}

let cs = 0;
for (let q = 0; q < QUERIES; q++) {
  let a = nextU() % CELLS;
  let b = nextU() % CELLS;
  while (!walkable[a]) a = nextU() % CELLS;
  while (!walkable[b]) b = nextU() % CELLS;
  cs += astar(a, b);
}
console.log(`CHECKSUM=${cs}`);