// 13_polars_data_pipeline — 64k orders: filter -> inner join -> group_by aggregates -> rolling-mean window
const ROWS = 64000, CUSTOMERS = 2000, KNOWN = 1800, REGIONS = 16;

let rng = 130013;
function lcg() {
  rng = (Math.imul(rng, 1103515245) + 12345) & 0x7fffffff;
  return rng;
}

const cust = new Int32Array(ROWS), qty = new Int32Array(ROWS), amtq = new Int32Array(ROWS);
const amt = new Float64Array(ROWS);
for (let i = 0; i < ROWS; i++) {
  const c = (lcg() >> 8) % CUSTOMERS;
  const q = 1 + (lcg() >> 8) % 9;
  const p = (lcg() >> 8) % 400;
  cust[i] = c; qty[i] = q; amtq[i] = q * p; amt[i] = q * p * 0.25;
}
// dimension table: direct-address hash on customer id
const regionOf = new Int32Array(CUSTOMERS).fill(-1);
for (let c = 0; c < KNOWN; c++) regionOf[c] = (lcg() >> 8) % 16;

let big = 0, joined = 0, rollSum = 0, rollCnt = 0;
const tot = new Float64Array(REGIONS), n = new Float64Array(REGIONS), mx = new Float64Array(REGIONS);
const sumAmt = new Float64Array(REGIONS);
const ring = new Float64Array(REGIONS * 8);
const seen = new Int32Array(REGIONS);
for (let i = 0; i < ROWS; i++) {
  if (!(amt[i] >= 20.0 && qty[i] > 1)) continue;
  big++;
  const r = regionOf[cust[i]];
  if (r < 0) continue;
  joined++;
  tot[r] += amtq[i];
  n[r] += 1;
  sumAmt[r] += amt[i];
  if (n[r] === 1 || amtq[i] > mx[r]) mx[r] = amtq[i];
  ring[r * 8 + (seen[r] % 8)] = amt[i];
  seen[r]++;
  if (seen[r] >= 8) {
    let s = 0.0;
    for (let k = 0; k < 8; k++) s += ring[r * 8 + ((seen[r] - 8 + k) % 8)];
    rollSum += Math.floor(s / 8.0 * 1024.0);
    rollCnt++;
  }
}
let cs = big + joined * 3;
for (let r = 0; r < REGIONS; r++) {
  if (n[r] === 0) continue;
  const avg = sumAmt[r] / n[r];
  cs += (r + 1) * (tot[r] + 7 * n[r] + Math.floor(avg * 1024.0) + 13 * mx[r]);
}
cs += rollSum + 5 * rollCnt;
console.log(`CHECKSUM=${cs}`);
