//! 01_nbody — 64 bodies × 500 steps, soft gravity, SoA
const N: usize = 64;
const STEPS: usize = 500;
const DT: f64 = 0.001;
const SOFT: f64 = 0.01;

struct Lcg(u64);

impl Lcg {
    #[inline(always)]
    fn next_u(&mut self) -> u32 {
        self.0 = (self.0.wrapping_mul(1103515245).wrapping_add(12345)) % 2147483648;
        self.0 as u32
    }
    #[inline(always)]
    fn next_f(&mut self) -> f64 {
        self.next_u() as f64 / 2147483648.0
    }
}

fn main() {
    let mut rng = Lcg(12648430);
    let (mut x, mut y, mut vx, mut vy, mut m) = ([0f64; N], [0f64; N], [0f64; N], [0f64; N], [0f64; N]);
    for i in 0..N {
        x[i] = rng.next_f() * 2.0 - 1.0;
        y[i] = rng.next_f() * 2.0 - 1.0;
        vx[i] = (rng.next_f() - 0.5) * 0.01;
        vy[i] = (rng.next_f() - 0.5) * 0.01;
        m[i] = 0.5 + rng.next_f();
    }
    for _ in 0..STEPS {
        for i in 0..N {
            let (xi, yi) = (x[i], y[i]);
            let (mut ax, mut ay) = (0.0f64, 0.0f64);
            for j in 0..N {
                if i == j {
                    continue;
                }
                let dx = x[j] - xi;
                let dy = y[j] - yi;
                let d2 = dx * dx + dy * dy + SOFT;
                let inv = 1.0 / d2.sqrt();
                let inv3 = inv * inv * inv;
                ax += m[j] * dx * inv3;
                ay += m[j] * dy * inv3;
            }
            vx[i] += ax * DT;
            vy[i] += ay * DT;
        }
        for i in 0..N {
            x[i] += vx[i] * DT;
            y[i] += vy[i] * DT;
        }
    }
    let mut cs: i64 = 0;
    for i in 0..N {
        cs += (x[i] * 1000.0).floor() as i64;
        cs += (y[i] * 1000.0).floor() as i64;
        cs += (vx[i] * 1e6).floor() as i64;
    }
    println!("CHECKSUM={cs}");
}
