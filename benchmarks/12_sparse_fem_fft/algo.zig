// 12_sparse_fem_fft — 128x128 reaction-Poisson CSR, Jacobi-PCG x20, 2D FFT low-pass bank
const std = @import("std");

const G = 128;
const N = 16384;
const ITERS = 20;
const PI = 3.141592653589793;

var indptr: [N + 1]u32 = undefined;
var indices: [5 * N]u32 = undefined;
var values: [5 * N]f64 = undefined;
var diag: [N]f64 = undefined;

fn buildSystem() void {
    var nz: u32 = 0;
    indptr[0] = 0;
    for (0..N) |i| {
        const row = i / G;
        const col = i % G;
        const d: f64 = 4.0 + @as(f64, @floatFromInt(i * 37 % 11)) * 0.25;
        diag[i] = d;
        if (row > 0) { indices[nz] = @intCast(i - G); values[nz] = -1.0; nz += 1; }
        if (col > 0) { indices[nz] = @intCast(i - 1); values[nz] = -1.0; nz += 1; }
        indices[nz] = @intCast(i); values[nz] = d; nz += 1;
        if (col < G - 1) { indices[nz] = @intCast(i + 1); values[nz] = -1.0; nz += 1; }
        if (row < G - 1) { indices[nz] = @intCast(i + G); values[nz] = -1.0; nz += 1; }
        indptr[i + 1] = nz;
    }
}

fn matvec(x: *const [N]f64, out: *[N]f64) void {
    for (0..N) |r| {
        var s: f64 = 0.0;
        var k: usize = indptr[r];
        while (k < indptr[r + 1]) : (k += 1) s += values[k] * x[indices[k]];
        out[r] = s;
    }
}

fn dot(a: *const [N]f64, b: *const [N]f64) f64 {
    var s: f64 = 0.0;
    for (0..N) |i| s += a[i] * b[i];
    return s;
}

var r_: [N]f64 = undefined;
var z_: [N]f64 = undefined;
var p_: [N]f64 = undefined;
var ap_: [N]f64 = undefined;

fn pcg(b: *const [N]f64, x: *[N]f64) f64 {
    for (0..N) |i| {
        x[i] = 0.0;
        r_[i] = b[i];
        z_[i] = r_[i] / diag[i];
        p_[i] = z_[i];
    }
    var rz = dot(&r_, &z_);
    for (0..ITERS) |_| {
        matvec(&p_, &ap_);
        const alpha = rz / dot(&p_, &ap_);
        for (0..N) |i| {
            x[i] += alpha * p_[i];
            r_[i] -= alpha * ap_[i];
            z_[i] = r_[i] / diag[i];
        }
        const rz_new = dot(&r_, &z_);
        const beta = rz_new / rz;
        for (0..N) |i| p_[i] = z_[i] + beta * p_[i];
        rz = rz_new;
    }
    return dot(&r_, &r_);
}

var bitrev: [G]usize = undefined;
var twr: [7][G / 2]f64 = undefined;
var twi: [7][G / 2]f64 = undefined;

fn initFft() void {
    for (0..G) |i| {
        var r: usize = 0;
        var v: usize = i;
        for (0..7) |_| {
            r = (r << 1) | (v & 1);
            v >>= 1;
        }
        bitrev[i] = r;
    }
    var span: usize = 2;
    var lvl: usize = 0;
    while (span <= G) : (lvl += 1) {
        const ang: f64 = 0.0 - 2.0 * PI / @as(f64, @floatFromInt(span));
        for (0..span / 2) |k| {
            const t = ang * @as(f64, @floatFromInt(k));
            twr[lvl][k] = @cos(t);
            twi[lvl][k] = @sin(t);
        }
        span *= 2;
    }
}

fn fft1(re: *[G]f64, im: *[G]f64, inv: bool) void {
    var tre: [G]f64 = undefined;
    var tim: [G]f64 = undefined;
    if (inv) for (0..G) |i| {
        im[i] = 0.0 - im[i];
    };
    for (0..G) |i| {
        const s = bitrev[i];
        tre[i] = re[s];
        tim[i] = im[s];
    }
    var span: usize = 2;
    var lvl: usize = 0;
    while (span <= G) : (lvl += 1) {
        const half = span / 2;
        var start: usize = 0;
        while (start < G) : (start += span) {
            for (0..half) |k| {
                const wr = twr[lvl][k];
                const wi = twi[lvl][k];
                const a = start + k;
                const b = a + half;
                const tr = tre[b] * wr - tim[b] * wi;
                const ti = tre[b] * wi + tim[b] * wr;
                tre[b] = tre[a] - tr;
                tim[b] = tim[a] - ti;
                tre[a] = tre[a] + tr;
                tim[a] = tim[a] + ti;
            }
        }
        span *= 2;
    }
    if (inv) {
        const s: f64 = 1.0 / @as(f64, G);
        for (0..G) |i| {
            re[i] = tre[i] * s;
            im[i] = (0.0 - tim[i]) * s;
        }
    } else {
        re.* = tre;
        im.* = tim;
    }
}

fn fft2(re: *[N]f64, im: *[N]f64, inv: bool) void {
    var lr: [G]f64 = undefined;
    var li: [G]f64 = undefined;
    for (0..G) |r| {
        for (0..G) |c| {
            lr[c] = re[r * G + c];
            li[c] = im[r * G + c];
        }
        fft1(&lr, &li, inv);
        for (0..G) |c| {
            re[r * G + c] = lr[c];
            im[r * G + c] = li[c];
        }
    }
    for (0..G) |c| {
        for (0..G) |r| {
            lr[r] = re[r * G + c];
            li[r] = im[r * G + c];
        }
        fft1(&lr, &li, inv);
        for (0..G) |r| {
            re[r * G + c] = lr[r];
            im[r * G + c] = li[r];
        }
    }
}

var b_: [N]f64 = undefined;
var x_: [N]f64 = undefined;
var sre: [N]f64 = undefined;
var sim: [N]f64 = undefined;
var fre: [N]f64 = undefined;
var fim: [N]f64 = undefined;

pub fn main(init: std.process.Init) !void {
    for (0..N) |i| b_[i] = (@as(f64, @floatFromInt((i * 7 + (i / G) * 13) % 17)) - 8.0) / 8.0;
    buildSystem();
    const res = @sqrt(pcg(&b_, &x_));

    initFft();
    sre = x_;
    @memset(&sim, 0.0);
    fft2(&sre, &sim, false);

    const cuts = [3]usize{ 64, 256, 1024 };
    var energy: f64 = 0.0;
    var corner: f64 = 0.0;
    for (cuts) |cut| {
        fre = sre;
        fim = sim;
        for (0..N) |k| {
            const ky = k / G;
            const kx = k % G;
            const fy = if (ky <= G / 2) ky else G - ky;
            const fx = if (kx <= G / 2) kx else G - kx;
            if (fx * fx + fy * fy > cut) {
                fre[k] = 0.0;
                fim[k] = 0.0;
            }
        }
        fft2(&fre, &fim, true);
        for (0..N) |k| energy += fre[k] * fre[k];
        corner += fre[0] + fre[N / 2 + G / 2] + fre[N - 1];
    }

    const xs = dot(&x_, &x_);
    const cs: i64 = @as(i64, @intFromFloat(@floor(@sqrt(xs) * 1000000.0))) +
        @as(i64, @intFromFloat(@floor(energy * 1000.0))) +
        @as(i64, @intFromFloat(@floor(corner * 1000000.0))) +
        @as(i64, @intFromFloat(@floor(res * 1000000000.0)));
    var buf: [64]u8 = undefined;
    var w = std.Io.File.stdout().writer(init.io, &buf);
    try w.interface.print("CHECKSUM={d}\n", .{cs});
    try w.interface.flush();
}
