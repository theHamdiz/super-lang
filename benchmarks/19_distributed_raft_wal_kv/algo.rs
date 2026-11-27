// 19_distributed_raft_wal_kv — single-process Raft log replication (leader + 2 followers):
// CRC32-protected WAL records, lossy/corrupting virtual links, majority commit, KV state machine.
// Build: rustc -C opt-level=3 -C target-cpu=native algo.rs

const MOD: i64 = 1_000_000_007;
const TICKS: usize = 6000;
const DRAIN: usize = 200;
const REC: usize = 20;
const BATCH: usize = 4;
const KEYS: usize = 512;
const DROP_PCT: i64 = 35;
const ACK_DROP_PCT: i64 = 15;
const CORRUPT_PCT: i64 = 3;
const CAP: usize = TICKS * REC;

const fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 == 1 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}
static CRC: [u32; 256] = crc_table();

fn crc32(d: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in d {
        c = CRC[((c ^ b as u32) & 255) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}
fn put32(p: &mut [u8], v: u32) {
    p[..4].copy_from_slice(&v.to_be_bytes());
}
fn get32(p: &[u8]) -> u32 {
    u32::from_be_bytes([p[0], p[1], p[2], p[3]])
}

struct Node {
    wal: Vec<u8>,
    len: usize,
    applied: usize,
    kv: [i64; KEYS],
}

impl Node {
    fn new() -> Self {
        Node { wal: vec![0; CAP], len: 0, applied: 0, kv: [0; KEYS] }
    }
    /// Apply WAL entries up to `upto` into the KV map, re-verifying each CRC.
    fn apply_upto(&mut self, upto: usize) {
        while self.applied < upto {
            let r = &self.wal[self.applied * REC..(self.applied + 1) * REC];
            if crc32(&r[..16]) == get32(&r[16..]) {
                self.kv[get32(&r[8..]) as usize % KEYS] = get32(&r[12..]) as i64;
            }
            self.applied += 1;
        }
    }
}

struct Rng(i64);
impl Rng {
    fn next(&mut self) -> i64 {
        self.0 = self.0 * 48271 % 2147483647;
        self.0
    }
}

fn main() {
    let mut rng = Rng(19019);
    let mut nodes = [Node::new(), Node::new(), Node::new()];
    let mut nxt = [1usize; 3];
    let mut mtch = [0usize; 3];
    let (mut commit, mut sent, mut dropped, mut crc_rejects, mut acks_lost) = (0usize, 0i64, 0i64, 0i64, 0i64);
    for t in 0..TICKS + DRAIN {
        if t < TICKS {
            let l = &mut nodes[0];
            let o = l.len * REC;
            put32(&mut l.wal[o..], (l.len + 1) as u32);
            put32(&mut l.wal[o + 4..], 1);
            put32(&mut l.wal[o + 8..], (rng.next() % KEYS as i64) as u32);
            put32(&mut l.wal[o + 12..], (rng.next() % 1_000_000) as u32);
            let c = crc32(&l.wal[o..o + 16]);
            put32(&mut l.wal[o + 16..], c);
            l.len += 1;
        }
        let leader_commit = commit;
        for f in 1..=2 {
            let n0 = nxt[f];
            if n0 > nodes[0].len {
                continue;
            }
            let cnt = (nodes[0].len - n0 + 1).min(BATCH);
            let r1 = rng.next() % 100;
            let r2 = rng.next() % 100;
            let r3 = rng.next() % 100;
            sent += 1;
            if r1 < DROP_PCT {
                dropped += 1;
                continue;
            }
            let mut msg = [0u8; BATCH * REC];
            msg[..cnt * REC].copy_from_slice(&nodes[0].wal[(n0 - 1) * REC..(n0 - 1) * REC + cnt * REC]);
            if r2 < CORRUPT_PCT {
                msg[(rng.next() % (cnt * REC) as i64) as usize] ^= 0x5A;
            }
            let fol = &mut nodes[f];
            for j in 0..cnt {
                let r = &msg[j * REC..(j + 1) * REC];
                if crc32(&r[..16]) != get32(&r[16..]) {
                    crc_rejects += 1;
                    break;
                }
                let idx = get32(r) as usize;
                if idx == fol.len + 1 {
                    let d = fol.len * REC;
                    fol.wal[d..d + REC].copy_from_slice(r);
                    fol.len += 1;
                } else if idx > fol.len + 1 {
                    break;
                }
            }
            fol.apply_upto(leader_commit.min(fol.len));
            if r3 < ACK_DROP_PCT {
                acks_lost += 1;
                continue;
            }
            mtch[f] = mtch[f].max(fol.len);
            nxt[f] = fol.len + 1;
        }
        let mut m = [nodes[0].len, mtch[1], mtch[2]];
        m.sort_unstable();
        commit = commit.max(m[1]);
        nodes[0].apply_upto(commit);
    }
    let mut cs = 0i64;
    let mut fold = |v: i64| cs = (cs * 131 + v) % MOD;
    for n in &nodes {
        fold(n.len as i64);
    }
    fold(commit as i64);
    for n in &nodes {
        fold(n.applied as i64);
    }
    for v in [sent, dropped, crc_rejects, acks_lost] {
        fold(v);
    }
    for n in &nodes {
        fold(n.kv.iter().fold(0i64, |d, &v| (d * 131 + v) % MOD));
    }
    for n in &nodes[..2] {
        fold(crc32(&n.wal[..n.len * REC]) as i64);
    }
    println!("CHECKSUM={cs}");
}
