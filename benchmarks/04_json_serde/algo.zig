// 04_json_serde — LCG field checksum
const std = @import("std");

const N = 10_000;

var rng: u64 = 4861952;

inline fn step() u64 {
    rng = (rng *% 1103515245 +% 12345) % 2147483648;
    return rng;
}

fn emit(io: std.Io, cs: i64) void {
    var buf: [64]u8 = undefined;
    const s = std.fmt.bufPrint(&buf, "CHECKSUM={d}\n", .{cs}) catch unreachable;
    std.Io.File.stdout().writeStreamingAll(io, s) catch {};
}

pub fn main(init: std.process.Init) !void {
    var cs: i64 = 0;
    for (0..N) |_| {
        const id: i64 = @intCast(step() % 1_000_000);
        const total: i64 = @intCast(step() % 100_000);
        const items: i64 = @as(i64, @intCast(step() % 8)) + 1;
        const v = id + total + items * 17;
        cs += v;
        cs += v;
    }
    emit(init.io, cs);
}
