// 15_physics2d_broadphase_solver - 6k circles/boxes: uniform spatial-hash broadphase,
// circle/box manifolds, sequential-impulse solver (4 iterations x 10 steps).
// Mirrors std.game.physics2d (grid build, candidate order, narrowphase, resolve).
const COLS: usize = 100;
const ROWS: usize = 60;
const N_DYN: usize = COLS * ROWS;
const FLOOR_TILES: usize = 62;
const WALL_TILES: usize = 40;
const STEPS: usize = 10;
const ITERS: usize = 4;
const MOD: i64 = 1_000_000_007;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Circle,
    Box,
}

#[derive(Clone, Copy)]
struct Body {
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    inv_mass: f64,
    shape: Shape,
    r: f64,
    hw: f64,
    hh: f64,
    e: f64,
    mu: f64,
}

#[derive(Clone, Copy)]
struct Contact {
    nx: f64,
    ny: f64,
    depth: f64,
}

struct Rng(i64);
impl Rng {
    fn draw(&mut self, m: i64) -> i64 {
        self.0 = (self.0 * 1103515245 + 12345) % 2147483648;
        self.0 / 256 % m
    }
}

fn inv(mass: f64) -> f64 {
    if mass <= 0.0 { 0.0 } else { 1.0 / mass }
}
fn circle(x: f64, y: f64, r: f64, mass: f64, vx: f64, vy: f64) -> Body {
    Body { x, y, vx, vy, inv_mass: inv(mass), shape: Shape::Circle, r, hw: r, hh: r, e: 0.2, mu: 0.4 }
}
fn aabb(x: f64, y: f64, hw: f64, hh: f64, mass: f64, vx: f64, vy: f64) -> Body {
    Body { x, y, vx, vy, inv_mass: inv(mass), shape: Shape::Box, r: 0.0, hw, hh, e: 0.2, mu: 0.4 }
}

fn sign(v: f64) -> f64 {
    if v < 0.0 { -1.0 } else { 1.0 }
}
fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    if v < lo { lo } else if v > hi { hi } else { v }
}
fn pmax(a: f64, b: f64) -> f64 {
    if a > b { a } else { b }
}
fn pmin(a: f64, b: f64) -> f64 {
    if a < b { a } else { b }
}

fn circle_circle(a: &Body, b: &Body) -> Option<Contact> {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let rr = a.r + b.r;
    let d2 = dx * dx + dy * dy;
    if d2 >= rr * rr {
        return None;
    }
    if d2 == 0.0 {
        return Some(Contact { nx: 1.0, ny: 0.0, depth: rr });
    }
    let d = d2.sqrt();
    Some(Contact { nx: dx / d, ny: dy / d, depth: rr - d })
}

fn box_box(a: &Body, b: &Body) -> Option<Contact> {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let ox = a.hw + b.hw - dx.abs();
    let oy = a.hh + b.hh - dy.abs();
    if ox <= 0.0 || oy <= 0.0 {
        return None;
    }
    if ox < oy {
        return Some(Contact { nx: sign(dx), ny: 0.0, depth: ox });
    }
    Some(Contact { nx: 0.0, ny: sign(dy), depth: oy })
}

fn circle_box(a: &Body, b: &Body) -> Option<Contact> {
    let (lx, ly) = (a.x - b.x, a.y - b.y);
    let cx = clamp(lx, 0.0 - b.hw, b.hw);
    let cy = clamp(ly, 0.0 - b.hh, b.hh);
    let (fx, fy) = (lx - cx, ly - cy);
    let d2 = fx * fx + fy * fy;
    if d2 > 0.0 {
        if d2 >= a.r * a.r {
            return None;
        }
        let d = d2.sqrt();
        return Some(Contact { nx: (0.0 - fx) / d, ny: (0.0 - fy) / d, depth: a.r - d });
    }
    let (px, py) = (b.hw - lx.abs(), b.hh - ly.abs());
    if px < py {
        return Some(Contact { nx: 0.0 - sign(lx), ny: 0.0, depth: px + a.r });
    }
    Some(Contact { nx: 0.0, ny: 0.0 - sign(ly), depth: py + a.r })
}

fn collide(a: &Body, b: &Body) -> Option<Contact> {
    match (a.shape, b.shape) {
        (Shape::Circle, Shape::Circle) => circle_circle(a, b),
        (Shape::Box, Shape::Box) => box_box(a, b),
        (Shape::Circle, Shape::Box) => circle_box(a, b),
        (Shape::Box, Shape::Circle) => circle_box(b, a).map(|c| Contact { nx: 0.0 - c.nx, ny: 0.0 - c.ny, depth: c.depth }),
    }
}

fn resolve(a: &mut Body, b: &mut Body, c: Contact) {
    let (ia, ib) = (a.inv_mass, b.inv_mass);
    let isum = ia + ib;
    if isum == 0.0 {
        return;
    }
    let (rvx, rvy) = (b.vx - a.vx, b.vy - a.vy);
    let vn = rvx * c.nx + rvy * c.ny;
    if vn < 0.0 {
        let e = pmin(a.e, b.e);
        let j = (0.0 - (1.0 + e)) * vn / isum;
        a.vx = a.vx - j * ia * c.nx;
        a.vy = a.vy - j * ia * c.ny;
        b.vx = b.vx + j * ib * c.nx;
        b.vy = b.vy + j * ib * c.ny;
        let (mut tx, mut ty) = (rvx - vn * c.nx, rvy - vn * c.ny);
        let tl = (tx * tx + ty * ty).sqrt();
        if tl > 0.000001 {
            tx /= tl;
            ty /= tl;
            let mut jt = (0.0 - (rvx * tx + rvy * ty)) / isum;
            let mu = (a.mu * b.mu).sqrt();
            let lim = mu * j;
            jt = clamp(jt, 0.0 - lim, lim);
            a.vx = a.vx - jt * ia * tx;
            a.vy = a.vy - jt * ia * ty;
            b.vx = b.vx + jt * ib * tx;
            b.vy = b.vy + jt * ib * ty;
        }
    }
    let corr = pmax(c.depth - 0.01, 0.0) / isum * 0.8;
    a.x = a.x - corr * ia * c.nx;
    a.y = a.y - corr * ia * c.ny;
    b.x = b.x + corr * ib * c.nx;
    b.y = b.y + corr * ib * c.ny;
}

fn bucket(cx: i64, cy: i64, mask: i64) -> usize {
    (((cx * 73856093) ^ (cy * 19349663)) & mask) as usize
}

/// Uniform spatial hash: per-bucket entry chains (heads -> next), as in std.game.physics2d.
#[derive(Default)]
struct Grid {
    heads: Vec<i64>,
    eb: Vec<i64>,
    ex: Vec<i64>,
    ey: Vec<i64>,
    en: Vec<i64>,
    inv: f64,
    mask: i64,
}

impl Grid {
    fn build(&mut self, bodies: &[Body]) {
        let n = bodies.len();
        let (mut sum, mut big) = (0.0f64, 0.0f64);
        for b in bodies {
            let e = pmax(b.hw, b.hh);
            sum += e;
            if e > big {
                big = e;
            }
        }
        let mut cell = pmax(2.0 * sum / n as f64, big / 8.0);
        if cell < 0.000001 {
            cell = 1.0;
        }
        self.inv = 1.0 / cell;
        let pad = cell * 0.1;
        let mut size = 16usize;
        while size < n * 2 {
            size *= 2;
        }
        self.mask = size as i64 - 1;
        self.heads.clear();
        self.heads.resize(size, -1);
        self.eb.clear();
        self.ex.clear();
        self.ey.clear();
        self.en.clear();
        let inv = self.inv;
        for (i, b) in bodies.iter().enumerate() {
            let cx0 = ((b.x - b.hw - pad) * inv).floor() as i64;
            let cx1 = ((b.x + b.hw + pad) * inv).floor() as i64;
            let cy0 = ((b.y - b.hh - pad) * inv).floor() as i64;
            let cy1 = ((b.y + b.hh + pad) * inv).floor() as i64;
            for cy in cy0..cy1 + 1 {
                for cx in cx0..cx1 + 1 {
                    let slot = bucket(cx, cy, self.mask);
                    self.en.push(self.heads[slot]);
                    self.heads[slot] = self.eb.len() as i64;
                    self.eb.push(i as i64);
                    self.ex.push(cx);
                    self.ey.push(cy);
                }
            }
        }
    }

    /// bodies j > i whose registered cells overlap body i's padded AABB: ascending, unique
    fn candidates(&self, b: &Body, i: i64, mark: &mut [i64], out: &mut Vec<i64>) {
        out.clear();
        let pad = 0.1 / self.inv;
        let cx0 = ((b.x - b.hw - pad) * self.inv).floor() as i64;
        let cx1 = ((b.x + b.hw + pad) * self.inv).floor() as i64;
        let cy0 = ((b.y - b.hh - pad) * self.inv).floor() as i64;
        let cy1 = ((b.y + b.hh + pad) * self.inv).floor() as i64;
        for cy in cy0..cy1 + 1 {
            for cx in cx0..cx1 + 1 {
                let mut e = self.heads[bucket(cx, cy, self.mask)];
                while e >= 0 {
                    let eu = e as usize;
                    if self.ex[eu] == cx && self.ey[eu] == cy {
                        let j = self.eb[eu];
                        if j > i && mark[j as usize] != i + 1 {
                            mark[j as usize] = i + 1;
                            out.push(j);
                        }
                    }
                    e = self.en[eu];
                }
            }
        }
        out.sort_unstable();
    }
}

fn solve_pair(bodies: &mut [Body], i: usize, j: usize) -> i64 {
    let (lo, hi) = bodies.split_at_mut(j);
    let (a, b) = (&mut lo[i], &mut hi[0]);
    if a.inv_mass + b.inv_mass > 0.0
        && (b.x - a.x).abs() < a.hw + b.hw
        && (b.y - a.y).abs() < a.hh + b.hh
    {
        if let Some(c) = collide(a, b) {
            resolve(a, b, c);
            return 1;
        }
    }
    0
}

fn solve_grid(bodies: &mut [Body], g: &mut Grid, mark: &mut [i64], cands: &mut Vec<i64>) -> i64 {
    g.build(bodies);
    mark.fill(0);
    let mut last = 0;
    for i in 0..bodies.len() {
        let bi = bodies[i];
        g.candidates(&bi, i as i64, mark, cands);
        for k in 0..cands.len() {
            last += solve_pair(bodies, i, cands[k] as usize);
        }
    }
    last
}

fn posmod(v: i64) -> i64 {
    ((v % MOD) + MOD) % MOD
}

fn fl(v: f64) -> i64 {
    v.floor() as i64
}

fn main() {
    let mut bodies: Vec<Body> = Vec::with_capacity(FLOOR_TILES + 2 * WALL_TILES + N_DYN);
    for k in 0..FLOOR_TILES {
        bodies.push(aabb(1.0 + 2.0 * k as f64, -1.0, 1.0, 1.0, 0.0, 0.0, 0.0));
    }
    for k in 0..WALL_TILES {
        bodies.push(aabb(-1.0, 1.0 + 2.0 * k as f64, 1.0, 1.0, 0.0, 0.0, 0.0));
    }
    for k in 0..WALL_TILES {
        bodies.push(aabb(122.0, 1.0 + 2.0 * k as f64, 1.0, 1.0, 0.0, 0.0, 0.0));
    }
    let mut r = Rng(150015);
    for i in 0..N_DYN {
        let (col, row) = ((i % COLS) as f64, (i / COLS) as f64);
        let kind = r.draw(3);
        let sw = 0.35 + r.draw(400001) as f64 * 0.000001;
        let sh = 0.35 + r.draw(400001) as f64 * 0.000001;
        let mass = 1.0 + r.draw(4) as f64;
        let jx = (r.draw(600001) - 300000) as f64 * 0.000001;
        let jy = (r.draw(600001) - 300000) as f64 * 0.000001;
        let vx = (r.draw(8000001) - 4000000) as f64 * 0.000001;
        let vy = (r.draw(8000001) - 4000000) as f64 * 0.000001;
        let x = 0.75 + col * 1.2 + jx;
        let y = 1.75 + row * 1.2 + jy;
        bodies.push(if kind == 0 { aabb(x, y, sw, sh, mass, vx, vy) } else { circle(x, y, sw, mass, vx, vy) });
    }

    let n = bodies.len();
    let mut grid = Grid::default();
    let mut mark = vec![0i64; n];
    let mut cands: Vec<i64> = Vec::with_capacity(256);
    let (mut cs, dt) = (0i64, 1.0f64 / 60.0);
    let damp = 1.0 / (1.0 + dt * 0.0);
    for s in 0..STEPS {
        for b in bodies.iter_mut().filter(|b| b.inv_mass > 0.0) {
            b.vx = (b.vx + 0.0 * dt) * damp;
            b.vy = (b.vy + -9.81 * dt) * damp;
            b.x = b.x + b.vx * dt;
            b.y = b.y + b.vy * dt;
        }
        let mut last = 0;
        for _ in 0..ITERS {
            last = solve_grid(&mut bodies, &mut grid, &mut mark, &mut cands);
        }
        cs += last * (s as i64 + 1);
    }
    let (mut h, mut ke) = (0i64, 0.0f64);
    for b in &bodies {
        for v in [b.x, b.y, b.vx, b.vy] {
            h = (h * 131 + posmod(fl(v * 1000.0))) % MOD;
        }
        if b.inv_mass > 0.0 {
            ke += 0.5 * (b.vx * b.vx + b.vy * b.vy) / b.inv_mass;
        }
    }
    cs += h + fl(ke * 10.0);
    println!("CHECKSUM={cs}");
}
