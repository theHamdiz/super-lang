/* 05_parallel_mapreduce — 1 MiB corpus, word + 3-gram frequencies */
import os from "node:os";
import { Worker } from "node:worker_threads";

const CORPUS = 65536;
const BUCKETS = 65536;
const WORDS = [
  "alpha",
  "bravo",
  "charlie",
  "delta",
  "echo",
  "foxtrot",
  "golf",
  "hotel",
  "india",
  "juliet",
];

const U64 = 0xffffffffffffffffn;
const i64 = (x) => BigInt.asIntN(64, BigInt(x));

function hashBytes(data, start, n) {
  let h = 2166136261;
  for (let i = 0; i < n; i++) {
    h ^= data[start + i];
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

function bump(tab, key, add) {
  let i = key & (BUCKETS - 1);
  for (let probe = 0; probe < BUCKETS; probe++) {
    const j = (i + probe) & (BUCKETS - 1);
    const slot = j * 2;
    if (tab[slot + 1] === 0) {
      tab[slot] = key >>> 0;
      tab[slot + 1] = add;
      return;
    }
    if (tab[slot] === (key >>> 0)) {
      tab[slot + 1] += add;
      return;
    }
  }
}

function scanRange(text, begin, end, tab) {
  let i = begin;
  while (i < end) {
    while (i < end && (text[i] < 97 || text[i] > 122)) i++;
    const start = i;
    while (i < end && text[i] >= 97 && text[i] <= 122) i++;
    if (i > start) {
      const len = i - start;
      bump(tab, hashBytes(text, start, len), 1);
      if (len >= 3) bump(tab, hashBytes(text, start, 3) ^ 0xc0ffee, 1);
    }
  }
}

let nworkers = os.cpus().length;
if (nworkers < 1) nworkers = 1;
if (nworkers > 32) nworkers = 32;

const text = new Uint8Array(CORPUS + 1);
const LCG_MOD = 2147483648n;
let rng = 370945n;
let off = 0;
while (off + 16 < CORPUS) {
  rng = (rng * 1103515245n + 12345n) % LCG_MOD;
  const w = WORDS[Number(rng % 10n)];
  for (let k = 0; k < w.length; k++) text[off++] = w.charCodeAt(k);
  text[off++] = 32;
}
while (off < CORPUS) text[off++] = 120;

const chunk = Math.floor(CORPUS / nworkers);
const workerSrc = `
const BUCKETS = ${BUCKETS};
const { parentPort, workerData } = require('node:worker_threads');
function hashBytes(data, start, n) {
  let h = 2166136261;
  for (let i = 0; i < n; i++) {
    h ^= data[start + i];
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}
function bump(tab, key, add) {
  let i = key & (BUCKETS - 1);
  for (let probe = 0; probe < BUCKETS; probe++) {
    const j = (i + probe) & (BUCKETS - 1);
    const slot = j * 2;
    if (tab[slot + 1] === 0) { tab[slot] = key >>> 0; tab[slot + 1] = add; return; }
    if (tab[slot] === (key >>> 0)) { tab[slot + 1] += add; return; }
  }
}
const text = new Uint8Array(workerData.buf);
const tab = new Uint32Array(BUCKETS * 2);
let i = workerData.begin;
while (i < workerData.end) {
  while (i < workerData.end && (text[i] < 97 || text[i] > 122)) i++;
  const start = i;
  while (i < workerData.end && text[i] >= 97 && text[i] <= 122) i++;
  if (i > start) {
    const len = i - start;
    bump(tab, hashBytes(text, start, len), 1);
    if (len >= 3) bump(tab, hashBytes(text, start, 3) ^ 0xC0FFEE, 1);
  }
}
parentPort.postMessage(tab);
`;

const tables = await Promise.all(
  Array.from({ length: nworkers }, (_, id) => {
    const begin = id * chunk;
    const end = id === nworkers - 1 ? CORPUS : begin + chunk;
    return new Promise((resolve, reject) => {
      const w = new Worker(workerSrc, {
        eval: true,
        workerData: { buf: text.buffer, begin, end },
      });
      w.on("message", (tab) => {
        w.terminate();
        resolve(tab);
      });
      w.on("error", reject);
    });
  }),
);

const scratch = new Uint32Array(BUCKETS * 2);
for (const tab of tables) {
  for (let i = 0; i < BUCKETS; i++) {
    if (tab[i * 2 + 1]) bump(scratch, tab[i * 2], tab[i * 2 + 1]);
  }
}

const merged = new Uint32Array(BUCKETS * 2);
scanRange(text, 0, CORPUS, merged);

let total = 0;
let best = 0;
let bestKey = 0;
for (let k = 0; k < BUCKETS; k++) {
  const cnt = merged[k * 2 + 1];
  if (!cnt) continue;
  total += cnt;
  const key = merged[k * 2];
  if (cnt > best || (cnt === best && key < bestKey)) {
    best = cnt;
    bestKey = key;
  }
}
const cs = i64(BigInt(total) ^ i64(BigInt(best) * 1315423911n) ^ BigInt(bestKey));
console.log(`CHECKSUM=${cs}`);