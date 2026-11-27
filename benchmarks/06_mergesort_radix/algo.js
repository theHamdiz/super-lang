/* 06_mergesort_radix — sort 50000 u64, verify + checksum */
const N = 50000;

const LCG_MOD = 2147483648n;
let rng = 395799n;
function nextU() {
  rng = (rng * 1103515245n + 12345n) % LCG_MOD;
  return Number(rng);
}
const i64 = (x) => BigInt.asIntN(64, BigInt(x));

function merge(a, tmp, lo, mid, hi) {
  let i = lo,
    j = mid,
    k = lo;
  while (i < mid && j < hi) {
    if (a[i] <= a[j]) tmp[k++] = a[i++];
    else tmp[k++] = a[j++];
  }
  while (i < mid) tmp[k++] = a[i++];
  while (j < hi) tmp[k++] = a[j++];
  for (i = lo; i < hi; i++) a[i] = tmp[i];
}

function mergesort(a, tmp, lo, hi) {
  if (hi - lo <= 1) return;
  const mid = lo + ((hi - lo) >> 1);
  mergesort(a, tmp, lo, mid);
  mergesort(a, tmp, mid, hi);
  merge(a, tmp, lo, mid, hi);
}

const a = new BigUint64Array(N);
const tmp = new BigUint64Array(N);
for (let i = 0; i < N; i++) a[i] = BigInt(nextU());

mergesort(a, tmp, 0, N);

let cs = 0n;
let ok = 1;
for (let i = 0; i < N; i++) {
  const v = a[i];
  cs = i64(cs ^ i64(v & 0xffffffffn));
  cs = i64(cs * 131n + i64(v >> 32n));
  if (i > 0 && a[i] < a[i - 1]) ok = 0;
}
console.log(`CHECKSUM=${i64(cs ^ BigInt(ok ? 1 : 0))}`);