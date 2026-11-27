// 16_crypto_pq_argon2_suite — Argon2id-like KDF (m=8KiB t=2 p=1) x32, SHA-256 Merkle of tags,
// toy lattice negacyclic NTT poly-mul (q=12289, n=256)
// Build: rustc -C opt-level=3 -C target-cpu=native algo.rs

const MOD: u64 = 1_000_000_007;
const NH: usize = 32;
const BW: usize = 64;
const NB: usize = 32;
const PASSES: usize = 2;
const Q: u64 = 12289;
const N: usize = 256;
const PAIRS: usize = 6;
const M32: u64 = 0xFFFF_FFFF;

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0 * 48271 % 2147483647;
        self.0
    }
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

type Digest = [u8; 32];

fn sha256(msg: &[u8]) -> Digest {
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut total = msg.len() + 1;
    while total % 64 != 56 {
        total += 1;
    }
    total += 8;
    let mut buf = vec![0u8; total];
    buf[..msg.len()].copy_from_slice(msg);
    buf[msg.len()] = 0x80;
    buf[total - 8..].copy_from_slice(&((msg.len() as u64) * 8).to_be_bytes());
    for chunk in buf.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let t1 = hh
                .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                .wrapping_add((e & f) ^ (!e & g))
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22)).wrapping_add((a & b) ^ (a & c) ^ (b & c));
            hh = g; g = f; f = e; e = d.wrapping_add(t1); d = c; c = b; b = a; a = t1.wrapping_add(t2);
        }
        for (s, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *s = s.wrapping_add(v);
        }
    }
    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

#[inline]
fn rot32(x: u64, r: u32) -> u64 {
    ((x >> r) | (x << (32 - r))) & M32
}
#[inline]
fn blamka(x: u64, y: u64) -> u64 {
    (x + y + 2 * (x & 0xFFFF) * (y & 0xFFFF)) & M32
}

fn g_mix(z: &mut [u64; BW], ia: usize, ib: usize, ic: usize, id: usize) {
    let (mut a, mut b, mut c, mut d) = (z[ia], z[ib], z[ic], z[id]);
    a = blamka(a, b); d = rot32(d ^ a, 16);
    c = blamka(c, d); b = rot32(b ^ c, 12);
    a = blamka(a, b); d = rot32(d ^ a, 8);
    c = blamka(c, d); b = rot32(b ^ c, 7);
    z[ia] = a; z[ib] = b; z[ic] = c; z[id] = d;
}

fn p_round(z: &mut [u64; BW], base: usize, s: usize) {
    g_mix(z, base, base + 2 * s, base + 4 * s, base + 6 * s);
    g_mix(z, base + s, base + 3 * s, base + 5 * s, base + 7 * s);
    g_mix(z, base, base + 3 * s, base + 4 * s, base + 7 * s);
    g_mix(z, base + s, base + 2 * s, base + 5 * s, base + 6 * s);
}

fn compress(x: &[u64; BW], y: &[u64; BW], out: &mut [u64; BW]) {
    let mut r = [0u64; BW];
    for i in 0..BW {
        r[i] = x[i] ^ y[i];
    }
    *out = r;
    for row in 0..8 {
        p_round(out, row * 8, 1);
    }
    for col in 0..8 {
        p_round(out, col, 8);
    }
    for i in 0..BW {
        out[i] ^= r[i];
    }
}

fn init_block(h0: &Digest, j: u8) -> [u64; BW] {
    let mut msg = [0u8; 33];
    msg[..32].copy_from_slice(h0);
    msg[32] = j;
    let d = sha256(&msg);
    let mut w = [0u64; BW];
    for k in 0..8 {
        w[k] = u32::from_be_bytes([d[4 * k], d[4 * k + 1], d[4 * k + 2], d[4 * k + 3]]) as u64;
    }
    for k in 8..BW {
        w[k] = (w[k - 8] + (w[k - 3] & 0xFFFF) * 40503 + (w[k - 1] >> 16) * 69069) & M32;
    }
    w
}

fn argon2_like(idx_pw: usize, rng: &mut Lcg) -> Digest {
    let mut msg = [0u8; 36];
    msg[0] = idx_pw as u8;
    msg[1] = (NB * 256 / 1024) as u8;
    msg[2] = PASSES as u8;
    msg[3] = 1;
    for b in msg[4..].iter_mut() {
        *b = (rng.next() & 255) as u8;
    }
    let h0 = sha256(&msg);
    let mut b = [[0u64; BW]; NB];
    b[0] = init_block(&h0, 0);
    b[1] = init_block(&h0, 1);
    let mut nw = [0u64; BW];
    for pass in 0..PASSES {
        for idx in (if pass == 0 { 2 } else { 0 })..NB {
            let prev = b[(idx + NB - 1) % NB];
            let r = if pass == 0 && idx < NB / 2 {
                (((pass * 131 + idx) as u64 * 40503 + 12345) * 69069) % 4294967296
            } else {
                prev[0]
            };
            let r16 = r >> 16;
            let x = (r16 * r16) >> 16;
            let rf = if pass == 0 {
                let area = (idx - 1) as u64;
                (area - 1 - ((area * x) >> 16)) as usize
            } else {
                let area = (NB - 2) as u64;
                (idx as u64 + 1 + (area - 1 - ((area * x) >> 16))) as usize % NB
            };
            let other = b[rf];
            compress(&prev, &other, &mut nw);
            if pass == 0 {
                b[idx] = nw;
            } else {
                for i in 0..BW {
                    b[idx][i] ^= nw[i];
                }
            }
        }
    }
    let mut bytes = [0u8; BW * 4];
    for (i, w) in b[NB - 1].iter().enumerate() {
        bytes[4 * i..4 * i + 4].copy_from_slice(&(*w as u32).to_be_bytes());
    }
    sha256(&bytes)
}

fn pow_mod(mut b: u64, mut e: u64) -> u64 {
    let mut r = 1;
    b %= Q;
    while e > 0 {
        if e & 1 == 1 {
            r = r * b % Q;
        }
        b = b * b % Q;
        e >>= 1;
    }
    r
}

fn ntt(a: &mut [u64; N], w: u64) {
    let mut j = 0usize;
    for i in 1..N {
        let mut bit = N >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            a.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= N {
        let wl = pow_mod(w, (N / len) as u64);
        let half = len / 2;
        for i in (0..N).step_by(len) {
            let mut wn = 1u64;
            for k in 0..half {
                let u = a[i + k];
                let v = a[i + k + half] * wn % Q;
                a[i + k] = (u + v) % Q;
                a[i + k + half] = (u + Q - v) % Q;
                wn = wn * wl % Q;
            }
        }
        len <<= 1;
    }
}

fn polymul(a0: &[u64; N], b0: &[u64; N], psi: u64) -> [u64; N] {
    let psi_inv = pow_mod(psi, Q - 2);
    let omega = psi * psi % Q;
    let n_inv = pow_mod(N as u64, Q - 2);
    let (mut a, mut b, mut c) = ([0u64; N], [0u64; N], [0u64; N]);
    let mut p = 1u64;
    for i in 0..N {
        a[i] = a0[i] * p % Q;
        b[i] = b0[i] * p % Q;
        p = p * psi % Q;
    }
    ntt(&mut a, omega);
    ntt(&mut b, omega);
    for i in 0..N {
        c[i] = a[i] * b[i] % Q;
    }
    ntt(&mut c, pow_mod(omega, Q - 2));
    p = n_inv;
    for i in 0..N {
        c[i] = c[i] * p % Q;
        p = p * psi_inv % Q;
    }
    c
}

fn schoolbook(a: &[u64; N], b: &[u64; N]) -> [u64; N] {
    let mut c = [0u64; N];
    for i in 0..N {
        for j in 0..N {
            let pr = a[i] * b[j] % Q;
            let k = i + j;
            if k >= N {
                c[k - N] = (c[k - N] + Q - pr) % Q;
            } else {
                c[k] = (c[k] + pr) % Q;
            }
        }
    }
    c
}

fn main() {
    let mut rng = Lcg(16001);
    let mut level: Vec<Digest> = (0..NH).map(|i| argon2_like(i, &mut rng)).collect();
    while level.len() > 1 {
        let n = level.len();
        level = (0..n.div_ceil(2))
            .map(|i| {
                let mut cat = [0u8; 64];
                cat[..32].copy_from_slice(&level[2 * i]);
                cat[32..].copy_from_slice(&level[if 2 * i + 1 < n { 2 * i + 1 } else { 2 * i }]);
                sha256(&cat)
            })
            .collect();
    }
    let mut cs: u64 = 0;
    for byte in level[0] {
        cs = (cs * 131 + byte as u64) % MOD;
    }
    let psi = pow_mod(11, (Q - 1) / (2 * N as u64));
    let mut ok = u64::from(pow_mod(psi, N as u64) == Q - 1);
    for pr in 0..PAIRS {
        let mut a = [0u64; N];
        let mut b = [0u64; N];
        for v in a.iter_mut() {
            *v = rng.next() % Q;
        }
        for v in b.iter_mut() {
            *v = rng.next() % Q;
        }
        let c = polymul(&a, &b, psi);
        if pr < 2 && schoolbook(&a, &b) != c {
            ok = 0;
        }
        for v in c {
            cs = (cs * 131 + v) % MOD;
        }
    }
    cs = (cs * 131 + ok) % MOD;
    println!("CHECKSUM={cs}");
}
