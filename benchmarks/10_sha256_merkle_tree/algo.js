/* 10_sha256_merkle — 1024 leaves × 64 bytes, SHA-256 Merkle root */
import { createHash } from "node:crypto";

const LEAVES = 1024;
const LEAF_SZ = 64;

function sha256(buf) {
  return createHash("sha256").update(buf).digest();
}

const leaf = Buffer.alloc(LEAF_SZ);
let level = [];
for (let i = 0; i < LEAVES; i++) {
  for (let b = 0; b < LEAF_SZ; b++) leaf[b] = (i * 131 + b * 17) & 255;
  level.push(sha256(leaf));
}

let n = LEAVES;
while (n > 1) {
  const m = ((n + 1) / 2) | 0;
  const nxt = [];
  for (let i = 0; i < m; i++) {
    const cat = Buffer.alloc(64);
    level[2 * i].copy(cat, 0);
    if (2 * i + 1 < n) level[2 * i + 1].copy(cat, 32);
    else level[2 * i].copy(cat, 32);
    nxt.push(sha256(cat));
  }
  level = nxt;
  n = m;
}

let cs = 0n;
const MASK = (1n << 64n) - 1n;
for (const b of level[0]) cs = (cs * 131n + BigInt(b)) & MASK;
if (cs >= (1n << 63n)) cs -= (1n << 64n);
console.log(`CHECKSUM=${cs}`);