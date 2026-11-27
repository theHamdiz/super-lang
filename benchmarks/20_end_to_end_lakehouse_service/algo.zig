// 20_end_to_end_lakehouse_service — in-process pipeline:
// ingest JSONL customers + CSV order batches -> filter/join/group -> fixed-point GD linear fit on the
// per-product aggregates -> serve HTTP-like request bytes (bearer-token check via SHA-256, Merkle root of
// the group table in every response) -> checksum of response bytes.
// Build: zig build-exe -OReleaseFast algo.zig
const std = @import("std");
const Sha256 = std.crypto.hash.sha2.Sha256;

const MOD: i64 = 1_000_000_007;
const NCUST = 500;
const NPROD = 50;
const NREG = 8;
const NTIER = 4;
const NGRP = NREG * NTIER;
const BATCHES = 6;
const ROWS_PB = 4000;
const NREQ = 3000;
const NCLIENT = 8;
const ITERS = 400;

var rng: i64 = 20020;
fn nextRand() i64 {
    rng = @rem(rng * 48271, 2147483647);
    return rng;
}
fn sdiv(a: i64, d: i64) i64 {
    return if (a >= 0) @divTrunc(a, d) else -@divTrunc(-a, d);
}

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

fn sha256(msg: []const u8) [32]u8 {
    var out: [32]u8 = undefined;
    Sha256.hash(msg, &out, .{});
    return out;
}

/// Fixed-capacity byte writer.
const Out = struct {
    b: []u8,
    n: usize = 0,

    fn str(self: *Out, s: []const u8) void {
        @memcpy(self.b[self.n..][0..s.len], s);
        self.n += s.len;
    }
    fn int(self: *Out, v: i64) void {
        const s = std.fmt.bufPrint(self.b[self.n..], "{d}", .{v}) catch unreachable;
        self.n += s.len;
    }
    fn byte(self: *Out, c: u8) void {
        self.b[self.n] = c;
        self.n += 1;
    }
    fn hex(self: *Out, d: []const u8) void {
        const digits = "0123456789abcdef";
        for (d) |x| {
            self.byte(digits[x >> 4]);
            self.byte(digits[x & 15]);
        }
    }
    fn slice(self: *const Out) []const u8 {
        return self.b[0..self.n];
    }
};

/// Parse the decimal run at `p.*` (advancing it); -1 when there are no digits.
fn scanInt(b: []const u8, p: *usize) i64 {
    var v: i64 = 0;
    var have = false;
    while (p.* < b.len and b[p.*] >= '0' and b[p.*] <= '9') {
        v = v * 10 + (b[p.*] - '0');
        p.* += 1;
        have = true;
    }
    return if (have) v else -1;
}
fn isDigit(c: u8) bool {
    return c >= '0' and c <= '9';
}
fn indexOf(b: []const u8, pat: []const u8, from: usize) ?usize {
    if (b.len < pat.len) return null;
    var i = from;
    while (i + pat.len <= b.len) : (i += 1) {
        if (std.mem.eql(u8, b[i..][0..pat.len], pat)) return i;
    }
    return null;
}

var cbuf: [NCUST * 48 + 1]u8 = undefined;
var obuf: [ROWS_PB * 40 + 160]u8 = undefined;
var region_of: [NCUST]i64 = undefined;
var tier_of: [NCUST]i64 = undefined;
var grp_cnt: [NGRP]i64 = @splat(0);
var grp_qty: [NGRP]i64 = @splat(0);
var grp_rev: [NGRP]i64 = @splat(0);
var prod_cnt: [NPROD]i64 = @splat(0);
var prod_rev: [NPROD]i64 = @splat(0);
var prod_price: [NPROD]i64 = @splat(0);
var reg_prod_rev: [(NREG * NPROD)]i64 = @splat(0);
var pred_cents: [NPROD]i64 = @splat(0);

fn emit(io: std.Io, cs: i64) void {
    var buf: [64]u8 = undefined;
    const s = std.fmt.bufPrint(&buf, "CHECKSUM={d}\n", .{cs}) catch unreachable;
    std.Io.File.stdout().writeStreamingAll(io, s) catch {};
}

pub fn main(init: std.process.Init) !void {
    var cs: i64 = 0;
    const fold = struct {
        fn f(c: *i64, v: i64) void {
            c.* = @rem(c.* * 131 + v, MOD);
        }
    }.f;

    // ---- ingest customers (JSONL) ----
    var cw = Out{ .b = &cbuf };
    for (0..NCUST) |i| {
        const id: i64 = @intCast(i * 7919 % NCUST);
        const r = @rem(nextRand(), NREG);
        const t = @rem(nextRand(), NTIER);
        cw.str("{\"id\":");
        cw.int(id);
        cw.str(",\"region\":");
        cw.int(r);
        cw.str(",\"tier\":");
        cw.int(t);
        cw.str("}\n");
    }
    const clen = cw.n;
    cbuf[clen] = 0;
    @memset(&region_of, -1);
    @memset(&tier_of, -1);
    var ncust: i64 = 0;
    var p: usize = 0;
    while (p < clen) {
        while (p < clen and !isDigit(cbuf[p])) p += 1;
        const id = scanInt(&cbuf, &p);
        while (!isDigit(cbuf[p])) p += 1;
        const r = scanInt(&cbuf, &p);
        while (!isDigit(cbuf[p])) p += 1;
        const t = scanInt(&cbuf, &p);
        region_of[@intCast(id)] = r;
        tier_of[@intCast(id)] = t;
        ncust += 1;
        while (p < clen and cbuf[p] != '\n') p += 1;
        p += 1;
    }

    // ---- ingest + filter/join/group order batches (CSV) ----
    var oid: i64 = 0;
    var rows: i64 = 0;
    var null_cust: i64 = 0;
    var bad_qty: i64 = 0;
    var valid: i64 = 0;
    for (0..BATCHES) |_| {
        var ow = Out{ .b = &obuf };
        ow.str("order_id,cust_id,product,qty,price_cents\n");
        for (0..ROWS_PB) |_| {
            const nf = @rem(nextRand(), 100);
            const c = @rem(nextRand(), NCUST);
            const pr = @rem(nextRand(), NPROD);
            const q = @rem(nextRand(), 9);
            const price = 100 + pr * 37 + @rem(nextRand(), 200);
            ow.int(oid);
            oid += 1;
            ow.byte(',');
            if (nf != 0) ow.int(c);
            ow.byte(',');
            ow.int(pr);
            ow.byte(',');
            ow.int(q);
            ow.byte(',');
            ow.int(price);
            ow.byte('\n');
        }
        const olen = ow.n;
        obuf[olen] = 0;
        var pos: usize = 0;
        while (obuf[pos] != '\n') pos += 1;
        pos += 1;
        while (pos < olen) {
            var f: [5]i64 = undefined;
            for (&f) |*v| {
                v.* = scanInt(&obuf, &pos);
                pos += 1;
            }
            rows += 1;
            if (f[1] < 0) {
                null_cust += 1;
                continue;
            }
            if (f[3] <= 0) {
                bad_qty += 1;
                continue;
            }
            valid += 1;
            const rev = f[3] * f[4];
            const reg = region_of[@intCast(f[1])];
            const g: usize = @intCast(reg * NTIER + tier_of[@intCast(f[1])]);
            grp_cnt[g] += 1;
            grp_qty[g] += f[3];
            grp_rev[g] += rev;
            const pd: usize = @intCast(f[2]);
            prod_cnt[pd] += 1;
            prod_rev[pd] += rev;
            prod_price[pd] += f[4];
            reg_prod_rev[@as(usize, @intCast(reg)) * NPROD + pd] += rev;
        }
    }

    // ---- fit y = w*x + b (w in Q16) by gradient descent on per-product aggregates ----
    var xs: [NPROD]i64 = undefined;
    var ys: [NPROD]i64 = undefined;
    var n: usize = 0;
    for (0..NPROD) |pd| {
        if (prod_cnt[pd] > 0) {
            xs[n] = @divTrunc(@divTrunc(prod_price[pd], prod_cnt[pd]), 16);
            ys[n] = @divTrunc(@divTrunc(prod_rev[pd], prod_cnt[pd]), 16);
            n += 1;
        }
    }
    const ni: i64 = @intCast(n);
    var w: i64 = 0;
    var bias: i64 = 0;
    for (0..ITERS) |_| {
        var gw: i64 = 0;
        var gb: i64 = 0;
        for (0..n) |i| {
            const err = sdiv(w * xs[i], 65536) + bias - ys[i];
            gw += err * xs[i];
            gb += err;
        }
        w -= sdiv(gw * 65536, ni * 8000);
        bias -= sdiv(gb, ni * 4);
    }
    var loss: i64 = 0;
    for (0..n) |i| {
        const err = sdiv(w * xs[i], 65536) + bias - ys[i];
        loss += err * err;
    }
    for (0..NPROD) |pd| {
        const x: i64 = if (prod_cnt[pd] > 0) @divTrunc(@divTrunc(prod_price[pd], prod_cnt[pd]), 16) else 0;
        const y = sdiv(w * x, 65536) + bias;
        pred_cents[pd] = @max(y, 0) * 16;
    }

    // ---- merkle root over the region x tier group table ----
    var level: [NGRP][32]u8 = undefined;
    for (0..NGRP) |g| {
        var leaf: [32]u8 = undefined;
        const vals = [4]i64{ @intCast(g), grp_cnt[g], grp_qty[g], grp_rev[g] };
        for (vals, 0..) |v, k| std.mem.writeInt(u64, leaf[k * 8 ..][0..8], @intCast(v), .big);
        level[g] = sha256(&leaf);
    }
    var m: usize = NGRP;
    while (m > 1) : (m /= 2) {
        for (0..m / 2) |i| {
            var cat: [64]u8 = undefined;
            @memcpy(cat[0..32], &level[2 * i]);
            @memcpy(cat[32..64], &level[2 * i + 1]);
            level[i] = sha256(&cat);
        }
    }
    const root = level[0];
    var root_hex_buf: [64]u8 = undefined;
    var rh = Out{ .b = &root_hex_buf };
    rh.hex(&root);

    // ---- client tokens ----
    var tokens: [NCLIENT][16]u8 = undefined;
    for (0..NCLIENT) |k| {
        var kb: [8]u8 = undefined;
        const key = std.fmt.bufPrint(&kb, "key-{d}", .{k}) catch unreachable;
        const d = sha256(key);
        var tw = Out{ .b = &tokens[k] };
        tw.hex(d[0..8]);
    }

    // ---- serve ----
    var ok200: i64 = 0;
    var un401: i64 = 0;
    var nf404: i64 = 0;
    var total_bytes: i64 = 0;
    var resp_cs: i64 = 0;
    var reqb: [512]u8 = undefined;
    var bodyb: [512]u8 = undefined;
    var respb: [1024]u8 = undefined;
    for (0..NREQ) |_| {
        const k: usize = @intCast(@rem(nextRand(), NCLIENT));
        const pq = @rem(nextRand(), 60);
        const rg = @rem(nextRand(), NREG);
        const bad = @rem(nextRand(), 100);
        const at: usize = @intCast(@rem(nextRand(), 16));
        var rq = Out{ .b = &reqb };
        rq.str("GET /v1/product/");
        rq.int(pq);
        rq.str("?region=");
        rq.int(rg);
        rq.str(" HTTP/1.1\r\nHost: lake\r\nX-Client: ");
        rq.int(@intCast(k));
        rq.str("\r\nAuthorization: Bearer ");
        const tpos = rq.n;
        rq.str(&tokens[k]);
        if (bad < 8) reqb[tpos + at] = 'z';
        rq.str("\r\n\r\n");
        const req = rq.slice();
        // server side: parse request bytes
        var status: i64 = 400;
        var pp: i64 = -1;
        var rr: i64 = -1;
        if (req.len > 16 and std.mem.startsWith(u8, req, "GET /v1/product/")) {
            var q: usize = 16;
            pp = scanInt(req, &q);
            q += 8;
            rr = scanInt(req, &q);
            const xc = indexOf(req, "X-Client: ", q);
            const bt = indexOf(req, "Bearer ", q);
            if (xc != null and bt != null) {
                var a: usize = xc.? + 10;
                const kk = scanInt(req, &a);
                const good = kk >= 0 and kk < NCLIENT and bt.? + 7 + 16 <= req.len and
                    std.mem.eql(u8, req[bt.? + 7 ..][0..16], &tokens[@intCast(kk)]);
                status = if (good)
                    (if (pp >= 0 and pp < NPROD and rr >= 0 and rr < NREG and prod_cnt[@intCast(pp)] > 0) @as(i64, 200) else 404)
                else
                    401;
            }
        }
        var bw = Out{ .b = &bodyb };
        var head: []const u8 = "HTTP/1.1 400 Bad Request";
        if (status == 200) {
            const pi: usize = @intCast(pp);
            bw.str("{\"product\":");
            bw.int(pp);
            bw.str(",\"region\":");
            bw.int(rr);
            bw.str(",\"orders\":");
            bw.int(prod_cnt[pi]);
            bw.str(",\"revenue\":");
            bw.int(prod_rev[pi]);
            bw.str(",\"region_revenue\":");
            bw.int(reg_prod_rev[@as(usize, @intCast(rr)) * NPROD + pi]);
            bw.str(",\"forecast\":");
            bw.int(pred_cents[pi]);
            bw.str(",\"root\":\"");
            bw.str(rh.slice());
            bw.str("\"}");
            ok200 += 1;
            head = "HTTP/1.1 200 OK";
        } else if (status == 404) {
            bw.str("{\"error\":\"not found\"}");
            nf404 += 1;
            head = "HTTP/1.1 404 Not Found";
        } else if (status == 401) {
            bw.str("{\"error\":\"unauthorized\"}");
            un401 += 1;
            head = "HTTP/1.1 401 Unauthorized";
        } else {
            bw.str("{\"error\":\"bad request\"}");
        }
        var rw = Out{ .b = &respb };
        rw.str(head);
        rw.str("\r\nContent-Length: ");
        rw.int(@intCast(bw.n));
        rw.str("\r\n\r\n");
        rw.str(bw.slice());
        total_bytes += @intCast(rw.n);
        resp_cs = @rem(resp_cs * 131 + crc32(rw.slice()), MOD);
    }

    // ---- checksum ----
    for ([_]i64{ ncust, rows, null_cust, bad_qty, valid }) |v| fold(&cs, v);
    for (0..NGRP) |g| {
        fold(&cs, grp_cnt[g]);
        fold(&cs, grp_qty[g]);
        fold(&cs, grp_rev[g]);
    }
    for (0..NPROD) |pd| {
        fold(&cs, prod_cnt[pd]);
        fold(&cs, prod_rev[pd]);
    }
    for ([_]i64{ w, bias, loss }) |v| fold(&cs, v);
    for (root) |b| fold(&cs, b);
    for ([_]i64{ ok200, un401, nf404, total_bytes, resp_cs }) |v| fold(&cs, v);
    emit(init.io, cs);
}
