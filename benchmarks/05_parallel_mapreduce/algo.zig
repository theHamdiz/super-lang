// 05_parallel_mapreduce — word + 3-gram frequencies over a generated corpus.
// Worker threads build per-thread tables; the final checksum is the single-pass
// rescan, exactly as in algo.c (chunk boundaries may split words).
const std = @import("std");

const CORPUS = 65536;
const BUCKETS = 65536;
const MAX_WORKERS = 32;
const WORDS = [_][]const u8{ "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet" };

const Pair = struct { key: u32 = 0, count: i32 = 0 };

inline fn hashBytes(s: []const u8) u32 {
    var h: u32 = 2166136261;
    for (s) |b| {
        h ^= b;
        h *%= 16777619;
    }
    return h;
}

inline fn bump(tab: []Pair, key: u32, add: i32) void {
    const i = key & (BUCKETS - 1);
    var probe: u32 = 0;
    while (probe < BUCKETS) : (probe += 1) {
        const slot = &tab[(i +% probe) & (BUCKETS - 1)];
        if (slot.count == 0) {
            slot.* = .{ .key = key, .count = add };
            return;
        }
        if (slot.key == key) {
            slot.count += add;
            return;
        }
    }
}

inline fn isLower(c: u8) bool {
    return c >= 'a' and c <= 'z';
}

fn scan(text: []const u8, tab: []Pair) void {
    const end = text.len;
    var i: usize = 0;
    while (i < end) {
        while (i < end and !isLower(text[i])) i += 1;
        const start = i;
        while (i < end and isLower(text[i])) i += 1;
        if (i > start) {
            const w = text[start..i];
            bump(tab, hashBytes(w), 1);
            if (w.len >= 3) bump(tab, hashBytes(w[0..3]) ^ 0xC0FFEE, 1);
        }
    }
}

fn worker(text: []const u8, tab: []Pair) void {
    scan(text, tab);
}

const Summary = struct { total: i64, best: i32, best_key: u32 };

fn summarize(tab: []const Pair) Summary {
    var s = Summary{ .total = 0, .best = 0, .best_key = 0 };
    for (tab) |p| {
        if (p.count == 0) continue;
        s.total += p.count;
        if (p.count > s.best or (p.count == s.best and p.key < s.best_key)) {
            s.best = p.count;
            s.best_key = p.key;
        }
    }
    return s;
}

fn emit(io: std.Io, cs: i64) void {
    var buf: [64]u8 = undefined;
    const s = std.fmt.bufPrint(&buf, "CHECKSUM={d}\n", .{cs}) catch unreachable;
    std.Io.File.stdout().writeStreamingAll(io, s) catch {};
}

pub fn main(init: std.process.Init) !void {
    const alloc = std.heap.page_allocator;
    const nworkers: usize = @max(1, @min(std.Thread.getCpuCount() catch 1, MAX_WORKERS));

    var text: [CORPUS]u8 = undefined;
    var rng: u64 = 370945;
    var off: usize = 0;
    while (off + 16 < CORPUS) {
        rng = (rng *% 1103515245 +% 12345) % 2147483648;
        const w = WORDS[rng % 10];
        @memcpy(text[off..][0..w.len], w);
        off += w.len;
        text[off] = ' ';
        off += 1;
    }
    while (off < CORPUS) : (off += 1) text[off] = 'x';

    const tables = try alloc.alloc(Pair, nworkers * BUCKETS);
    defer alloc.free(tables);
    @memset(tables, .{});
    const merged = try alloc.alloc(Pair, BUCKETS);
    defer alloc.free(merged);
    @memset(merged, .{});

    var threads: [MAX_WORKERS]std.Thread = undefined;
    const chunk = CORPUS / nworkers;
    for (0..nworkers) |id| {
        const begin = id * chunk;
        const end = if (id == nworkers - 1) CORPUS else begin + chunk;
        threads[id] = try std.Thread.spawn(.{}, worker, .{ @as([]const u8, text[begin..end]), tables[id * BUCKETS ..][0..BUCKETS] });
    }
    for (0..nworkers) |id| threads[id].join();

    for (0..nworkers) |w| {
        for (tables[w * BUCKETS ..][0..BUCKETS]) |p| {
            if (p.count != 0) bump(merged, p.key, p.count);
        }
    }
    _ = summarize(merged);

    @memset(merged, .{});
    scan(&text, merged);
    const s = summarize(merged);
    const cs: i64 = s.total ^ (@as(i64, s.best) *% 1315423911) ^ @as(i64, s.best_key);
    emit(init.io, cs);
}
