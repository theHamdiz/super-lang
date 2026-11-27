// 11_symbolic_autodiff_ode — 20k random expressions: simplify + forward dual AD,
// then dual-RK4 sensitivities of a stiff-ish ODE
const std = @import("std");

const Op = enum(u8) { konst, variable, add, sub, mul, neg, sin };
const EXPRS = 20000;
const POINTS = 7;
const TRAJ = 1024;
const STEPS = 1000;
const MAXN = 64;
const LEAF = [5]f64{ 0.0, 1.0, 2.0, 0.5, -1.0 };

const Dual = struct {
    v: f64,
    d: f64,
    fn add(a: Dual, b: Dual) Dual { return .{ .v = a.v + b.v, .d = a.d + b.d }; }
    fn sub(a: Dual, b: Dual) Dual { return .{ .v = a.v - b.v, .d = a.d - b.d }; }
    fn mul(a: Dual, b: Dual) Dual { return .{ .v = a.v * b.v, .d = a.v * b.d + a.d * b.v }; }
    fn neg(a: Dual) Dual { return .{ .v = -a.v, .d = -a.d }; }
    fn scale(a: Dual, k: f64) Dual { return .{ .v = a.v * k, .d = a.d * k }; }
    fn shift(a: Dual, k: f64) Dual { return .{ .v = a.v + k, .d = a.d }; }
    fn sin(a: Dual) Dual { return .{ .v = @sin(a.v), .d = @cos(a.v) * a.d }; }
};

const Tree = struct {
    op: [MAXN]Op = @splat(.konst),
    a: [MAXN]usize = @splat(0),
    b: [MAXN]usize = @splat(0),
    cv: [MAXN]f64 = @splat(0.0),
    cnt: usize = 0,

    fn push(self: *Tree, kind: Op, a: usize, b: usize, c: f64) usize {
        const n = self.cnt;
        self.cnt += 1;
        self.op[n] = kind;
        self.a[n] = a;
        self.b[n] = b;
        self.cv[n] = c;
        return n;
    }

    fn isConst(self: *const Tree, i: usize, c: f64) bool {
        return self.op[i] == .konst and self.cv[i] == c;
    }
};

var rng_state: u64 = 20240611;
fn lcg() u64 {
    rng_state = (rng_state * 1103515245 + 12345) % 2147483648;
    return rng_state;
}

fn gen(t: *Tree, depth: u32) usize {
    const k = lcg() / 256 % 10;
    if (k < 2 or depth == 0) {
        const m = lcg() / 256 % 8;
        return if (m < 3) t.push(.variable, 0, 0, 0.0) else t.push(.konst, 0, 0, LEAF[m - 3]);
    }
    const a = gen(t, depth - 1);
    const kind: Op = switch (k) {
        0...3 => .add,
        4 => .sub,
        5...7 => .mul,
        8 => .neg,
        else => .sin,
    };
    const b: usize = if (kind == .add or kind == .sub or kind == .mul) gen(t, depth - 1) else 0;
    return t.push(kind, a, b, 0.0);
}

fn simpNode(s: *Tree, kind: Op, cv: f64, l: usize, r: usize) usize {
    switch (kind) {
        .konst => return s.push(.konst, 0, 0, cv),
        .variable => return s.push(.variable, 0, 0, 0.0),
        .add => {
            if (s.isConst(l, 0.0)) return r;
            if (s.isConst(r, 0.0)) return l;
            if (s.op[l] == .konst and s.op[r] == .konst) return s.push(.konst, 0, 0, s.cv[l] + s.cv[r]);
            return s.push(.add, l, r, 0.0);
        },
        .sub => {
            if (s.isConst(r, 0.0)) return l;
            if (s.op[l] == .konst and s.op[r] == .konst) return s.push(.konst, 0, 0, s.cv[l] - s.cv[r]);
            if (l == r) return s.push(.konst, 0, 0, 0.0);
            return s.push(.sub, l, r, 0.0);
        },
        .mul => {
            if (s.isConst(l, 0.0) or s.isConst(r, 0.0)) return s.push(.konst, 0, 0, 0.0);
            if (s.isConst(l, 1.0)) return r;
            if (s.isConst(r, 1.0)) return l;
            if (s.op[l] == .konst and s.op[r] == .konst) return s.push(.konst, 0, 0, s.cv[l] * s.cv[r]);
            return s.push(.mul, l, r, 0.0);
        },
        .neg => {
            if (s.op[l] == .konst) return s.push(.konst, 0, 0, -s.cv[l]);
            if (s.op[l] == .neg) return s.a[l];
            return s.push(.neg, l, 0, 0.0);
        },
        .sin => {
            if (s.isConst(l, 0.0)) return s.push(.konst, 0, 0, 0.0);
            return s.push(.sin, l, 0, 0.0);
        },
    }
}

fn simplify(t: *const Tree, s: *Tree, rep: *[MAXN]usize) void {
    s.cnt = 0;
    for (0..t.cnt) |i| {
        rep[i] = simpNode(s, t.op[i], t.cv[i], rep[t.a[i]], rep[t.b[i]]);
    }
}

fn evalDual(t: *const Tree, x: f64, ev: *[MAXN]f64, ed: *[MAXN]f64) void {
    for (0..t.cnt) |i| {
        const a = Dual{ .v = ev[t.a[i]], .d = ed[t.a[i]] };
        const b = Dual{ .v = ev[t.b[i]], .d = ed[t.b[i]] };
        const r: Dual = switch (t.op[i]) {
            .konst => .{ .v = t.cv[i], .d = 0.0 },
            .variable => .{ .v = x, .d = 1.0 },
            .add => a.add(b),
            .sub => a.sub(b),
            .mul => a.mul(b),
            .neg => a.neg(),
            .sin => a.sin(),
        };
        ev[i] = r.v;
        ed[i] = r.d;
    }
}

inline fn rhs(t: f64, y: Dual, lam: Dual) Dual {
    const s = @sin(t);
    const c = @cos(t);
    return lam.mul(y.sub(.{ .v = s, .d = 0.0 })).neg().shift(0.5 * c);
}

fn rk4Final(lam: f64, y0: f64, h: f64, steps: usize) Dual {
    const ld = Dual{ .v = lam, .d = 1.0 };
    var y = Dual{ .v = y0, .d = 0.0 };
    const hh = 0.5 * h;
    const h6 = h / 6.0;
    for (0..steps) |k| {
        const t = @as(f64, @floatFromInt(k)) * h;
        const k1 = rhs(t, y, ld);
        const k2 = rhs(t + hh, y.add(k1.scale(hh)), ld);
        const k3 = rhs(t + hh, y.add(k2.scale(hh)), ld);
        const k4 = rhs(t + h, y.add(k3.scale(h)), ld);
        y = y.add(k1.add(k2.scale(2.0)).add(k3.scale(2.0)).add(k4).scale(h6));
    }
    return y;
}

pub fn main(init: std.process.Init) !void {
    var t = Tree{};
    var s = Tree{};
    var rep: [MAXN]usize = @splat(0);
    var ev: [MAXN]f64 = @splat(0.0);
    var ed: [MAXN]f64 = @splat(0.0);
    var sev: [MAXN]f64 = @splat(0.0);
    var sed: [MAXN]f64 = @splat(0.0);
    var orig_nodes: i64 = 0;
    var simp_nodes: i64 = 0;
    var mismatch: i64 = 0;
    var acc_v: f64 = 0.0;
    var acc_d: f64 = 0.0;
    for (0..EXPRS) |_| {
        t.cnt = 0;
        _ = gen(&t, 4);
        const n = t.cnt;
        simplify(&t, &s, &rep);
        const m = s.cnt;
        orig_nodes += @intCast(n);
        simp_nodes += @intCast(m);
        for (0..POINTS) |p| {
            const x = (@as(f64, @floatFromInt(@as(i64, @intCast(p)) - 3))) / 4.0;
            evalDual(&t, x, &ev, &ed);
            evalDual(&s, x, &sev, &sed);
            const vo = ev[n - 1];
            const dro = ed[n - 1];
            const vs = sev[rep[n - 1]];
            const ds = sed[rep[n - 1]];
            if (@abs(vo - vs) > 0.000000001 * (1.0 + @abs(vo))) mismatch += 1;
            if (@abs(dro - ds) > 0.000000001 * (1.0 + @abs(dro))) mismatch += 1;
            acc_v += vs;
            acc_d += ds;
        }
    }

    var sum_y: f64 = 0.0;
    var sum_dy: f64 = 0.0;
    for (0..TRAJ) |j| {
        const r = rk4Final(10.0 + @as(f64, @floatFromInt(j)) * 0.25, 1.0, 0.002, STEPS);
        sum_y += r.v;
        sum_dy += r.d;
    }

    const q: i64 = @intFromFloat(@floor(acc_v * 10000.0));
    const qd: i64 = @intFromFloat(@floor(acc_d * 10000.0));
    const y1: i64 = @intFromFloat(@floor(sum_y * 1000000.0));
    const y2: i64 = @intFromFloat(@floor(sum_dy * 1000000000.0));
    const cs: i64 = orig_nodes + simp_nodes * 7 + mismatch * 13 + q * 3 + qd * 5 + y1 * 11 + y2 * 17;
    var buf: [64]u8 = undefined;
    var w = std.Io.File.stdout().writer(init.io, &buf);
    try w.interface.print("CHECKSUM={d}\n", .{cs});
    try w.interface.flush();
}
