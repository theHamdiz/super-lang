// 01_nbody — 64 bodies × 500 steps, soft gravity, SoA
const std = @import("std");

const N = 64;
const STEPS = 500;
const DT: f64 = 0.001;
const SOFT: f64 = 0.01;

const Lcg = struct {
    s: u64,
    inline fn nextU(self: *Lcg) u32 {
        self.s = (self.s *% 1103515245 +% 12345) % 2147483648;
        return @truncate(self.s);
    }
    inline fn nextF(self: *Lcg) f64 {
        return @as(f64, @floatFromInt(self.nextU())) / 2147483648.0;
    }
};

fn emit(io: std.Io, cs: i64) void {
    var buf: [64]u8 = undefined;
    const s = std.fmt.bufPrint(&buf, "CHECKSUM={d}\n", .{cs}) catch unreachable;
    std.Io.File.stdout().writeStreamingAll(io, s) catch {};
}

pub fn main(init: std.process.Init) !void {
    var rng = Lcg{ .s = 12648430 };
    var x: [N]f64 = undefined;
    var y: [N]f64 = undefined;
    var vx: [N]f64 = undefined;
    var vy: [N]f64 = undefined;
    var m: [N]f64 = undefined;
    for (0..N) |i| {
        x[i] = rng.nextF() * 2.0 - 1.0;
        y[i] = rng.nextF() * 2.0 - 1.0;
        vx[i] = (rng.nextF() - 0.5) * 0.01;
        vy[i] = (rng.nextF() - 0.5) * 0.01;
        m[i] = 0.5 + rng.nextF();
    }
    for (0..STEPS) |_| {
        for (0..N) |i| {
            const xi = x[i];
            const yi = y[i];
            var ax: f64 = 0.0;
            var ay: f64 = 0.0;
            for (0..N) |j| {
                if (i == j) continue;
                const dx = x[j] - xi;
                const dy = y[j] - yi;
                const d2 = dx * dx + dy * dy + SOFT;
                const inv = 1.0 / @sqrt(d2);
                const inv3 = inv * inv * inv;
                ax += m[j] * dx * inv3;
                ay += m[j] * dy * inv3;
            }
            vx[i] += ax * DT;
            vy[i] += ay * DT;
        }
        for (0..N) |i| {
            x[i] += vx[i] * DT;
            y[i] += vy[i] * DT;
        }
    }
    var cs: i64 = 0;
    for (0..N) |i| {
        cs += @as(i64, @intFromFloat(@floor(x[i] * 1000.0)));
        cs += @as(i64, @intFromFloat(@floor(y[i] * 1000.0)));
        cs += @as(i64, @intFromFloat(@floor(vx[i] * 1e6)));
    }
    emit(init.io, cs);
}
