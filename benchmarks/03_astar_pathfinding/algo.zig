// 03_astar — 64×64 grid, 40 queries, binary min-heap (lazy duplicates, as in C)
const std = @import("std");

const S: i32 = 64;
const CELLS = S * S;
const QUERIES = 40;
const INF: i32 = 0x3fffffff;
const DIRS = [4][2]i32{ .{ 1, 0 }, .{ -1, 0 }, .{ 0, 1 }, .{ 0, -1 } };

const Lcg = struct {
    s: u64,
    inline fn nextU(self: *Lcg) u32 {
        self.s = (self.s *% 1103515245 +% 12345) % 2147483648;
        return @truncate(self.s);
    }
    inline fn cell(self: *Lcg) usize {
        return @as(usize, self.nextU()) % CELLS;
    }
};

const Entry = struct { prio: i32, node: i32 };

var walkable: [CELLS]bool = undefined;
var gscore: [CELLS]i32 = undefined;
var came: [CELLS]i32 = undefined;
var open: [CELLS]bool = undefined;
var heap: [CELLS]Entry = undefined;
var hn: usize = 0;

fn push(node: i32, prio: i32) void {
    var i = hn;
    hn += 1;
    heap[i] = .{ .prio = prio, .node = node };
    while (i > 0) {
        const p = (i - 1) / 2;
        if (heap[i].prio >= heap[p].prio) break;
        std.mem.swap(Entry, &heap[i], &heap[p]);
        i = p;
    }
}

fn pop() i32 {
    const top = heap[0].node;
    hn -= 1;
    heap[0] = heap[hn];
    var i: usize = 0;
    while (true) {
        const l = 2 * i + 1;
        const r = l + 1;
        var sm = i;
        if (l < hn and heap[l].prio < heap[sm].prio) sm = l;
        if (r < hn and heap[r].prio < heap[sm].prio) sm = r;
        if (sm == i) break;
        std.mem.swap(Entry, &heap[i], &heap[sm]);
        i = sm;
    }
    return top;
}

inline fn heur(a: i32, b: i32) i32 {
    return @intCast(@abs(@rem(a, S) - @rem(b, S)) + @abs(@divTrunc(a, S) - @divTrunc(b, S)));
}

fn astar(start: i32, goal: i32) i32 {
    @memset(&gscore, INF);
    @memset(&open, false);
    @memset(&came, -1);
    hn = 0;
    gscore[@intCast(start)] = 0;
    push(start, heur(start, goal));
    open[@intCast(start)] = true;
    while (hn > 0) {
        const cur = pop();
        open[@intCast(cur)] = false;
        if (cur == goal) {
            var len: i32 = 0;
            var c = goal;
            while (c != start and c >= 0) {
                len += 1;
                c = came[@intCast(c)];
            }
            return len;
        }
        const cx = @rem(cur, S);
        const cy = @divTrunc(cur, S);
        for (DIRS) |d| {
            const nx = cx + d[0];
            const ny = cy + d[1];
            if (nx < 0 or ny < 0 or nx >= S or ny >= S) continue;
            const ni = ny * S + nx;
            if (!walkable[@intCast(ni)]) continue;
            const tent = gscore[@intCast(cur)] + 1;
            if (tent < gscore[@intCast(ni)]) {
                came[@intCast(ni)] = cur;
                gscore[@intCast(ni)] = tent;
                push(ni, tent + heur(ni, goal));
                open[@intCast(ni)] = true;
            }
        }
    }
    return -1;
}

fn emit(io: std.Io, cs: i64) void {
    var buf: [64]u8 = undefined;
    const s = std.fmt.bufPrint(&buf, "CHECKSUM={d}\n", .{cs}) catch unreachable;
    std.Io.File.stdout().writeStreamingAll(io, s) catch {};
}

pub fn main(init: std.process.Init) !void {
    var rng = Lcg{ .s = 10846167 };
    @memset(&walkable, true);
    for (0..CELLS / 5) |_| {
        const cell = rng.cell();
        if (cell != 0 and cell != CELLS - 1) walkable[cell] = false;
    }
    var cs: i64 = 0;
    for (0..QUERIES) |_| {
        var a = rng.cell();
        var b = rng.cell();
        while (!walkable[a]) a = rng.cell();
        while (!walkable[b]) b = rng.cell();
        cs += astar(@intCast(a), @intCast(b));
    }
    emit(init.io, cs);
}
