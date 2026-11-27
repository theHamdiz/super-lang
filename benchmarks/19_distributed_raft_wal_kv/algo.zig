// 19_distributed_raft_wal_kv — single-process Raft log replication (leader + 2 followers):
// CRC32-protected WAL records, lossy/corrupting virtual links, majority commit, KV state machine.
// Build: zig build-exe -OReleaseFast algo.zig
const std = @import("std");

const MOD: i64 = 1_000_000_007;
const TICKS = 6000;
const DRAIN = 200;
const REC = 20;
const BATCH = 4;
const KEYS = 512;
const DROP_PCT = 35;
const ACK_DROP_PCT = 15;
const CORRUPT_PCT = 3;
const CAP = TICKS * REC;

const CRC: [256]u32 = blk: {
    @setEvalBranchQuota(10000);
    var t: [256]u32 = undefined;
    for (&t, 0..) |*e, i| {
        var c: u32 = @intCast(i);
        for (0..8) |_| c = if (c & 1 == 1) (c >> 1) ^ 0xEDB88320 else c >> 1;
        e.* = c;
    }
    break :blk t;
};

fn crc32(d: []const u8) u32 {
    var c: u32 = 0xFFFFFFFF;
    for (d) |b| c = CRC[(c ^ b) & 255] ^ (c >> 8);
    return c ^ 0xFFFFFFFF;
}
fn put32(p: []u8, v: u32) void {
    std.mem.writeInt(u32, p[0..4], v, .big);
}
fn get32(p: []const u8) u32 {
    return std.mem.readInt(u32, p[0..4], .big);
}

const Node = struct {
    wal: [CAP]u8 = undefined,
    len: usize = 0,
    applied: usize = 0,
    kv: [KEYS]i64 = @splat(0),

    fn applyUpto(self: *Node, upto: usize) void {
        while (self.applied < upto) {
            const r = self.wal[self.applied * REC ..][0..REC];
            if (crc32(r[0..16]) == get32(r[16..])) self.kv[get32(r[8..]) % KEYS] = get32(r[12..]);
            self.applied += 1;
        }
    }
};

var rng: i64 = 19019;
fn nextRand() i64 {
    rng = @rem(rng * 48271, 2147483647);
    return rng;
}

var nodes: [3]Node = .{ .{}, .{}, .{} };

fn emit(io: std.Io, cs: i64) void {
    var buf: [64]u8 = undefined;
    const s = std.fmt.bufPrint(&buf, "CHECKSUM={d}\n", .{cs}) catch unreachable;
    std.Io.File.stdout().writeStreamingAll(io, s) catch {};
}

pub fn main(init: std.process.Init) !void {
    var nxt = [3]usize{ 1, 1, 1 };
    var mtch = [3]usize{ 0, 0, 0 };
    var commit: usize = 0;
    var sent: i64 = 0;
    var dropped: i64 = 0;
    var crc_rejects: i64 = 0;
    var acks_lost: i64 = 0;
    for (0..TICKS + DRAIN) |t| {
        if (t < TICKS) {
            const l = &nodes[0];
            const o = l.len * REC;
            put32(l.wal[o..], @intCast(l.len + 1));
            put32(l.wal[o + 4 ..], 1);
            put32(l.wal[o + 8 ..], @intCast(@rem(nextRand(), KEYS)));
            put32(l.wal[o + 12 ..], @intCast(@rem(nextRand(), 1000000)));
            put32(l.wal[o + 16 ..], crc32(l.wal[o..][0..16]));
            l.len += 1;
        }
        const leader_commit = commit;
        for (1..3) |f| {
            const n0 = nxt[f];
            if (n0 > nodes[0].len) continue;
            const cnt: usize = @min(nodes[0].len - n0 + 1, BATCH);
            const r1 = @rem(nextRand(), 100);
            const r2 = @rem(nextRand(), 100);
            const r3 = @rem(nextRand(), 100);
            sent += 1;
            if (r1 < DROP_PCT) {
                dropped += 1;
                continue;
            }
            var msg: [BATCH * REC]u8 = undefined;
            @memcpy(msg[0 .. cnt * REC], nodes[0].wal[(n0 - 1) * REC ..][0 .. cnt * REC]);
            if (r2 < CORRUPT_PCT) {
                const pos: usize = @intCast(@rem(nextRand(), @as(i64, @intCast(cnt * REC))));
                msg[pos] ^= 0x5A;
            }
            const fol = &nodes[f];
            for (0..cnt) |j| {
                const r = msg[j * REC ..][0..REC];
                if (crc32(r[0..16]) != get32(r[16..])) {
                    crc_rejects += 1;
                    break;
                }
                const idx: usize = get32(r);
                if (idx == fol.len + 1) {
                    @memcpy(fol.wal[fol.len * REC ..][0..REC], r);
                    fol.len += 1;
                } else if (idx > fol.len + 1) break;
            }
            fol.applyUpto(@min(leader_commit, fol.len));
            if (r3 < ACK_DROP_PCT) {
                acks_lost += 1;
                continue;
            }
            mtch[f] = @max(mtch[f], fol.len);
            nxt[f] = fol.len + 1;
        }
        var m = [3]usize{ nodes[0].len, mtch[1], mtch[2] };
        std.mem.sort(usize, &m, {}, std.sort.asc(usize));
        commit = @max(commit, m[1]);
        nodes[0].applyUpto(commit);
    }
    var cs: i64 = 0;
    const fold = struct {
        fn f(c: *i64, v: i64) void {
            c.* = @rem(c.* * 131 + v, MOD);
        }
    }.f;
    for (&nodes) |*n| fold(&cs, @intCast(n.len));
    fold(&cs, @intCast(commit));
    for (&nodes) |*n| fold(&cs, @intCast(n.applied));
    for ([_]i64{ sent, dropped, crc_rejects, acks_lost }) |v| fold(&cs, v);
    for (&nodes) |*n| {
        var d: i64 = 0;
        for (n.kv) |v| d = @rem(d * 131 + v, MOD);
        fold(&cs, d);
    }
    for (nodes[0..2]) |*n| fold(&cs, crc32(n.wal[0 .. n.len * REC]));
    emit(init.io, cs);
}
