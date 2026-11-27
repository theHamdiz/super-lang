//! 08_ecs — 2000 entities × 20 ticks, spatial-grid neighbour queries (SoA)
const N: usize = 2000;
const TICKS: usize = 20;
const CELL: f64 = 10.0;
const GRID: i32 = 64;

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

#[inline(always)]
fn cell_of(p: f64) -> i32 {
    ((p / CELL) as i32).clamp(0, GRID - 1)
}

fn main() {
    let mut rng = Lcg(15482888);
    let (mut px, mut py) = (vec![0f64; N], vec![0f64; N]);
    let (mut vx, mut vy) = (vec![0f64; N], vec![0f64; N]);
    let mut hp = vec![0i32; N];
    for i in 0..N {
        px[i] = rng.next_f() * 600.0;
        py[i] = rng.next_f() * 600.0;
        vx[i] = (rng.next_f() - 0.5) * 2.0;
        vy[i] = (rng.next_f() - 0.5) * 2.0;
        hp[i] = 10 + (rng.next_u() % 20) as i32;
    }
    let mut heads = vec![-1i32; (GRID * GRID) as usize];
    let mut next = vec![-1i32; N];

    for _ in 0..TICKS {
        for i in 0..N {
            px[i] += vx[i];
            py[i] += vy[i];
            if px[i] < 0.0 || px[i] > 600.0 {
                vx[i] = -vx[i];
            }
            if py[i] < 0.0 || py[i] > 600.0 {
                vy[i] = -vy[i];
            }
        }
        heads.fill(-1);
        for i in 0..N {
            let cell = (cell_of(py[i]) * GRID + cell_of(px[i])) as usize;
            next[i] = heads[cell];
            heads[cell] = i as i32;
        }
        for i in 0..N {
            let (cx, cy) = (cell_of(px[i]), cell_of(py[i]));
            let mut hits = 0;
            for ny in (cy - 1).max(0)..=(cy + 1).min(GRID - 1) {
                for nx in (cx - 1).max(0)..=(cx + 1).min(GRID - 1) {
                    let mut id = heads[(ny * GRID + nx) as usize];
                    while id >= 0 {
                        let o = id as usize;
                        if o != i {
                            let dx = (px[i] - px[o]) as f32;
                            let dy = (py[i] - py[o]) as f32;
                            if dx * dx + dy * dy < 4.0 {
                                hits += 1;
                            }
                        }
                        id = next[o];
                    }
                }
            }
            if hits > 3 {
                hp[i] -= 1;
            }
        }
    }
    let mut cs: i64 = 0;
    for i in 0..N {
        cs += hp[i] as i64;
        cs += (px[i] * 10.0).floor() as i64;
        cs += (py[i] * 10.0).floor() as i64;
    }
    println!("CHECKSUM={cs}");
}
