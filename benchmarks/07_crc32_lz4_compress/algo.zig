// 07_crc32 — IEEE CRC32 on expanded LCG-scrambled block
const std = @import("std");

const SEED = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ..";

const table: [256]u32 = blk: {
    @setEvalBranchQuota(10_000);
    var t: [256]u32 = undefined;
    for (&t, 0..) |*e, i| {
        var c: u32 = @intCast(i);
        for (0..8) |_| c = if (c & 1 != 0) (0xEDB88320 ^ (c >> 1)) else (c >> 1);
        e.* = c;
    }
    break :blk t;
};

fn crc32(data: []const u8) u32 {
    var crc: u32 = 0xFFFF_FFFF;
    for (data) |b| crc = table[(crc ^ b) & 0xFF] ^ (crc >> 8);
    return crc ^ 0xFFFF_FFFF;
}

fn emit(io: std.Io, cs: i64) void {
    var buf: [64]u8 = undefined;
    const s = std.fmt.bufPrint(&buf, "CHECKSUM={d}\n", .{cs}) catch unreachable;
    std.Io.File.stdout().writeStreamingAll(io, s) catch {};
}

pub fn main(init: std.process.Init) !void {
    const alloc = std.heap.page_allocator;
    const block_len = SEED.len << 6;
    const block = try alloc.alloc(u8, block_len);
    defer alloc.free(block);
    @memcpy(block[0..SEED.len], SEED);
    var cur: usize = SEED.len;
    for (0..6) |_| {
        @memcpy(block[cur..][0..cur], block[0..cur]);
        cur *= 2;
    }

    var rng: u64 = 8137778;
    var acc: i64 = 0;
    for (0..4096) |i| {
        rng = (rng *% 1103515245 +% 12345) % 2147483648;
        acc += @as(i64, @intCast(rng % 255)) + @as(i64, block[i % block_len]);
    }

    const payload = try alloc.alloc(u8, block_len * 9);
    defer alloc.free(payload);
    for (0..9) |r| @memcpy(payload[r * block_len ..][0..block_len], block);

    const c1 = crc32(payload);
    const c2 = crc32(block);
    const cs: i64 = @as(i64, c1) + @as(i64, c2) + @rem(acc, 1000);
    emit(init.io, cs);
}
