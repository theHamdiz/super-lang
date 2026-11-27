/* 19_distributed_raft_wal_kv — single-process Raft log replication (leader + 2 followers):
   CRC32-protected WAL records, lossy/corrupting virtual links, majority commit, KV state machine. */
const MOD = 1000000007;
const TICKS = 6000, DRAIN = 200, REC = 20, BATCH = 4, KEYS = 512;
const DROP_PCT = 35, ACK_DROP_PCT = 15, CORRUPT_PCT = 3;
const CAP = TICKS * REC;

const CRC = new Uint32Array(256);
for (let i = 0; i < 256; i++) {
  let c = i;
  for (let k = 0; k < 8; k++) c = c & 1 ? (c >>> 1) ^ 0xedb88320 : c >>> 1;
  CRC[i] = c >>> 0;
}
function crc32(d, off, n) {
  let c = 0xffffffff;
  for (let i = off; i < off + n; i++) c = (CRC[(c ^ d[i]) & 255] ^ (c >>> 8)) >>> 0;
  return (c ^ 0xffffffff) >>> 0;
}
function put32(p, o, v) { p[o] = (v >>> 24) & 255; p[o + 1] = (v >>> 16) & 255; p[o + 2] = (v >>> 8) & 255; p[o + 3] = v & 255; }
function get32(p, o) { return ((p[o] << 24) | (p[o + 1] << 16) | (p[o + 2] << 8) | p[o + 3]) >>> 0; }

let rng = 19019;
const nextRand = () => (rng = (rng * 48271) % 2147483647);

const wal = [new Uint8Array(CAP), new Uint8Array(CAP), new Uint8Array(CAP)];
const kv = [new Int32Array(KEYS), new Int32Array(KEYS), new Int32Array(KEYS)];
const lens = [0, 0, 0], applied = [0, 0, 0], match = [0, 0, 0], nxt = [1, 1, 1];
let commit = 0, sent = 0, dropped = 0, crcRejects = 0, acksLost = 0;

function applyUpto(n, upto) {
  const w = wal[n], m = kv[n];
  while (applied[n] < upto) {
    const o = applied[n] * REC;
    if (crc32(w, o, 16) === get32(w, o + 16)) m[get32(w, o + 8) % KEYS] = get32(w, o + 12);
    applied[n]++;
  }
}

for (let t = 0; t < TICKS + DRAIN; t++) {
  if (t < TICKS) {
    const w = wal[0], o = lens[0] * REC;
    put32(w, o, lens[0] + 1);
    put32(w, o + 4, 1);
    put32(w, o + 8, nextRand() % KEYS);
    put32(w, o + 12, nextRand() % 1000000);
    put32(w, o + 16, crc32(w, o, 16));
    lens[0]++;
  }
  const leaderCommit = commit;
  for (let f = 1; f <= 2; f++) {
    const n0 = nxt[f];
    if (n0 > lens[0]) continue;
    const cnt = Math.min(lens[0] - n0 + 1, BATCH);
    const r1 = nextRand() % 100, r2 = nextRand() % 100, r3 = nextRand() % 100;
    sent++;
    if (r1 < DROP_PCT) { dropped++; continue; }
    const msg = wal[0].slice((n0 - 1) * REC, (n0 - 1) * REC + cnt * REC);
    if (r2 < CORRUPT_PCT) msg[nextRand() % (cnt * REC)] ^= 0x5a;
    for (let j = 0; j < cnt; j++) {
      const o = j * REC;
      if (crc32(msg, o, 16) !== get32(msg, o + 16)) { crcRejects++; break; }
      const idx = get32(msg, o);
      if (idx === lens[f] + 1) {
        wal[f].set(msg.subarray(o, o + REC), lens[f] * REC);
        lens[f]++;
      } else if (idx > lens[f] + 1) break;
    }
    applyUpto(f, Math.min(leaderCommit, lens[f]));
    if (r3 < ACK_DROP_PCT) { acksLost++; continue; }
    if (lens[f] > match[f]) match[f] = lens[f];
    nxt[f] = lens[f] + 1;
  }
  const med = [lens[0], match[1], match[2]].sort((a, b) => a - b)[1];
  if (med > commit) commit = med;
  applyUpto(0, commit);
}

let cs = 0;
const fold = (v) => { cs = (cs * 131 + v) % MOD; };
for (const v of [...lens, commit, ...applied, sent, dropped, crcRejects, acksLost]) fold(v);
for (let n = 0; n < 3; n++) {
  let d = 0;
  for (let k = 0; k < KEYS; k++) d = (d * 131 + kv[n][k]) % MOD;
  fold(d);
}
for (let n = 0; n < 2; n++) fold(crc32(wal[n], 0, lens[n] * REC));
console.log(`CHECKSUM=${cs}`);
