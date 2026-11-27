// 11_symbolic_autodiff_ode — 20k random expressions: simplify + forward dual AD,
// then dual-RK4 sensitivities of a stiff-ish ODE
const EXPRS: usize = 20000;
const POINTS: i64 = 7;
const TRAJ: usize = 1024;
const STEPS: usize = 1000;
const MAXN: usize = 64;
const LEAF: [f64; 5] = [0.0, 1.0, 2.0, 0.5, -1.0];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Const,
    Var,
    Add,
    Sub,
    Mul,
    Neg,
    Sin,
}

#[derive(Clone, Copy)]
struct Dual {
    v: f64,
    d: f64,
}

impl Dual {
    fn add(self, o: Dual) -> Dual { Dual { v: self.v + o.v, d: self.d + o.d } }
    fn sub(self, o: Dual) -> Dual { Dual { v: self.v - o.v, d: self.d - o.d } }
    fn mul(self, o: Dual) -> Dual { Dual { v: self.v * o.v, d: self.v * o.d + self.d * o.v } }
    fn neg(self) -> Dual { Dual { v: -self.v, d: -self.d } }
    fn scale(self, k: f64) -> Dual { Dual { v: self.v * k, d: self.d * k } }
    fn shift(self, k: f64) -> Dual { Dual { v: self.v + k, d: self.d } }
    fn sin(self) -> Dual { Dual { v: self.v.sin(), d: self.v.cos() * self.d } }
}

struct Tree {
    op: [Op; MAXN],
    a: [usize; MAXN],
    b: [usize; MAXN],
    cv: [f64; MAXN],
    cnt: usize,
}

impl Tree {
    fn new() -> Self {
        Tree { op: [Op::Const; MAXN], a: [0; MAXN], b: [0; MAXN], cv: [0.0; MAXN], cnt: 0 }
    }

    fn push(&mut self, kind: Op, a: usize, b: usize, c: f64) -> usize {
        let n = self.cnt;
        self.cnt += 1;
        self.op[n] = kind;
        self.a[n] = a;
        self.b[n] = b;
        self.cv[n] = c;
        n
    }

    fn is_const(&self, i: usize, c: f64) -> bool {
        self.op[i] == Op::Const && self.cv[i] == c
    }
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = (self.0 * 1103515245 + 12345) % 2147483648;
        self.0
    }
}

fn gen(t: &mut Tree, rng: &mut Lcg, depth: u32) -> usize {
    let k = rng.next() / 256 % 10;
    if k < 2 || depth == 0 {
        let m = (rng.next() / 256 % 8) as usize;
        return if m < 3 { t.push(Op::Var, 0, 0, 0.0) } else { t.push(Op::Const, 0, 0, LEAF[m - 3]) };
    }
    let a = gen(t, rng, depth - 1);
    let kind = match k {
        0..=3 => Op::Add,
        4 => Op::Sub,
        5..=7 => Op::Mul,
        8 => Op::Neg,
        _ => Op::Sin,
    };
    let b = if matches!(kind, Op::Add | Op::Sub | Op::Mul) { gen(t, rng, depth - 1) } else { 0 };
    t.push(kind, a, b, 0.0)
}

// Algebraic rewrite of one node given the images `l`, `r` of its children.
fn simp_node(s: &mut Tree, kind: Op, cv: f64, l: usize, r: usize) -> usize {
    match kind {
        Op::Const => s.push(Op::Const, 0, 0, cv),
        Op::Var => s.push(Op::Var, 0, 0, 0.0),
        Op::Add => {
            if s.is_const(l, 0.0) { return r; }
            if s.is_const(r, 0.0) { return l; }
            if s.op[l] == Op::Const && s.op[r] == Op::Const { return s.push(Op::Const, 0, 0, s.cv[l] + s.cv[r]); }
            s.push(Op::Add, l, r, 0.0)
        }
        Op::Sub => {
            if s.is_const(r, 0.0) { return l; }
            if s.op[l] == Op::Const && s.op[r] == Op::Const { return s.push(Op::Const, 0, 0, s.cv[l] - s.cv[r]); }
            if l == r { return s.push(Op::Const, 0, 0, 0.0); }
            s.push(Op::Sub, l, r, 0.0)
        }
        Op::Mul => {
            if s.is_const(l, 0.0) || s.is_const(r, 0.0) { return s.push(Op::Const, 0, 0, 0.0); }
            if s.is_const(l, 1.0) { return r; }
            if s.is_const(r, 1.0) { return l; }
            if s.op[l] == Op::Const && s.op[r] == Op::Const { return s.push(Op::Const, 0, 0, s.cv[l] * s.cv[r]); }
            s.push(Op::Mul, l, r, 0.0)
        }
        Op::Neg => {
            if s.op[l] == Op::Const { return s.push(Op::Const, 0, 0, -s.cv[l]); }
            if s.op[l] == Op::Neg { return s.a[l]; }
            s.push(Op::Neg, l, 0, 0.0)
        }
        Op::Sin => {
            if s.is_const(l, 0.0) { return s.push(Op::Const, 0, 0, 0.0); }
            s.push(Op::Sin, l, 0, 0.0)
        }
    }
}

fn simplify(t: &Tree, s: &mut Tree, rep: &mut [usize; MAXN]) {
    s.cnt = 0;
    for i in 0..t.cnt {
        rep[i] = simp_node(s, t.op[i], t.cv[i], rep[t.a[i]], rep[t.b[i]]);
    }
}

fn eval_dual(t: &Tree, x: f64, ev: &mut [f64; MAXN], ed: &mut [f64; MAXN]) {
    for i in 0..t.cnt {
        let a = Dual { v: ev[t.a[i]], d: ed[t.a[i]] };
        let b = Dual { v: ev[t.b[i]], d: ed[t.b[i]] };
        let r = match t.op[i] {
            Op::Const => Dual { v: t.cv[i], d: 0.0 },
            Op::Var => Dual { v: x, d: 1.0 },
            Op::Add => a.add(b),
            Op::Sub => a.sub(b),
            Op::Mul => a.mul(b),
            Op::Neg => a.neg(),
            Op::Sin => a.sin(),
        };
        ev[i] = r.v;
        ed[i] = r.d;
    }
}

#[inline]
fn rhs(t: f64, y: Dual, lam: Dual) -> Dual {
    let (s, c) = (t.sin(), t.cos());
    lam.mul(y.sub(Dual { v: s, d: 0.0 })).neg().shift(0.5 * c)
}

fn rk4_final(lam: f64, y0: f64, h: f64, steps: usize) -> Dual {
    let ld = Dual { v: lam, d: 1.0 };
    let mut y = Dual { v: y0, d: 0.0 };
    let (hh, h6) = (0.5 * h, h / 6.0);
    for k in 0..steps {
        let t = k as f64 * h;
        let k1 = rhs(t, y, ld);
        let k2 = rhs(t + hh, y.add(k1.scale(hh)), ld);
        let k3 = rhs(t + hh, y.add(k2.scale(hh)), ld);
        let k4 = rhs(t + h, y.add(k3.scale(h)), ld);
        y = y.add(k1.add(k2.scale(2.0)).add(k3.scale(2.0)).add(k4).scale(h6));
    }
    y
}

fn main() {
    let mut rng = Lcg(20240611);
    let (mut t, mut s) = (Tree::new(), Tree::new());
    let mut rep = [0usize; MAXN];
    let (mut ev, mut ed, mut sev, mut sed) = ([0.0; MAXN], [0.0; MAXN], [0.0; MAXN], [0.0; MAXN]);
    let (mut orig_nodes, mut simp_nodes, mut mismatch) = (0i64, 0i64, 0i64);
    let (mut acc_v, mut acc_d) = (0.0f64, 0.0f64);
    for _ in 0..EXPRS {
        t.cnt = 0;
        gen(&mut t, &mut rng, 4);
        let n = t.cnt;
        simplify(&t, &mut s, &mut rep);
        let m = s.cnt;
        orig_nodes += n as i64;
        simp_nodes += m as i64;
        for p in 0..POINTS {
            let x = (p - 3) as f64 / 4.0;
            eval_dual(&t, x, &mut ev, &mut ed);
            eval_dual(&s, x, &mut sev, &mut sed);
            let (vo, dro, vs, ds) = (ev[n - 1], ed[n - 1], sev[rep[n - 1]], sed[rep[n - 1]]);
            if (vo - vs).abs() > 0.000000001 * (1.0 + vo.abs()) { mismatch += 1; }
            if (dro - ds).abs() > 0.000000001 * (1.0 + dro.abs()) { mismatch += 1; }
            acc_v += vs;
            acc_d += ds;
        }
    }

    let (mut sum_y, mut sum_dy) = (0.0f64, 0.0f64);
    for j in 0..TRAJ {
        let r = rk4_final(10.0 + j as f64 * 0.25, 1.0, 0.002, STEPS);
        sum_y += r.v;
        sum_dy += r.d;
    }

    let q = (acc_v * 10000.0).floor() as i64;
    let qd = (acc_d * 10000.0).floor() as i64;
    let y1 = (sum_y * 1000000.0).floor() as i64;
    let y2 = (sum_dy * 1000000000.0).floor() as i64;
    let cs = orig_nodes + simp_nodes * 7 + mismatch * 13 + q * 3 + qd * 5 + y1 * 11 + y2 * 17;
    println!("CHECKSUM={cs}");
}
