// 06_mergesort — top-down merge sort of 50000 u64, verify + checksum
const std = @import("std");

const N = 50_000;

fn merge(a: []u64, tmp: []u64, mid: usize) void {
    const n = a.len;
    var i: usize = 0;
    var j: usize = mid;
    var k: usize = 0;
    while (i < mid and j < n) : (k += 1) {
        if (a[i] <= a[j]) {
            tmp[k] = a[i];
            i += 1;
        } else {
            tmp[k] = a[j];
            j += 1;
        }
    }
    if (i < mid) {
        @memcpy(tmp[k..][0 .. mid - i], a[i..mid]);
        k += mid - i;
    }
    if (j < n) @memcpy(tmp[k..][0 .. n - j], a[j..n]);
    @memcpy(a, tmp[0..n]);
}

fn mergesort(a: []u64, tmp: []u64) void {
    const n = a.len;
    if (n <= 1) return;
    const mid = n / 2;
    mergesort(a[0..mid], tmp[0..mid]);
    mergesort(a[mid..], tmp[mid..]);
    merge(a, tmp, mid);
}

fn emit(io: std.Io, cs: i64) void {
    var buf: [64]u8 = undefined;
    const s = std.fmt.bufPrint(&buf, "CHECKSUM={d}\n", .{cs}) catch unreachable;
    std.Io.File.stdout().writeStreamingAll(io, s) catch {};
}

pub fn main(init: std.process.Init) !void {
    const alloc = std.heap.page_allocator;
    const a = try alloc.alloc(u64, N);
    defer alloc.free(a);
    const tmp = try alloc.alloc(u64, N);
    defer alloc.free(tmp);
    var rng: u64 = 395799;
    for (a) |*v| {
        rng = (rng *% 1103515245 +% 12345) % 2147483648;
        v.* = rng & 0xffff_ffff;
    }
    mergesort(a, tmp);
    var cs: i64 = 0;
    var ok = true;
    for (a, 0..) |v, i| {
        cs ^= @as(i64, @intCast(v & 0xffff_ffff));
        cs = cs *% 131 +% @as(i64, @intCast(v >> 32));
        if (i > 0 and v < a[i - 1]) ok = false;
    }
    emit(init.io, cs ^ @as(i64, @intFromBool(ok)));
}
