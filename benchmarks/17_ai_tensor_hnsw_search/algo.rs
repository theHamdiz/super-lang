// 17_ai_tensor_hnsw_search — fixed-point (Q10) transformer blocks (causal attention, softmax,
// ReLU FFN, RMS norm) on 12 sequences, then brute-force top-10 cosine search over 4096 unit
// vectors of dim 32 using the pooled embeddings as queries. Integer-only => bit-exact everywhere.
// Build: rustc -C opt-level=3 -C target-cpu=native algo.rs

const MOD: i64 = 1_000_000_007;
const BIAS: i64 = 1 << 40;
const D: usize = 32;
const T: usize = 16;
const FF: usize = 64;
const LAYERS: usize = 2;
const SEQS: usize = 12;
const NV: usize = 4096;
const K: usize = 10;

struct Lcg(i64);
impl Lcg {
    fn next(&mut self) -> i64 {
        self.0 = self.0 * 48271 % 2147483647;
        self.0
    }
}

fn isqrt(n: i64) -> i64 {
    if n == 0 {
        return 0;
    }
    let (mut x, mut y) = (n, (n + 1) >> 1);
    while y < x {
        x = y;
        y = (x + n / x) >> 1;
    }
    x
}

fn exp_frac(f: i64) -> i64 {
    let (mut term, mut sum) = (1024i64, 1024i64);
    for k in 1..=6 {
        term = ((term * f) >> 10) / k;
        if k & 1 == 1 { sum -= term } else { sum += term }
    }
    sum.max(0)
}

fn exp_q(x: i64, e1: i64) -> i64 {
    let a = -x;
    let n = a >> 10;
    if n >= 16 {
        return 0;
    }
    let mut r = exp_frac(a & 1023);
    for _ in 0..n {
        r = (r * e1) >> 10;
    }
    r
}

fn matmul(x: &[i64], w: &[i64], y: &mut [i64], din: usize, dout: usize) {
    for t in 0..T {
        for o in 0..dout {
            let mut s = 0i64;
            for i in 0..din {
                s += x[t * din + i] * w[i * dout + o];
            }
            y[t * dout + o] = s >> 10;
        }
    }
}

fn rms_norm(h: &mut [i64]) {
    for t in 0..T {
        let ss: i64 = h[t * D..(t + 1) * D].iter().map(|v| v * v).sum();
        let rms = isqrt(ss / D as i64);
        let inv = (1i64 << 20) / (rms + 1);
        for v in &mut h[t * D..(t + 1) * D] {
            *v = (*v * inv) >> 10;
        }
    }
}

struct Layer {
    wq: Vec<i64>,
    wk: Vec<i64>,
    wv: Vec<i64>,
    wo: Vec<i64>,
    w1: Vec<i64>,
    w2: Vec<i64>,
}

fn fill_w(rng: &mut Lcg, n: usize) -> Vec<i64> {
    (0..n).map(|_| rng.next() % 257 - 128).collect()
}

fn block(ly: &Layer, h: &mut [i64], e1: i64) {
    let (mut q, mut k, mut v) = (vec![0i64; T * D], vec![0i64; T * D], vec![0i64; T * D]);
    let (mut at, mut o, mut f, mut g) = (vec![0i64; T * D], vec![0i64; T * D], vec![0i64; T * FF], vec![0i64; T * D]);
    matmul(h, &ly.wq, &mut q, D, D);
    matmul(h, &ly.wk, &mut k, D, D);
    matmul(h, &ly.wv, &mut v, D, D);
    let (mut sc, mut pr) = ([0i64; T], [0i64; T]);
    for t in 0..T {
        let mut mx = i64::MIN;
        let mut sum = 0i64;
        for j in 0..=t {
            let mut s = 0i64;
            for d in 0..D {
                s += q[t * D + d] * k[j * D + d];
            }
            sc[j] = s >> 13;
            mx = mx.max(sc[j]);
        }
        for j in 0..=t {
            pr[j] = exp_q(sc[j] - mx, e1);
            sum += pr[j];
        }
        for j in 0..=t {
            pr[j] = pr[j] * 1024 / sum;
        }
        for d in 0..D {
            let mut s = 0i64;
            for j in 0..=t {
                s += pr[j] * v[j * D + d];
            }
            at[t * D + d] = s >> 10;
        }
    }
    matmul(&at, &ly.wo, &mut o, D, D);
    for i in 0..T * D {
        h[i] += o[i];
    }
    rms_norm(h);
    matmul(h, &ly.w1, &mut f, D, FF);
    for x in f.iter_mut() {
        *x = (*x).max(0);
    }
    matmul(&f, &ly.w2, &mut g, FF, D);
    for i in 0..T * D {
        h[i] += g[i];
    }
    rms_norm(h);
}

fn normalize(raw: &[i64]) -> [i64; D] {
    let ss: i64 = raw.iter().map(|v| v * v).sum();
    let inv = (1i64 << 20) / (isqrt(ss) + 1);
    let mut out = [0i64; D];
    for d in 0..D {
        out[d] = (raw[d] * inv) >> 10;
    }
    out
}

fn main() {
    let mut rng = Lcg(17017);
    let e1 = exp_frac(1024);
    let layers: Vec<Layer> = (0..LAYERS)
        .map(|_| Layer {
            wq: fill_w(&mut rng, D * D),
            wk: fill_w(&mut rng, D * D),
            wv: fill_w(&mut rng, D * D),
            wo: fill_w(&mut rng, D * D),
            w1: fill_w(&mut rng, D * FF),
            w2: fill_w(&mut rng, FF * D),
        })
        .collect();
    let db: Vec<[i64; D]> = (0..NV)
        .map(|_| {
            let raw: Vec<i64> = (0..D).map(|_| rng.next() % 2049 - 1024).collect();
            normalize(&raw)
        })
        .collect();
    let mut cs = 0i64;
    let mut fold = |v: i64| cs = (cs * 131 + v + BIAS) % MOD;
    for _ in 0..SEQS {
        let mut h: Vec<i64> = (0..T * D).map(|_| rng.next() % 2049 - 1024).collect();
        for ly in &layers {
            block(ly, &mut h, e1);
        }
        let mut pooled = [0i64; D];
        for d in 0..D {
            let sum: i64 = (0..T).map(|t| h[t * D + d]).sum();
            pooled[d] = sum >> 4;
            fold(pooled[d]);
        }
        let query = normalize(&pooled);
        let (mut top_s, mut top_i) = ([0i64; K], [0i64; K]);
        let mut cnt = 0usize;
        for (i, row) in db.iter().enumerate() {
            let sc: i64 = query.iter().zip(row).map(|(a, b)| a * b).sum();
            if cnt < K || sc > top_s[K - 1] {
                let mut p = if cnt < K { cnt } else { K - 1 };
                while p > 0 && top_s[p - 1] < sc {
                    top_s[p] = top_s[p - 1];
                    top_i[p] = top_i[p - 1];
                    p -= 1;
                }
                top_s[p] = sc;
                top_i[p] = i as i64;
                if cnt < K {
                    cnt += 1;
                }
            }
        }
        for j in 0..K {
            fold(top_i[j]);
            fold(top_s[j]);
        }
    }
    println!("CHECKSUM={cs}");
}
