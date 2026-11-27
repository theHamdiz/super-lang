// 08_ecs — 2000 entities × 20 ticks, spatial-grid neighbour queries (SoA)
const std = @import("std");

const N = 2000;
const TICKS = 20;
const CELL: f64 = 10.0;
const GRID: i32 = 64;

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

inline fn cellOf(p: f64) i32 {
    const c: i32 = @intFromFloat(p / CELL);
    return std.math.clamp(c, 0, GRID - 1);
}

fn emit(io: std.Io, cs: i64) void {
    var buf: [64]u8 = undefined;
    const s = std.fmt.bufPrint(&buf, "CHECKSUM={d}\n", .{cs}) catch unreachable;
    std.Io.File.stdout().writeStreamingAll(io, s) catch {};
}

pub fn main(init: std.process.Init) !void {
    var rng = Lcg{ .s = 15482888 };
    var px: [N]f64 = undefined;
    var py: [N]f64 = undefined;
    var vx: [N]f64 = undefined;
    var vy: [N]f64 = undefined;
    var hp: [N]i32 = undefined;
    for (0..N) |i| {
        px[i] = rng.nextF() * 600.0;
        py[i] = rng.nextF() * 600.0;
        vx[i] = (rng.nextF() - 0.5) * 2.0;
        vy[i] = (rng.nextF() - 0.5) * 2.0;
        hp[i] = 10 + @as(i32, @intCast(rng.nextU() % 20));
    }
    var heads: [GRID * GRID]i32 = undefined;
    var next: [N]i32 = undefined;

    for (0..TICKS) |_| {
        for (0..N) |i| {
            px[i] += vx[i];
            py[i] += vy[i];
            if (px[i] < 0 or px[i] > 600) vx[i] = -vx[i];
            if (py[i] < 0 or py[i] > 600) vy[i] = -vy[i];
        }
        @memset(&heads, -1);
        for (0..N) |i| {
            const cell: usize = @intCast(cellOf(py[i]) * GRID + cellOf(px[i]));
            next[i] = heads[cell];
            heads[cell] = @intCast(i);
        }
        for (0..N) |i| {
            const cx = cellOf(px[i]);
            const cy = cellOf(py[i]);
            var hits: i32 = 0;
            var ny = @max(cy - 1, 0);
            while (ny <= @min(cy + 1, GRID - 1)) : (ny += 1) {
                var nx = @max(cx - 1, 0);
                while (nx <= @min(cx + 1, GRID - 1)) : (nx += 1) {
                    var id = heads[@intCast(ny * GRID + nx)];
                    while (id >= 0) {
                        const o: usize = @intCast(id);
                        if (o != i) {
                            const dx: f32 = @floatCast(px[i] - px[o]);
                            const dy: f32 = @floatCast(py[i] - py[o]);
                            if (dx * dx + dy * dy < 4.0) hits += 1;
                        }
                        id = next[o];
                    }
                }
            }
            if (hits > 3) hp[i] -= 1;
        }
    }
    var cs: i64 = 0;
    for (0..N) |i| {
        cs += hp[i];
        cs += @as(i64, @intFromFloat(@floor(px[i] * 10.0)));
        cs += @as(i64, @intFromFloat(@floor(py[i] * 10.0)));
    }
    emit(init.io, cs);
}
