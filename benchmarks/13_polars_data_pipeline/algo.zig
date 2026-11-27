// 13_polars_data_pipeline — 64k orders: filter -> inner join -> group_by aggregates -> rolling-mean window
const std = @import("std");

const ROWS: comptime_int = 64000;
const CUSTOMERS: comptime_int = 2000;
const KNOWN: comptime_int = 1800;
const REGIONS: comptime_int = 16;
const WINDOW: comptime_int = 8;
const AMT_FLOOR: comptime_float = 20.0;
const QTY_FLOOR: comptime_int = 1;
const ROLL_SCALE: comptime_float = 1024.0;

var rng_state: u64 = 130013;
fn lcg() u64 {
    rng_state = (rng_state * 1103515245 + 12345) % 2147483648;
    return rng_state;
}

const Group = struct {
    tot: i64 = 0,
    n: i64 = 0,
    mx: i64 = 0,
    sum_amt: f64 = 0.0,
    seen: usize = 0,
    ring: [WINDOW]f64 = @splat(0.0),
};

var cust: [ROWS]usize = undefined;
var qty: [ROWS]i64 = undefined;
var amtq: [ROWS]i64 = undefined;
var amt: [ROWS]f64 = undefined;

pub fn main(init: std.process.Init) !void {
    for (0..ROWS) |i| {
        const c = lcg() / 256 % CUSTOMERS;
        const q: i64 = @intCast(1 + lcg() / 256 % 9);
        const p: i64 = @intCast(lcg() / 256 % 400);
        cust[i] = @intCast(c);
        qty[i] = q;
        amtq[i] = q * p;
        amt[i] = @as(f64, @floatFromInt(q * p)) * 0.25;
    }
    // dimension table: direct-address hash on customer id
    var region_of: [CUSTOMERS]i64 = @splat(-1);
    for (0..KNOWN) |c| region_of[c] = @intCast(lcg() / 256 % 16);

    var big: i64 = 0;
    var joined: i64 = 0;
    var roll_sum: i64 = 0;
    var roll_cnt: i64 = 0;
    var groups: [REGIONS]Group = @splat(.{});
    for (0..ROWS) |i| {
        if (!(amt[i] >= AMT_FLOOR and qty[i] > QTY_FLOOR)) continue;
        big += 1;
        const r = region_of[cust[i]];
        if (r < 0) continue;
        joined += 1;
        const g = &groups[@intCast(r)];
        g.tot += amtq[i];
        g.n += 1;
        g.sum_amt += amt[i];
        if (g.n == 1 or amtq[i] > g.mx) g.mx = amtq[i];
        g.ring[g.seen % WINDOW] = amt[i];
        g.seen += 1;
        if (g.seen >= WINDOW) {
            var s: f64 = 0.0;
            inline for (0..WINDOW) |k| s += g.ring[(g.seen - WINDOW + k) % WINDOW];
            roll_sum += @as(i64, @intFromFloat(@floor(s / @as(f64, WINDOW) * ROLL_SCALE)));
            roll_cnt += 1;
        }
    }
    var cs: i64 = big + joined * 3;
    for (groups, 0..) |g, r| {
        if (g.n == 0) continue;
        const avg = g.sum_amt / @as(f64, @floatFromInt(g.n));
        cs += (@as(i64, @intCast(r)) + 1) * (g.tot + 7 * g.n + @as(i64, @intFromFloat(@floor(avg * ROLL_SCALE))) + 13 * g.mx);
    }
    cs += roll_sum + 5 * roll_cnt;
    var buf: [64]u8 = undefined;
    var w = std.Io.File.stdout().writer(init.io, &buf);
    try w.interface.print("CHECKSUM={d}\n", .{cs});
    try w.interface.flush();
}
