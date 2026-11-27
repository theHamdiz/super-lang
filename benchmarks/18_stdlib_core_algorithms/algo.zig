// 18_stdlib_core_algorithms — one checksum from four std-style kernels:
// (A) KMP-DFA "regex" field extraction over 3000 generated log lines,
// (B) strict UTF-8 validation/counting over 60 documents (some corrupted),
// (C) SQL-ish filter / group-by / order-by-limit / join over 6000 rows x 24 queries,
// (D) HM-style type unification (union-find, iterative) over 8504 type nodes.
// Build: zig build-exe -OReleaseFast algo.zig
const std = @import("std");

const MOD: i64 = 1_000_000_007;
const NL = 3000;
const NDOC = 60;
const PERDOC = 200;
const NR = 6000;
const NQ = 24;
const NVARS = 2500;
const NCOMP = 6000;
const NCON = 5000;
const NTOT = NVARS + 4 + NCOMP;

const Ctx = struct {
    rng: i64 = 18018,
    cs: i64 = 0,
    fn next(self: *Ctx) i64 {
        self.rng = @mod(self.rng * 48271, 2147483647);
        return self.rng;
    }
    fn fold(self: *Ctx, v: i64) void {
        self.cs = @mod(self.cs * 131 + v, MOD);
    }
};

// ---------- (A) log extraction ----------
fn buildDfa(comptime needle: []const u8) [needle.len * 256]u16 {
    var dfa: [(needle.len * 256)]u16 = @splat(0);
    dfa[needle[0]] = 1;
    var x: usize = 0;
    for (1..needle.len) |j| {
        @memcpy(dfa[j * 256 ..][0..256], dfa[x * 256 ..][0..256]);
        dfa[j * 256 + needle[j]] = @intCast(j + 1);
        x = dfa[x * 256 + needle[j]];
    }
    return dfa;
}

fn extract(line: []const u8, dfa: []const u16, n: usize) i64 {
    var st: usize = 0;
    var i: usize = 0;
    while (i < line.len) {
        st = dfa[st * 256 + line[i]];
        i += 1;
        if (st == n) {
            var v: i64 = 0;
            var nd: usize = 0;
            while (i < line.len and line[i] >= '0' and line[i] <= '9') {
                v = v * 10 + (line[i] - '0');
                nd += 1;
                i += 1;
            }
            if (nd > 0) return v;
            st = 0;
        }
    }
    return -1;
}

fn partA(c: *Ctx) !void {
    const levels = [_][]const u8{ "INFO", "WARN", "ERROR", "DEBUG" };
    const codes = [_]i64{ 200, 201, 204, 301, 400, 401, 404, 500, 503 };
    const d_status = comptime buildDfa("status=");
    const d_took = comptime buildDfa("took=");
    const d_user = comptime buildDfa("user=");
    var cls: [8]i64 = @splat(0);
    var took_sum: i64 = 0;
    var took_max: i64 = 0;
    var took_n: i64 = 0;
    var user_sum: i64 = 0;
    var bad: i64 = 0;
    var total: i64 = 0;
    var buf: [160]u8 = undefined;
    for (0..NL) |i| {
        const mo = 1 + @mod(c.next(), 12);
        const dd = 1 + @mod(c.next(), 28);
        const hh = @mod(c.next(), 24);
        const mi = @mod(c.next(), 60);
        const ss = @mod(c.next(), 60);
        const lv: usize = @intCast(@mod(c.next(), 4));
        const svc = @mod(c.next(), 100);
        const uid = @mod(c.next(), 100000);
        const sidx: usize = @intCast(@mod(c.next(), 9));
        const ms = @mod(c.next(), 3000);
        var w = std.Io.Writer.fixed(&buf);
        try w.print("2026-{d:0>2}-{d:0>2} {d:0>2}:{d:0>2}:{d:0>2} {s} svc-{d:0>2} user={d}", .{ @as(u64, @intCast(mo)), @as(u64, @intCast(dd)), @as(u64, @intCast(hh)), @as(u64, @intCast(mi)), @as(u64, @intCast(ss)), levels[lv], @as(u64, @intCast(svc)), @as(u64, @intCast(uid)) });
        if (i % 13 == 5) {
            try w.writeAll(" status=abc");
        } else {
            try w.print(" status={d}", .{@as(u64, @intCast(codes[sidx]))});
        }
        if (i % 17 != 3) try w.print(" took={d}ms", .{@as(u64, @intCast(ms))});
        const line = w.buffered();
        const st = extract(line, &d_status, 7);
        const tk = extract(line, &d_took, 5);
        const us = extract(line, &d_user, 5);
        if (st < 0) bad += 1 else cls[@intCast(@divTrunc(st, 100))] += 1;
        if (tk >= 0) {
            took_sum += tk;
            took_n += 1;
            took_max = @max(took_max, tk);
        }
        user_sum += us;
        total += @as(i64, @intCast(line.len)) + 1;
    }
    for (cls) |v| c.fold(v);
    for ([_]i64{ took_sum, took_max, took_n, user_sum, bad, total }) |v| c.fold(v);
}

// ---------- (B) UTF-8 ----------
fn cont(b: u8) bool {
    return b & 0xC0 == 0x80;
}

fn utf8Valid(d: []const u8) bool {
    var i: usize = 0;
    const n = d.len;
    while (i < n) {
        const b0: i32 = d[i];
        if (b0 < 128) {
            i += 1;
        } else if (b0 < 194) {
            return false;
        } else if (b0 < 224) {
            if (i + 1 >= n or !cont(d[i + 1])) return false;
            i += 2;
        } else if (b0 < 240) {
            if (i + 2 >= n or !cont(d[i + 1]) or !cont(d[i + 2])) return false;
            const cp = (b0 - 224) * 4096 + (@as(i32, d[i + 1]) - 128) * 64 + (@as(i32, d[i + 2]) - 128);
            if (cp < 2048 or (cp >= 55296 and cp <= 57343)) return false;
            i += 3;
        } else if (b0 < 245) {
            if (i + 3 >= n or !cont(d[i + 1]) or !cont(d[i + 2]) or !cont(d[i + 3])) return false;
            const cp = (b0 - 240) * 262144 + (@as(i32, d[i + 1]) - 128) * 4096 + (@as(i32, d[i + 2]) - 128) * 64 + (@as(i32, d[i + 3]) - 128);
            if (cp < 65536 or cp > 1114111) return false;
            i += 4;
        } else return false;
    }
    return true;
}

fn utf8Count(d: []const u8) i64 {
    var n: i64 = 0;
    for (d) |b| {
        if (!cont(b)) n += 1;
    }
    return n;
}

fn encode(cp: i64, o: []u8) usize {
    if (cp < 128) {
        o[0] = @intCast(cp);
        return 1;
    }
    if (cp < 2048) {
        o[0] = @intCast(192 + @divTrunc(cp, 64));
        o[1] = @intCast(128 + @mod(cp, 64));
        return 2;
    }
    if (cp < 65536) {
        o[0] = @intCast(224 + @divTrunc(cp, 4096));
        o[1] = @intCast(128 + @mod(@divTrunc(cp, 64), 64));
        o[2] = @intCast(128 + @mod(cp, 64));
        return 3;
    }
    o[0] = @intCast(240 + @divTrunc(cp, 262144));
    o[1] = @intCast(128 + @mod(@divTrunc(cp, 4096), 64));
    o[2] = @intCast(128 + @mod(@divTrunc(cp, 64), 64));
    o[3] = @intCast(128 + @mod(cp, 64));
    return 4;
}

fn partB(c: *Ctx) void {
    const badb = [_]u8{ 0x80, 0xC0, 0xC1, 0xF5, 0xFF, 0xE0 };
    var doc: [PERDOC * 4]u8 = undefined;
    var valid_docs: i64 = 0;
    var total_cp: i64 = 0;
    for (0..NDOC) |d| {
        var n: usize = 0;
        for (0..PERDOC) |_| {
            const cat = @mod(c.next(), 4);
            const r = c.next();
            const cp: i64 = switch (cat) {
                0 => @mod(r, 128),
                1 => 128 + @mod(r, 1920),
                2 => 2048 + @mod(r, 53248),
                else => 65536 + @mod(r, 1048576),
            };
            n += encode(cp, doc[n..]);
        }
        if (d % 5 == 2) {
            const p: usize = @intCast(@mod(c.next(), @as(i64, @intCast(n))));
            const b: usize = @intCast(@mod(c.next(), 6));
            doc[p] = badb[b];
        }
        const ok = utf8Valid(doc[0..n]);
        const cnt: i64 = if (ok) utf8Count(doc[0..n]) else 0;
        c.fold(@intFromBool(ok));
        c.fold(cnt);
        c.fold(@intCast(n));
        valid_docs += @intFromBool(ok);
        total_cp += cnt;
    }
    c.fold(valid_docs);
    c.fold(total_cp);
}

// ---------- (C) SQL-ish ----------
fn partC(c: *Ctx) void {
    var dept: [NR]i64 = undefined;
    var age: [NR]i64 = undefined;
    var sal: [NR]i64 = undefined;
    var act: [NR]i64 = undefined;
    for (0..NR) |i| {
        dept[i] = @mod(c.next(), 16);
        age[i] = 18 + @mod(c.next(), 50);
        sal[i] = 30000 + @mod(c.next(), 90000);
        act[i] = @intFromBool(@mod(c.next(), 10) != 0);
    }
    for (0..NQ) |_| {
        const age_lo = 18 + @mod(c.next(), 30);
        const sal_max = 40000 + @mod(c.next(), 80000);
        const need = @mod(c.next(), 2);
        var cnt: [16]i64 = @splat(0);
        var sum: [16]i64 = @splat(0);
        var mx: [16]i64 = @splat(0);
        var taken: [16]bool = @splat(false);
        for (0..NR) |i| {
            if (age[i] >= age_lo and sal[i] < sal_max and act[i] >= need) {
                const g: usize = @intCast(dept[i]);
                cnt[g] += 1;
                sum[g] += sal[i];
                mx[g] = @max(mx[g], age[i]);
            }
        }
        for (0..3) |_| {
            var best: i64 = -1;
            for (0..16) |g| {
                if (cnt[g] >= 5 and !taken[g] and (best < 0 or sum[g] > sum[@intCast(best)])) best = @intCast(g);
            }
            if (best >= 0) {
                const b: usize = @intCast(best);
                taken[b] = true;
                c.fold(best);
                c.fold(cnt[b]);
                c.fold(sum[b]);
                c.fold(mx[b]);
                c.fold(@divTrunc(sum[b], (best + 1) * 1000));
            } else c.fold(999);
        }
    }
}

// ---------- (D) HM unification ----------
const Types = struct {
    kind: [NTOT]u8 = @splat(0),
    na: [NTOT]i32 = @splat(-1),
    nb: [NTOT]i32 = @splat(-1),
    parent: [NTOT]i32 = undefined,
    stk_a: [2 * NTOT + 16]i32 = undefined,
    stk_b: [2 * NTOT + 16]i32 = undefined,
    unions: i64 = 0,

    fn find(self: *Types, x0: i32) i32 {
        var x = x0;
        while (self.parent[@intCast(x)] != x) {
            const p = self.parent[@intCast(x)];
            self.parent[@intCast(x)] = self.parent[@intCast(p)];
            x = self.parent[@intCast(x)];
        }
        return x;
    }

    fn unify(self: *Types, x: i32, y: i32) bool {
        var sp: usize = 0;
        self.stk_a[sp] = x;
        self.stk_b[sp] = y;
        sp += 1;
        while (sp > 0) {
            sp -= 1;
            const rx = self.find(self.stk_a[sp]);
            const ry = self.find(self.stk_b[sp]);
            if (rx == ry) continue;
            const kx = self.kind[@intCast(rx)];
            const ky = self.kind[@intCast(ry)];
            if (kx == 0) {
                self.parent[@intCast(rx)] = ry;
                self.unions += 1;
            } else if (ky == 0) {
                self.parent[@intCast(ry)] = rx;
                self.unions += 1;
            } else if (kx == 1 or ky == 1) {
                if (kx == 1 and ky == 1 and self.na[@intCast(rx)] == self.na[@intCast(ry)]) {
                    self.parent[@intCast(ry)] = rx;
                    self.unions += 1;
                } else return false;
            } else {
                self.parent[@intCast(ry)] = rx;
                self.unions += 1;
                self.stk_a[sp] = self.na[@intCast(rx)];
                self.stk_b[sp] = self.na[@intCast(ry)];
                sp += 1;
                self.stk_a[sp] = self.nb[@intCast(rx)];
                self.stk_b[sp] = self.nb[@intCast(ry)];
                sp += 1;
            }
        }
        return true;
    }
};

fn partD(c: *Ctx, t: *Types) void {
    for (0..NTOT) |i| t.parent[i] = @intCast(i);
    for (0..4) |k| {
        t.kind[NVARS + k] = 1;
        t.na[NVARS + k] = @intCast(k);
    }
    for (NVARS + 4..NTOT) |i| {
        t.kind[i] = 2;
        t.na[i] = @intCast(@mod(c.next(), @as(i64, @intCast(i))));
        t.nb[i] = @intCast(@mod(c.next(), @as(i64, @intCast(i))));
    }
    var fails: i64 = 0;
    for (0..NCON) |_| {
        const tsel = @mod(c.next(), 2);
        const x: i32 = @intCast(if (tsel == 0) @mod(c.next(), NVARS) else @mod(c.next(), NTOT));
        const y: i32 = @intCast(@mod(c.next(), NTOT));
        if (!t.unify(x, y)) fails += 1;
    }
    c.fold(fails);
    c.fold(t.unions);
    var classes: i64 = 0;
    for (0..NTOT) |i| {
        const r: usize = @intCast(t.find(@intCast(i)));
        if (r == i) classes += 1;
        c.fold(@as(i64, @intCast(r)) * 3 + t.kind[r]);
    }
    c.fold(classes);
}

fn emit(io: std.Io, cs: i64) void {
    var buf: [64]u8 = undefined;
    const s = std.fmt.bufPrint(&buf, "CHECKSUM={d}\n", .{cs}) catch unreachable;
    std.Io.File.stdout().writeStreamingAll(io, s) catch {};
}

pub fn main(init: std.process.Init) !void {
    var c = Ctx{};
    try partA(&c);
    partB(&c);
    partC(&c);
    const t = try std.heap.page_allocator.create(Types);
    defer std.heap.page_allocator.destroy(t);
    t.* = .{};
    partD(&c, t);
    emit(init.io, c.cs);
}
