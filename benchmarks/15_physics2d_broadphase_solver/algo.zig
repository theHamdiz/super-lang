// 15_physics2d_broadphase_solver - 6k circles/boxes: uniform spatial-hash broadphase,
// circle/box manifolds, sequential-impulse solver (4 iterations x 10 steps).
// Mirrors std.game.physics2d (grid build, candidate order, narrowphase, resolve).
const std = @import("std");

const COLS = 100;
const ROWS = 60;
const N_DYN = COLS * ROWS;
const FLOOR_TILES = 62;
const WALL_TILES = 40;
const N = FLOOR_TILES + 2 * WALL_TILES + N_DYN;
const STEPS = 10;
const ITERS = 4;
const MOD: i64 = 1000000007;
const MAX_ENTRIES = 400000;

const Shape = enum(u8) { circle, box };

const Body = struct {
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    inv_mass: f64,
    shape: Shape,
    r: f64,
    hw: f64,
    hh: f64,
    e: f64 = 0.2,
    mu: f64 = 0.4,
};

const Contact = struct { nx: f64, ny: f64, depth: f64 };

var bodies: [N]Body = undefined;
var nb: usize = 0;

var rng: u64 = 150015;
fn draw(m: i64) i64 {
    rng = (rng * 1103515245 + 12345) % 2147483648;
    return @intCast(rng / 256 % @as(u64, @intCast(m)));
}

fn inv(mass: f64) f64 {
    return if (mass <= 0.0) 0.0 else 1.0 / mass;
}
fn addCircle(x: f64, y: f64, r: f64, mass: f64, vx: f64, vy: f64) void {
    bodies[nb] = .{ .x = x, .y = y, .vx = vx, .vy = vy, .inv_mass = inv(mass), .shape = .circle, .r = r, .hw = r, .hh = r };
    nb += 1;
}
fn addBox(x: f64, y: f64, hw: f64, hh: f64, mass: f64, vx: f64, vy: f64) void {
    bodies[nb] = .{ .x = x, .y = y, .vx = vx, .vy = vy, .inv_mass = inv(mass), .shape = .box, .r = 0.0, .hw = hw, .hh = hh };
    nb += 1;
}

fn sign(v: f64) f64 {
    return if (v < 0.0) -1.0 else 1.0;
}
fn clamp(v: f64, lo: f64, hi: f64) f64 {
    return if (v < lo) lo else if (v > hi) hi else v;
}
fn pmax(a: f64, b: f64) f64 {
    return if (a > b) a else b;
}
fn pmin(a: f64, b: f64) f64 {
    return if (a < b) a else b;
}

fn circleCircle(a: *const Body, b: *const Body) ?Contact {
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const rr = a.r + b.r;
    const d2 = dx * dx + dy * dy;
    if (d2 >= rr * rr) return null;
    if (d2 == 0.0) return .{ .nx = 1.0, .ny = 0.0, .depth = rr };
    const d = @sqrt(d2);
    return .{ .nx = dx / d, .ny = dy / d, .depth = rr - d };
}

fn boxBox(a: *const Body, b: *const Body) ?Contact {
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const ox = a.hw + b.hw - @abs(dx);
    const oy = a.hh + b.hh - @abs(dy);
    if (ox <= 0.0) return null;
    if (oy <= 0.0) return null;
    if (ox < oy) return .{ .nx = sign(dx), .ny = 0.0, .depth = ox };
    return .{ .nx = 0.0, .ny = sign(dy), .depth = oy };
}

fn circleBox(a: *const Body, b: *const Body) ?Contact {
    const lx = a.x - b.x;
    const ly = a.y - b.y;
    const cx = clamp(lx, 0.0 - b.hw, b.hw);
    const cy = clamp(ly, 0.0 - b.hh, b.hh);
    const fx = lx - cx;
    const fy = ly - cy;
    const d2 = fx * fx + fy * fy;
    if (d2 > 0.0) {
        if (d2 >= a.r * a.r) return null;
        const d = @sqrt(d2);
        return .{ .nx = (0.0 - fx) / d, .ny = (0.0 - fy) / d, .depth = a.r - d };
    }
    const px = b.hw - @abs(lx);
    const py = b.hh - @abs(ly);
    if (px < py) return .{ .nx = 0.0 - sign(lx), .ny = 0.0, .depth = px + a.r };
    return .{ .nx = 0.0, .ny = 0.0 - sign(ly), .depth = py + a.r };
}

fn collide(a: *const Body, b: *const Body) ?Contact {
    switch (a.shape) {
        .circle => switch (b.shape) {
            .circle => return circleCircle(a, b),
            .box => return circleBox(a, b),
        },
        .box => switch (b.shape) {
            .box => return boxBox(a, b),
            .circle => {
                const c = circleBox(b, a) orelse return null;
                return .{ .nx = 0.0 - c.nx, .ny = 0.0 - c.ny, .depth = c.depth };
            },
        },
    }
}

fn resolve(a: *Body, b: *Body, c: Contact) void {
    const ia = a.inv_mass;
    const ib = b.inv_mass;
    const isum = ia + ib;
    if (isum == 0.0) return;
    const rvx = b.vx - a.vx;
    const rvy = b.vy - a.vy;
    const vn = rvx * c.nx + rvy * c.ny;
    if (vn < 0.0) {
        const e = pmin(a.e, b.e);
        const j = (0.0 - (1.0 + e)) * vn / isum;
        a.vx = a.vx - j * ia * c.nx;
        a.vy = a.vy - j * ia * c.ny;
        b.vx = b.vx + j * ib * c.nx;
        b.vy = b.vy + j * ib * c.ny;
        var tx = rvx - vn * c.nx;
        var ty = rvy - vn * c.ny;
        const tl = @sqrt(tx * tx + ty * ty);
        if (tl > 0.000001) {
            tx = tx / tl;
            ty = ty / tl;
            var jt = (0.0 - (rvx * tx + rvy * ty)) / isum;
            const mu = @sqrt(a.mu * b.mu);
            const lim = mu * j;
            jt = clamp(jt, 0.0 - lim, lim);
            a.vx = a.vx - jt * ia * tx;
            a.vy = a.vy - jt * ia * ty;
            b.vx = b.vx + jt * ib * tx;
            b.vy = b.vy + jt * ib * ty;
        }
    }
    const corr = pmax(c.depth - 0.01, 0.0) / isum * 0.8;
    a.x = a.x - corr * ia * c.nx;
    a.y = a.y - corr * ia * c.ny;
    b.x = b.x + corr * ib * c.nx;
    b.y = b.y + corr * ib * c.ny;
}

// ---- broadphase: uniform spatial hash (per-bucket entry chains) ----
var heads: [4 * N]i64 = undefined;
var eb: [MAX_ENTRIES]i64 = undefined;
var ex: [MAX_ENTRIES]i64 = undefined;
var ey: [MAX_ENTRIES]i64 = undefined;
var en: [MAX_ENTRIES]i64 = undefined;
var elen: usize = 0;
var ginv: f64 = 1.0;
var gmask: i64 = 15;
var mark: [N]i64 = undefined;
var cands: [N]i64 = undefined;

fn cellOf(v: f64) i64 {
    return @intFromFloat(@floor(v * ginv));
}
fn bucket(cx: i64, cy: i64) usize {
    return @intCast(((cx *% 73856093) ^ (cy *% 19349663)) & gmask);
}

fn gridBuild() void {
    var sum: f64 = 0.0;
    var big: f64 = 0.0;
    for (bodies[0..nb]) |b| {
        const e = pmax(b.hw, b.hh);
        sum += e;
        if (e > big) big = e;
    }
    var cell = pmax(2.0 * sum / @as(f64, @floatFromInt(nb)), big / 8.0);
    if (cell < 0.000001) cell = 1.0;
    ginv = 1.0 / cell;
    const pad = cell * 0.1;
    var size: usize = 16;
    while (size < nb * 2) size *= 2;
    gmask = @intCast(size - 1);
    @memset(heads[0..size], -1);
    elen = 0;
    for (bodies[0..nb], 0..) |b, i| {
        const cx0 = cellOf(b.x - b.hw - pad);
        const cx1 = cellOf(b.x + b.hw + pad);
        const cy0 = cellOf(b.y - b.hh - pad);
        const cy1 = cellOf(b.y + b.hh + pad);
        var cy = cy0;
        while (cy < cy1 + 1) : (cy += 1) {
            var cx = cx0;
            while (cx < cx1 + 1) : (cx += 1) {
                const slot = bucket(cx, cy);
                en[elen] = heads[slot];
                heads[slot] = @intCast(elen);
                eb[elen] = @intCast(i);
                ex[elen] = cx;
                ey[elen] = cy;
                elen += 1;
            }
        }
    }
}

// bodies j > i whose registered cells overlap body i's padded AABB: ascending, unique
fn gridCandidates(b: *const Body, i: i64) usize {
    const pad = 0.1 / ginv;
    const cx0 = cellOf(b.x - b.hw - pad);
    const cx1 = cellOf(b.x + b.hw + pad);
    const cy0 = cellOf(b.y - b.hh - pad);
    const cy1 = cellOf(b.y + b.hh + pad);
    var cnt: usize = 0;
    var cy = cy0;
    while (cy < cy1 + 1) : (cy += 1) {
        var cx = cx0;
        while (cx < cx1 + 1) : (cx += 1) {
            var e = heads[bucket(cx, cy)];
            while (e >= 0) {
                const eu: usize = @intCast(e);
                if (ex[eu] == cx and ey[eu] == cy) {
                    const j = eb[eu];
                    if (j > i and mark[@intCast(j)] != i + 1) {
                        mark[@intCast(j)] = i + 1;
                        cands[cnt] = j;
                        cnt += 1;
                    }
                }
                e = en[eu];
            }
        }
    }
    std.mem.sort(i64, cands[0..cnt], {}, std.sort.asc(i64));
    return cnt;
}

fn solvePair(i: usize, j: usize) i64 {
    const a = &bodies[i];
    const b = &bodies[j];
    if (a.inv_mass + b.inv_mass > 0.0 and !(@abs(b.x - a.x) >= a.hw + b.hw) and !(@abs(b.y - a.y) >= a.hh + b.hh)) {
        if (collide(a, b)) |c| {
            resolve(a, b, c);
            return 1;
        }
    }
    return 0;
}

fn solveGrid() i64 {
    gridBuild();
    @memset(mark[0..nb], 0);
    var last: i64 = 0;
    for (0..nb) |i| {
        const cnt = gridCandidates(&bodies[i], @intCast(i));
        for (cands[0..cnt]) |j| last += solvePair(i, @intCast(j));
    }
    return last;
}

fn fl(v: f64) i64 {
    return @intFromFloat(@floor(v));
}
fn posmod(v: i64) i64 {
    return @mod(@mod(v, MOD) + MOD, MOD);
}

pub fn main(init: std.process.Init) !void {
    var k: usize = 0;
    while (k < FLOOR_TILES) : (k += 1) addBox(1.0 + 2.0 * @as(f64, @floatFromInt(k)), -1.0, 1.0, 1.0, 0.0, 0.0, 0.0);
    k = 0;
    while (k < WALL_TILES) : (k += 1) addBox(-1.0, 1.0 + 2.0 * @as(f64, @floatFromInt(k)), 1.0, 1.0, 0.0, 0.0, 0.0);
    k = 0;
    while (k < WALL_TILES) : (k += 1) addBox(122.0, 1.0 + 2.0 * @as(f64, @floatFromInt(k)), 1.0, 1.0, 0.0, 0.0, 0.0);
    for (0..N_DYN) |i| {
        const col: f64 = @floatFromInt(i % COLS);
        const row: f64 = @floatFromInt(i / COLS);
        const kind = draw(3);
        const sw = 0.35 + @as(f64, @floatFromInt(draw(400001))) * 0.000001;
        const sh = 0.35 + @as(f64, @floatFromInt(draw(400001))) * 0.000001;
        const mass = 1.0 + @as(f64, @floatFromInt(draw(4)));
        const jx = @as(f64, @floatFromInt(draw(600001) - 300000)) * 0.000001;
        const jy = @as(f64, @floatFromInt(draw(600001) - 300000)) * 0.000001;
        const vx = @as(f64, @floatFromInt(draw(8000001) - 4000000)) * 0.000001;
        const vy = @as(f64, @floatFromInt(draw(8000001) - 4000000)) * 0.000001;
        const x = 0.75 + col * 1.2 + jx;
        const y = 1.75 + row * 1.2 + jy;
        if (kind == 0) addBox(x, y, sw, sh, mass, vx, vy) else addCircle(x, y, sw, mass, vx, vy);
    }

    var cs: i64 = 0;
    const dt: f64 = 1.0 / 60.0;
    const damp: f64 = 1.0 / (1.0 + dt * 0.0);
    for (0..STEPS) |s| {
        for (bodies[0..nb]) |*b| {
            if (b.inv_mass > 0.0) {
                b.vx = (b.vx + 0.0 * dt) * damp;
                b.vy = (b.vy + -9.81 * dt) * damp;
                b.x = b.x + b.vx * dt;
                b.y = b.y + b.vy * dt;
            }
        }
        var last: i64 = 0;
        for (0..ITERS) |_| last = solveGrid();
        cs += last * @as(i64, @intCast(s + 1));
    }
    var h: i64 = 0;
    var ke: f64 = 0.0;
    for (bodies[0..nb]) |b| {
        h = @mod(h * 131 + posmod(fl(b.x * 1000.0)), MOD);
        h = @mod(h * 131 + posmod(fl(b.y * 1000.0)), MOD);
        h = @mod(h * 131 + posmod(fl(b.vx * 1000.0)), MOD);
        h = @mod(h * 131 + posmod(fl(b.vy * 1000.0)), MOD);
        if (b.inv_mass > 0.0) ke += 0.5 * (b.vx * b.vx + b.vy * b.vy) / b.inv_mass;
    }
    cs += h + fl(ke * 10.0);

    var buf: [64]u8 = undefined;
    var w = std.Io.File.stdout().writer(init.io, &buf);
    try w.interface.print("CHECKSUM={d}\n", .{cs});
    try w.interface.flush();
}
