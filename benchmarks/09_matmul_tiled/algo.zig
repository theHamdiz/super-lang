// 09_matmul_tiled — 256×256 f64 cache-tiled multiply (i-k-j order, @Vector inner loop)
const std = @import("std");

const N = 256;
const TILE = 32;
const VL = std.simd.suggestVectorLength(f64) orelse 4;
const V = @Vector(VL, f64);
comptime {
    std.debug.assert(TILE % VL == 0 and N % TILE == 0);
}

var A: [N * N]f64 = undefined;
var B: [N * N]f64 = undefined;
var C: [N * N]f64 = @splat(0);

fn emit(io: std.Io, cs: i64) void {
    var buf: [64]u8 = undefined;
    const s = std.fmt.bufPrint(&buf, "CHECKSUM={d}\n", .{cs}) catch unreachable;
    std.Io.File.stdout().writeStreamingAll(io, s) catch {};
}

pub fn main(init: std.process.Init) !void {
    for (0..N * N) |i| {
        A[i] = @as(f64, @floatFromInt((i * 17) % 100)) / 100.0;
        B[i] = @as(f64, @floatFromInt((i * 31) % 100)) / 100.0;
    }
    var ii: usize = 0;
    while (ii < N) : (ii += TILE) {
        var jj: usize = 0;
        while (jj < N) : (jj += TILE) {
            var kk: usize = 0;
            while (kk < N) : (kk += TILE) {
                for (ii..ii + TILE) |i| {
                    for (kk..kk + TILE) |k| {
                        const aik: V = @splat(A[i * N + k]);
                        var j = jj;
                        while (j < jj + TILE) : (j += VL) {
                            const bv: V = B[k * N + j ..][0..VL].*;
                            const cp = C[i * N + j ..][0..VL];
                            cp.* = @as(V, cp.*) + aik * bv;
                        }
                    }
                }
            }
        }
    }
    var fro: f64 = 0.0;
    for (C) |v| fro += v * v;
    const cs: i64 = @intFromFloat(@floor(@sqrt(fro) * 1000.0));
    emit(init.io, cs);
}
