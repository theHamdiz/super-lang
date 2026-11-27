// 12_sparse_fem_fft — 128x128 reaction-Poisson CSR, Jacobi-PCG x20, 2D FFT low-pass bank
use std::f64::consts::PI;

const G: usize = 128;
const N: usize = 16384;
const ITERS: usize = 20;

struct Csr {
    indptr: Vec<usize>,
    indices: Vec<usize>,
    values: Vec<f64>,
}

fn build_system(diag: &mut [f64]) -> Csr {
    let mut m = Csr { indptr: vec![0], indices: Vec::with_capacity(5 * N), values: Vec::with_capacity(5 * N) };
    for i in 0..N {
        let (row, col) = (i / G, i % G);
        let d = 4.0 + ((i * 37 % 11) as f64) * 0.25;
        diag[i] = d;
        if row > 0 { m.indices.push(i - G); m.values.push(-1.0); }
        if col > 0 { m.indices.push(i - 1); m.values.push(-1.0); }
        m.indices.push(i); m.values.push(d);
        if col < G - 1 { m.indices.push(i + 1); m.values.push(-1.0); }
        if row < G - 1 { m.indices.push(i + G); m.values.push(-1.0); }
        m.indptr.push(m.indices.len());
    }
    m
}

fn matvec(a: &Csr, x: &[f64], out: &mut [f64]) {
    for r in 0..N {
        let mut s = 0.0;
        for k in a.indptr[r]..a.indptr[r + 1] {
            s += a.values[k] * x[a.indices[k]];
        }
        out[r] = s;
    }
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    let mut s = 0.0;
    for i in 0..N {
        s += a[i] * b[i];
    }
    s
}

fn pcg(a: &Csr, diag: &[f64], b: &[f64], x: &mut [f64]) -> f64 {
    let mut r = b.to_vec();
    let mut z = vec![0.0; N];
    let mut p = vec![0.0; N];
    let mut ap = vec![0.0; N];
    for i in 0..N {
        x[i] = 0.0;
        z[i] = r[i] / diag[i];
        p[i] = z[i];
    }
    let mut rz = dot(&r, &z);
    for _ in 0..ITERS {
        matvec(a, &p, &mut ap);
        let alpha = rz / dot(&p, &ap);
        for i in 0..N {
            x[i] += alpha * p[i];
            r[i] -= alpha * ap[i];
            z[i] = r[i] / diag[i];
        }
        let rz_new = dot(&r, &z);
        let beta = rz_new / rz;
        for i in 0..N {
            p[i] = z[i] + beta * p[i];
        }
        rz = rz_new;
    }
    dot(&r, &r)
}

struct Fft {
    bitrev: [usize; G],
    tw: Vec<(Vec<f64>, Vec<f64>)>,
}

impl Fft {
    fn new() -> Self {
        let mut bitrev = [0usize; G];
        for (i, slot) in bitrev.iter_mut().enumerate() {
            let (mut r, mut v) = (0usize, i);
            for _ in 0..7 {
                r = (r << 1) | (v & 1);
                v >>= 1;
            }
            *slot = r;
        }
        let mut tw = Vec::new();
        let mut span = 2;
        while span <= G {
            let ang = 0.0 - 2.0 * PI / span as f64;
            tw.push((
                (0..span / 2).map(|k| (ang * k as f64).cos()).collect(),
                (0..span / 2).map(|k| (ang * k as f64).sin()).collect(),
            ));
            span *= 2;
        }
        Fft { bitrev, tw }
    }

    fn fft1(&self, re: &mut [f64; G], im: &mut [f64; G], inv: bool) {
        let (mut tre, mut tim) = ([0.0f64; G], [0.0f64; G]);
        if inv {
            for v in im.iter_mut() {
                *v = 0.0 - *v;
            }
        }
        for i in 0..G {
            let s = self.bitrev[i];
            tre[i] = re[s];
            tim[i] = im[s];
        }
        let (mut span, mut lvl) = (2usize, 0usize);
        while span <= G {
            let half = span / 2;
            let (wrt, wit) = &self.tw[lvl];
            let mut start = 0;
            while start < G {
                for k in 0..half {
                    let (wr, wi) = (wrt[k], wit[k]);
                    let a = start + k;
                    let b = a + half;
                    let tr = tre[b] * wr - tim[b] * wi;
                    let ti = tre[b] * wi + tim[b] * wr;
                    tre[b] = tre[a] - tr;
                    tim[b] = tim[a] - ti;
                    tre[a] = tre[a] + tr;
                    tim[a] = tim[a] + ti;
                }
                start += span;
            }
            span *= 2;
            lvl += 1;
        }
        if inv {
            let s = 1.0 / G as f64;
            for i in 0..G {
                re[i] = tre[i] * s;
                im[i] = (0.0 - tim[i]) * s;
            }
        } else {
            *re = tre;
            *im = tim;
        }
    }

    fn fft2(&self, re: &mut [f64], im: &mut [f64], inv: bool) {
        let (mut lr, mut li) = ([0.0f64; G], [0.0f64; G]);
        for r in 0..G {
            for c in 0..G {
                lr[c] = re[r * G + c];
                li[c] = im[r * G + c];
            }
            self.fft1(&mut lr, &mut li, inv);
            for c in 0..G {
                re[r * G + c] = lr[c];
                im[r * G + c] = li[c];
            }
        }
        for c in 0..G {
            for r in 0..G {
                lr[r] = re[r * G + c];
                li[r] = im[r * G + c];
            }
            self.fft1(&mut lr, &mut li, inv);
            for r in 0..G {
                re[r * G + c] = lr[r];
                im[r * G + c] = li[r];
            }
        }
    }
}

fn main() {
    let b: Vec<f64> = (0..N).map(|i| (((i * 7 + (i / G) * 13) % 17) as f64 - 8.0) / 8.0).collect();
    let mut diag = vec![0.0; N];
    let mut x = vec![0.0; N];
    let a = build_system(&mut diag);
    let res = pcg(&a, &diag, &b, &mut x).sqrt();

    let fft = Fft::new();
    let mut sre = x.clone();
    let mut sim = vec![0.0; N];
    fft.fft2(&mut sre, &mut sim, false);

    let cuts = [64usize, 256, 1024];
    let (mut energy, mut corner) = (0.0f64, 0.0f64);
    for cut in cuts {
        let mut fre = sre.clone();
        let mut fim = sim.clone();
        for k in 0..N {
            let (ky, kx) = (k / G, k % G);
            let fy = if ky <= G / 2 { ky } else { G - ky };
            let fx = if kx <= G / 2 { kx } else { G - kx };
            if fx * fx + fy * fy > cut {
                fre[k] = 0.0;
                fim[k] = 0.0;
            }
        }
        fft.fft2(&mut fre, &mut fim, true);
        for k in 0..N {
            energy += fre[k] * fre[k];
        }
        corner += fre[0] + fre[N / 2 + G / 2] + fre[N - 1];
    }

    let xs = dot(&x, &x);
    let cs = (xs.sqrt() * 1000000.0).floor() as i64
        + (energy * 1000.0).floor() as i64
        + (corner * 1000000.0).floor() as i64
        + (res * 1000000000.0).floor() as i64;
    println!("CHECKSUM={cs}");
}
