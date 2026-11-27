// 10_sha256_merkle — 1024 leaves × 64 bytes, SHA-256 Merkle root (hand-rolled, as in algo.c)
const std = @import("std");

const LEAVES = 1024;
const LEAF_SZ = 64;

const K = [64]u32{
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
};

const H0 = [8]u32{ 0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19 };

const Digest = [32]u8;

inline fn rotr(x: u32, comptime n: u5) u32 {
    return std.math.rotr(u32, x, n);
}

fn compress(h: *[8]u32, block: *const [64]u8) void {
    var w: [64]u32 = undefined;
    for (0..16) |i| w[i] = std.mem.readInt(u32, block[4 * i ..][0..4], .big);
    for (16..64) |i| {
        const s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >> 3);
        const s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16] +% s0 +% w[i - 7] +% s1;
    }
    var a = h[0];
    var b = h[1];
    var c = h[2];
    var d = h[3];
    var e = h[4];
    var f = h[5];
    var g = h[6];
    var hh = h[7];
    for (0..64) |i| {
        const s1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
        const ch = (e & f) ^ (~e & g);
        const t1 = hh +% s1 +% ch +% K[i] +% w[i];
        const s0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
        const maj = (a & b) ^ (a & c) ^ (b & c);
        const t2 = s0 +% maj;
        hh = g;
        g = f;
        f = e;
        e = d +% t1;
        d = c;
        c = b;
        b = a;
        a = t1 +% t2;
    }
    h[0] +%= a;
    h[1] +%= b;
    h[2] +%= c;
    h[3] +%= d;
    h[4] +%= e;
    h[5] +%= f;
    h[6] +%= g;
    h[7] +%= hh;
}

fn sha256(msg: []const u8) Digest {
    var h = H0;
    const full = msg.len / 64;
    for (0..full) |i| compress(&h, msg[i * 64 ..][0..64]);
    const rem = msg[full * 64 ..];
    var tail: [128]u8 = @splat(0);
    @memcpy(tail[0..rem.len], rem);
    tail[rem.len] = 0x80;
    const tail_len: usize = if (rem.len + 1 + 8 <= 64) 64 else 128;
    std.mem.writeInt(u64, tail[tail_len - 8 ..][0..8], @as(u64, msg.len) * 8, .big);
    compress(&h, tail[0..64]);
    if (tail_len == 128) compress(&h, tail[64..128]);
    var out: Digest = undefined;
    for (0..8) |i| std.mem.writeInt(u32, out[4 * i ..][0..4], h[i], .big);
    return out;
}

fn emit(io: std.Io, cs: i64) void {
    var buf: [64]u8 = undefined;
    const s = std.fmt.bufPrint(&buf, "CHECKSUM={d}\n", .{cs}) catch unreachable;
    std.Io.File.stdout().writeStreamingAll(io, s) catch {};
}

pub fn main(init: std.process.Init) !void {
    const alloc = std.heap.page_allocator;
    const level = try alloc.alloc(Digest, LEAVES);
    defer alloc.free(level);
    const next = try alloc.alloc(Digest, LEAVES);
    defer alloc.free(next);

    var leaf: [LEAF_SZ]u8 = undefined;
    for (0..LEAVES) |i| {
        for (&leaf, 0..) |*v, b| v.* = @truncate(i * 131 + b * 17);
        level[i] = sha256(&leaf);
    }
    var n: usize = LEAVES;
    while (n > 1) {
        const m = (n + 1) / 2;
        for (0..m) |i| {
            var cat: [64]u8 = undefined;
            @memcpy(cat[0..32], &level[2 * i]);
            const sib = if (2 * i + 1 < n) 2 * i + 1 else 2 * i;
            @memcpy(cat[32..64], &level[sib]);
            next[i] = sha256(&cat);
        }
        @memcpy(level[0..m], next[0..m]);
        n = m;
    }
    var cs: i64 = 0;
    for (level[0]) |b| cs = cs *% 131 +% @as(i64, b);
    emit(init.io, cs);
}
