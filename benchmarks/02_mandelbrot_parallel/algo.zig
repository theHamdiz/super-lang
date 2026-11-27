// 02_mandelbrot — 1024×1024 maxiter 128, std.Thread static row partition
const std = @import("std");

const W = 1024;
const H = 1024;
const MAXIT = 128;
const MAX_THREADS = 64;

fn row(y: usize) i64 {
    const cy = (@as(f64, @floatFromInt(y)) / @as(f64, H)) * 2.0 - 1.0;
    var local: i64 = 0;
    for (0..W) |x| {
        const cx = (@as(f64, @floatFromInt(x)) / @as(f64, W)) * 3.5 - 2.5;
        var zx: f64 = 0.0;
        var zy: f64 = 0.0;
        var it: i32 = 0;
        while (it < MAXIT) : (it += 1) {
            const zx2 = zx * zx;
            const zy2 = zy * zy;
            if (zx2 + zy2 > 4.0) break;
            const nzy = 2.0 * zx * zy + cy;
            zx = zx2 - zy2 + cx;
            zy = nzy;
        }
        local += it;
    }
    return local;
}

fn worker(lo: usize, hi: usize, out: *i64) void {
    var s: i64 = 0;
    for (lo..hi) |y| s += row(y);
    out.* = s;
}

fn emit(io: std.Io, cs: i64) void {
    var buf: [64]u8 = undefined;
    const s = std.fmt.bufPrint(&buf, "CHECKSUM={d}\n", .{cs}) catch unreachable;
    std.Io.File.stdout().writeStreamingAll(io, s) catch {};
}

pub fn main(init: std.process.Init) !void {
    const nt: usize = @min(std.Thread.getCpuCount() catch 1, MAX_THREADS);
    const chunk = (H + nt - 1) / nt;
    var results: [MAX_THREADS]i64 = @splat(0);
    var threads: [MAX_THREADS]std.Thread = undefined;
    var spawned: usize = 0;
    for (0..nt) |t| {
        const lo = t * chunk;
        const hi = @min((t + 1) * chunk, H);
        if (lo >= hi) break;
        threads[spawned] = try std.Thread.spawn(.{}, worker, .{ lo, hi, &results[spawned] });
        spawned += 1;
    }
    var cs: i64 = 0;
    for (0..spawned) |t| {
        threads[t].join();
        cs += results[t];
    }
    emit(init.io, cs);
}
