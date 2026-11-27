// 17_ai_tensor_hnsw_search — fixed-point (Q10) transformer blocks (causal attention, softmax,
// ReLU FFN, RMS norm) on 12 sequences, then brute-force top-10 cosine search over 4096 unit
// vectors of dim 32 using the pooled embeddings as queries. Integer-only => bit-exact everywhere.
// Build: zig build-exe -OReleaseFast algo.zig
const std = @import("std");

const MOD: i64 = 1_000_000_007;
const BIAS: i64 = 1 << 40;
const D = 32;
const T = 16;
const FF = 64;
const LAYERS = 2;
const SEQS = 12;
const NV = 4096;
const K = 10;

const Lcg = struct {
    s: i64,
    fn next(self: *Lcg) i64 {
        self.s = @mod(self.s * 48271, 2147483647);
        return self.s;
    }
};

fn isqrt(n: i64) i64 {
    if (n == 0) return 0;
    var x = n;
    var y = (x + 1) >> 1;
    while (y < x) {
        x = y;
        y = (x + @divTrunc(n, x)) >> 1;
    }
    return x;
}

fn expFrac(f: i64) i64 {
    var term: i64 = 1024;
    var sum: i64 = 1024;
    var k: i64 = 1;
    while (k <= 6) : (k += 1) {
        term = @divTrunc((term * f) >> 10, k);
        if (k & 1 == 1) sum -= term else sum += term;
    }
    return @max(sum, 0);
}

fn expQ(x: i64, e1: i64) i64 {
    const a = -x;
    const n = a >> 10;
    if (n >= 16) return 0;
    var r = expFrac(a & 1023);
    var i: i64 = 0;
    while (i < n) : (i += 1) r = (r * e1) >> 10;
    return r;
}

fn matmul(x: []const i64, w: []const i64, y: []i64, din: usize, dout: usize) void {
    for (0..T) |t| {
        for (0..dout) |o| {
            var s: i64 = 0;
            for (0..din) |i| s += x[t * din + i] * w[i * dout + o];
            y[t * dout + o] = s >> 10;
        }
    }
}

fn rmsNorm(h: []i64) void {
    for (0..T) |t| {
        var ss: i64 = 0;
        for (0..D) |d| ss += h[t * D + d] * h[t * D + d];
        const rms = isqrt(@divTrunc(ss, D));
        const inv = @divTrunc(@as(i64, 1) << 20, rms + 1);
        for (0..D) |d| h[t * D + d] = (h[t * D + d] * inv) >> 10;
    }
}

const Layer = struct {
    wq: [D * D]i64,
    wk: [D * D]i64,
    wv: [D * D]i64,
    wo: [D * D]i64,
    w1: [D * FF]i64,
    w2: [FF * D]i64,
};

fn fillW(rng: *Lcg, w: []i64) void {
    for (w) |*v| v.* = @mod(rng.next(), 257) - 128;
}

fn block(ly: *const Layer, h: *[T * D]i64, e1: i64) void {
    var q: [T * D]i64 = undefined;
    var k: [T * D]i64 = undefined;
    var v: [T * D]i64 = undefined;
    var at: [T * D]i64 = undefined;
    var o: [T * D]i64 = undefined;
    var f: [T * FF]i64 = undefined;
    var g: [T * D]i64 = undefined;
    matmul(h, &ly.wq, &q, D, D);
    matmul(h, &ly.wk, &k, D, D);
    matmul(h, &ly.wv, &v, D, D);
    var sc: [T]i64 = undefined;
    var pr: [T]i64 = undefined;
    for (0..T) |t| {
        var mx: i64 = std.math.minInt(i64);
        var sum: i64 = 0;
        for (0..t + 1) |j| {
            var s: i64 = 0;
            for (0..D) |d| s += q[t * D + d] * k[j * D + d];
            sc[j] = s >> 13;
            mx = @max(mx, sc[j]);
        }
        for (0..t + 1) |j| {
            pr[j] = expQ(sc[j] - mx, e1);
            sum += pr[j];
        }
        for (0..t + 1) |j| pr[j] = @divTrunc(pr[j] * 1024, sum);
        for (0..D) |d| {
            var s: i64 = 0;
            for (0..t + 1) |j| s += pr[j] * v[j * D + d];
            at[t * D + d] = s >> 10;
        }
    }
    matmul(&at, &ly.wo, &o, D, D);
    for (0..T * D) |i| h[i] += o[i];
    rmsNorm(h);
    matmul(h, &ly.w1, &f, D, FF);
    for (&f) |*x| x.* = @max(x.*, 0);
    matmul(&f, &ly.w2, &g, FF, D);
    for (0..T * D) |i| h[i] += g[i];
    rmsNorm(h);
}

fn normalize(raw: *const [D]i64) [D]i64 {
    var ss: i64 = 0;
    for (raw) |v| ss += v * v;
    const inv = @divTrunc(@as(i64, 1) << 20, isqrt(ss) + 1);
    var out: [D]i64 = undefined;
    for (0..D) |d| out[d] = (raw[d] * inv) >> 10;
    return out;
}

fn emit(io: std.Io, cs: i64) void {
    var buf: [64]u8 = undefined;
    const s = std.fmt.bufPrint(&buf, "CHECKSUM={d}\n", .{cs}) catch unreachable;
    std.Io.File.stdout().writeStreamingAll(io, s) catch {};
}

pub fn main(init: std.process.Init) !void {
    const alloc = std.heap.page_allocator;
    var rng = Lcg{ .s = 17017 };
    const e1 = expFrac(1024);
    const layers = try alloc.alloc(Layer, LAYERS);
    defer alloc.free(layers);
    for (layers) |*ly| {
        fillW(&rng, &ly.wq);
        fillW(&rng, &ly.wk);
        fillW(&rng, &ly.wv);
        fillW(&rng, &ly.wo);
        fillW(&rng, &ly.w1);
        fillW(&rng, &ly.w2);
    }
    const db = try alloc.alloc([D]i64, NV);
    defer alloc.free(db);
    for (db) |*row| {
        var raw: [D]i64 = undefined;
        for (&raw) |*v| v.* = @mod(rng.next(), 2049) - 1024;
        row.* = normalize(&raw);
    }
    var cs: i64 = 0;
    for (0..SEQS) |_| {
        var h: [T * D]i64 = undefined;
        for (&h) |*v| v.* = @mod(rng.next(), 2049) - 1024;
        for (layers) |*ly| block(ly, &h, e1);
        var pooled: [D]i64 = undefined;
        for (0..D) |d| {
            var sum: i64 = 0;
            for (0..T) |t| sum += h[t * D + d];
            pooled[d] = sum >> 4;
            cs = @mod(cs * 131 + pooled[d] + BIAS, MOD);
        }
        const query = normalize(&pooled);
        var top_s: [K]i64 = undefined;
        var top_i: [K]i64 = undefined;
        var cnt: usize = 0;
        for (db, 0..) |*row, i| {
            var sc: i64 = 0;
            for (0..D) |d| sc += query[d] * row[d];
            if (cnt < K or sc > top_s[K - 1]) {
                var p: usize = if (cnt < K) cnt else K - 1;
                while (p > 0 and top_s[p - 1] < sc) {
                    top_s[p] = top_s[p - 1];
                    top_i[p] = top_i[p - 1];
                    p -= 1;
                }
                top_s[p] = sc;
                top_i[p] = @intCast(i);
                if (cnt < K) cnt += 1;
            }
        }
        for (0..K) |j| {
            cs = @mod(cs * 131 + top_i[j] + BIAS, MOD);
            cs = @mod(cs * 131 + top_s[j] + BIAS, MOD);
        }
    }
    emit(init.io, cs);
}
