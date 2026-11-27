// 14_http_router_throughput - 30k simulated HTTP requests: parse bytes, radix route match,
// HMAC-SHA256 bearer-token check, JSON body validation, status-code checksum
const std = @import("std");
const HmacSha256 = std.crypto.auth.hmac.sha2.HmacSha256;

const NREQ: comptime_int = 30000;
const NOW: comptime_int = 1700000000;
const MAXNODES: comptime_int = 64;
const HEX = "0123456789abcdef";
/// Routes 1..7 and 10 need a bearer token (comptime table → LoC + Cycle).
const AUTH = [_]bool{ false, true, true, true, true, true, true, true, false, false, true };

var KEY: [32]u8 = undefined;
var rng: u64 = 140014;

fn draw(m: i64) i64 {
    rng = (rng * 1103515245 + 12345) % 2147483648;
    return @intCast(rng / 256 % @as(u64, @intCast(m)));
}

fn macOf(msg: []const u8) [32]u8 {
    var out: [32]u8 = undefined;
    HmacSha256.create(&out, msg, &KEY);
    return out;
}

const Buf = struct {
    d: [512]u8 = undefined,
    n: usize = 0,

    fn lit(self: *Buf, s: []const u8) void {
        @memcpy(self.d[self.n..][0..s.len], s);
        self.n += s.len;
    }
    fn byte(self: *Buf, c: u8) void {
        self.d[self.n] = c;
        self.n += 1;
    }
    fn dec(self: *Buf, v: i64) void {
        var tmp: [24]u8 = undefined;
        const s = std.fmt.bufPrint(&tmp, "{d}", .{v}) catch unreachable;
        self.lit(s);
    }
    fn chars(self: *Buf, cnt: i64) void {
        var i: i64 = 0;
        while (i < cnt) : (i += 1) self.byte(@intCast('a' + draw(26)));
    }
    fn id(self: *Buf) void {
        const bad = draw(50);
        const idv = 1 + draw(99999);
        if (bad == 0) self.lit("abc") else self.dec(idv);
    }
    fn bytes(self: *const Buf) []const u8 {
        return self.d[0..self.n];
    }
};

fn buildRequest(r: *Buf) void {
    r.n = 0;
    var path = Buf{};
    var body = Buf{};
    var tok = Buf{};
    var method: usize = 0;
    var has_body = false;
    const a = draw(100);
    if (a < 5) {
        path.lit("/health");
    } else if (a < 15) {
        path.lit("/users");
    } else if (a < 35) {
        path.lit("/users/");
        path.id();
    } else if (a < 45) {
        path.lit("/users/");
        path.id();
        path.lit("/orders");
    } else if (a < 55) {
        method = 1;
        has_body = true;
        path.lit("/orders");
    } else if (a < 65) {
        path.lit("/orders/");
        path.id();
    } else if (a < 70) {
        method = 2;
        has_body = true;
        path.lit("/orders/");
        path.id();
    } else if (a < 75) {
        method = 3;
        path.lit("/orders/");
        path.id();
    } else if (a < 85) {
        path.lit("/static/a");
        path.dec(draw(10));
        path.lit("/f");
        path.dec(draw(100));
        path.lit(".js");
    } else if (a < 90) {
        method = 1;
        has_body = true;
        path.lit("/auth/login");
    } else if (a < 95) {
        path.lit("/v1/items/");
        path.id();
        path.lit("/tags/t");
        path.dec(draw(50));
    } else if (a < 98) {
        path.lit("/nope/");
        path.id();
    } else {
        method = 3;
        path.lit("/users");
    }

    const t = draw(20);
    const uid = 1 + draw(9999);
    const ex = draw(1000);
    const has_tok = t >= 2;
    if (has_tok) {
        const exp: i64 = if (t == 4) NOW - 1 - ex else NOW + 1000 + ex;
        var sb = Buf{};
        sb.dec(uid);
        sb.byte('.');
        sb.dec(exp);
        var mac = macOf(sb.bytes());
        if (t < 4) mac[0] ^= 1;
        tok.lit(sb.bytes());
        tok.byte('.');
        for (mac[0..4]) |m| {
            tok.byte(HEX[m >> 4]);
            tok.byte(HEX[m & 15]);
        }
    }

    if (has_body) {
        const v = draw(10);
        const ws = draw(2);
        const sp: []const u8 = if (ws == 1) " " else "";
        if (a >= 45 and a < 55) {
            var user = 1 + draw(9999);
            var qty = 1 + draw(100);
            if (v == 0) qty = 0;
            if (v == 3) user = 10000;
            body.lit("{\"user\":");
            body.lit(sp);
            body.dec(user);
            body.lit(",");
            body.lit(sp);
            body.lit("\"qty\":");
            body.lit(sp);
            body.dec(qty);
            var sku = Buf{};
            sku.chars(4);
            if (v != 1) {
                body.lit(",");
                body.lit(sp);
                body.lit("\"sku\":");
                body.lit(sp);
                body.byte('"');
                body.lit(sku.bytes());
                body.byte('"');
            }
            body.byte('}');
            if (v == 2) body.n -= 1;
        } else if (a >= 65 and a < 70) {
            var qty = 1 + draw(100);
            const nl = 3 + draw(5);
            if (v == 0) qty = 101;
            body.lit("{");
            if (v != 1) {
                body.lit("\"qty\":");
                body.lit(sp);
                body.dec(qty);
                body.lit(",");
                body.lit(sp);
            }
            body.lit("\"note\":");
            body.lit(sp);
            body.byte('"');
            body.chars(nl);
            body.lit("\"}");
            if (v == 2) body.n -= 1;
        } else {
            const ul = 4 + draw(6);
            var un = Buf{};
            un.chars(ul);
            const pwl = 6 + draw(7);
            var pw = Buf{};
            pw.chars(pwl);
            if (v == 0) pw.n = 0;
            body.lit("{");
            if (v != 1) {
                body.lit("\"user\":");
                body.lit(sp);
                body.byte('"');
                body.lit(un.bytes());
                body.lit("\",");
                body.lit(sp);
            }
            body.lit("\"pass\":");
            body.lit(sp);
            body.byte('"');
            body.lit(pw.bytes());
            body.lit("\"}");
            if (v == 2) body.byte('x');
        }
    }

    const methods = [_][]const u8{ "GET ", "POST ", "PUT ", "DELETE " };
    r.lit(methods[method]);
    r.lit(path.bytes());
    r.lit(" HTTP/1.1\r\nHost: api.local\r\n");
    if (has_tok) {
        r.lit("Authorization: Bearer ");
        r.lit(tok.bytes());
        r.lit("\r\n");
    }
    if (has_body) {
        r.lit("Content-Length: ");
        r.dec(@intCast(body.n));
        r.lit("\r\n");
    }
    r.lit("\r\n");
    r.lit(body.bytes());
}

// ---- radix router ----
const Node = struct {
    kind: u8 = 0, // 0 static, 1 param, 2 wildcard
    lit: [16]u8 = undefined,
    len: usize = 0,
    first: i32 = -1,
    next: i32 = -1,
    h: [4]i32 = .{ -1, -1, -1, -1 },
};

var nodes: [MAXNODES]Node = undefined;
var nn: usize = 1;

fn addRoute(tpl: []const u8, method: usize, rid: i32) void {
    var cur: usize = 0;
    var it = std.mem.splitScalar(u8, tpl[1..], '/');
    while (it.next()) |seg| {
        const kind: u8 = if (seg[0] == ':') 1 else if (seg[0] == '*') 2 else 0;
        var found: i32 = -1;
        var c = nodes[cur].first;
        while (c >= 0 and found < 0) {
            const nd = &nodes[@intCast(c)];
            if (nd.kind == kind and (kind != 0 or std.mem.eql(u8, nd.lit[0..nd.len], seg))) found = c;
            c = nd.next;
        }
        if (found < 0) {
            var nd = Node{ .kind = kind, .next = nodes[cur].first };
            if (kind == 0) {
                @memcpy(nd.lit[0..seg.len], seg);
                nd.len = seg.len;
            }
            nodes[nn] = nd;
            found = @intCast(nn);
            nn += 1;
            nodes[cur].first = found;
        }
        cur = @intCast(found);
    }
    nodes[cur].h[method] = rid;
}


fn parseDigits(b: []const u8, p: usize, e: usize) i64 {
    if (e <= p or e - p > 10) return -1;
    var v: i64 = 0;
    for (b[p..e]) |c| {
        if (c < '0' or c > '9') return -1;
        v = v * 10 + (c - '0');
    }
    return v;
}

fn verifyToken(b: []const u8, off: usize, len: usize) i64 {
    const e = off + len;
    var d1: usize = std.math.maxInt(usize);
    var d2: usize = std.math.maxInt(usize);
    var i = off;
    while (i < e) : (i += 1) {
        if (b[i] == '.') {
            if (d1 == std.math.maxInt(usize)) {
                d1 = i;
            } else if (d2 == std.math.maxInt(usize)) {
                d2 = i;
            } else return -1;
        }
    }
    if (d1 == std.math.maxInt(usize) or d2 == std.math.maxInt(usize) or e - d2 - 1 != 8) return -1;
    const u = parseDigits(b, off, d1);
    const ex = parseDigits(b, d1 + 1, d2);
    if (u < 0 or ex < 0) return -1;
    const mac = macOf(b[off..d2]);
    var k: usize = 0;
    while (k < 4) : (k += 1) {
        if (b[d2 + 1 + 2 * k] != HEX[mac[k] >> 4] or b[d2 + 2 + 2 * k] != HEX[mac[k] & 15]) return -1;
    }
    if (ex <= NOW) return -1;
    return u;
}

fn skipWs(b: []const u8, p0: usize, e: usize) usize {
    var p = p0;
    while (p < e and (b[p] == ' ' or b[p] == '\t' or b[p] == '\n' or b[p] == '\r')) p += 1;
    return p;
}

fn keyId(k: []const u8) usize {
    if (std.mem.eql(u8, k, "user")) return 1;
    if (std.mem.eql(u8, k, "qty")) return 2;
    if (std.mem.eql(u8, k, "sku")) return 3;
    if (std.mem.eql(u8, k, "note")) return 4;
    if (std.mem.eql(u8, k, "pass")) return 5;
    return 0;
}

const Field = struct { ty: u8 = 0, val: i64 = 0 }; // ty: 0 absent, 1 int, 2 str (val = length)
const Fields = [6]Field;

fn parseJson(b: []const u8, off: usize, len: usize, f: *Fields) bool {
    const e = off + len;
    f.* = @as(Fields, @splat(.{}));
    var p = skipWs(b, off, e);
    if (p >= e or b[p] != '{') return false;
    p = skipWs(b, p + 1, e);
    if (p < e and b[p] == '}') {
        p = skipWs(b, p + 1, e);
        return p == e;
    }
    while (true) {
        if (p >= e or b[p] != '"') return false;
        p += 1;
        const ks = p;
        while (p < e and b[p] != '"') {
            if (b[p] == '\\') return false;
            p += 1;
        }
        if (p >= e) return false;
        const kid = keyId(b[ks..p]);
        p = skipWs(b, p + 1, e);
        if (p >= e or b[p] != ':') return false;
        p = skipWs(b, p + 1, e);
        if (p >= e) return false;
        var ty: u8 = 0;
        var val: i64 = 0;
        if (b[p] == '"') {
            p += 1;
            const vs = p;
            while (p < e and b[p] != '"') {
                if (b[p] == '\\') return false;
                p += 1;
            }
            if (p >= e) return false;
            ty = 2;
            val = @intCast(p - vs);
            p += 1;
        } else {
            var neg = false;
            if (b[p] == '-') {
                neg = true;
                p += 1;
            }
            const ds = p;
            while (p < e and b[p] >= '0' and b[p] <= '9') p += 1;
            const v = parseDigits(b, ds, p);
            if (v < 0) return false;
            ty = 1;
            val = if (neg) -v else v;
        }
        if (kid > 0) f[kid] = .{ .ty = ty, .val = val };
        p = skipWs(b, p, e);
        if (p >= e) return false;
        if (b[p] == ',') {
            p = skipWs(b, p + 1, e);
            continue;
        }
        if (b[p] == '}') {
            p += 1;
            break;
        }
        return false;
    }
    p = skipWs(b, p, e);
    return p == e;
}

fn hasInt(f: *const Fields, i: usize, lo: i64, hi: i64) bool {
    return f[i].ty == 1 and f[i].val >= lo and f[i].val <= hi;
}
fn hasStr(f: *const Fields, i: usize, lo: i64, hi: i64) bool {
    return f[i].ty == 2 and f[i].val >= lo and f[i].val <= hi;
}

fn bodyValid(rid: i32, b: []const u8, off: usize, len: usize) bool {
    var f: Fields = undefined;
    if (!parseJson(b, off, len, &f)) return false;
    if (rid == 4) return hasInt(&f, 1, 1, 9999) and hasInt(&f, 2, 1, 100) and hasStr(&f, 3, 4, 4);
    if (rid == 6) return hasInt(&f, 2, 1, 100) and (f[4].ty == 0 or hasStr(&f, 4, 0, 1000));
    return hasStr(&f, 1, 1, 64) and hasStr(&f, 5, 1, 64);
}

var pid_out: i64 = 0;

fn handle(b: []const u8) i64 {
    pid_out = 0;
    const n = b.len;
    var method: usize = 0;
    var p: usize = 0;
    if (std.mem.startsWith(u8, b, "GET ")) {
        method = 0;
        p = 4;
    } else if (std.mem.startsWith(u8, b, "POST ")) {
        method = 1;
        p = 5;
    } else if (std.mem.startsWith(u8, b, "PUT ")) {
        method = 2;
        p = 4;
    } else if (std.mem.startsWith(u8, b, "DELETE ")) {
        method = 3;
        p = 7;
    } else return 400;
    const po = p;
    while (p < n and b[p] != ' ') p += 1;
    const pe = p;
    while (p < n and b[p] != '\r') p += 1;
    p += 2;

    var tok_off: usize = std.math.maxInt(usize);
    var tok_len: usize = 0;
    var clen: i64 = -1;
    while (p < n) {
        if (b[p] == '\r') {
            p += 2;
            break;
        }
        var le = p;
        while (le < n and b[le] != '\r') le += 1;
        if (std.mem.startsWith(u8, b[p..], "Authorization: Bearer ")) {
            tok_off = p + 22;
            tok_len = le - (p + 22);
        } else if (std.mem.startsWith(u8, b[p..], "Content-Length: ")) {
            clen = parseDigits(b, p + 16, le);
        }
        p = le + 2;
    }
    const body_off = p;
    const body_len = n - p;
    if (clen >= 0 and clen != @as(i64, @intCast(body_len))) return 400;

    const plen = pe - po;
    var cur: usize = 0;
    var found = true;
    var nparam: u32 = 0;
    var bad_id = false;
    var pid: i64 = -1;
    if (plen < 1 or b[po] != '/') found = false;
    var i: usize = 1;
    while (found and i <= plen) {
        var e = i;
        while (e < plen and b[po + e] != '/') e += 1;
        if (e == i) {
            found = false;
            break;
        }
        const seg = b[po + i .. po + e];
        var st: i32 = -1;
        var pm: i32 = -1;
        var wc: i32 = -1;
        var c = nodes[cur].first;
        while (c >= 0) {
            const nd = &nodes[@intCast(c)];
            if (nd.kind == 0) {
                if (std.mem.eql(u8, nd.lit[0..nd.len], seg)) {
                    st = c;
                    break;
                }
            } else if (nd.kind == 1) {
                pm = c;
            } else {
                wc = c;
            }
            c = nd.next;
        }
        if (st >= 0) {
            cur = @intCast(st);
        } else if (pm >= 0) {
            cur = @intCast(pm);
            if (nparam == 0) {
                pid = parseDigits(b, po + i, po + e);
                if (pid < 0) bad_id = true;
            }
            nparam += 1;
        } else if (wc >= 0) {
            cur = @intCast(wc);
            break;
        } else {
            found = false;
            break;
        }
        i = e + 1;
    }
    if (!found) return 404;
    const hs = nodes[cur].h;
    const rid = hs[method];
    if (rid < 0) {
        return if (hs[0] >= 0 or hs[1] >= 0 or hs[2] >= 0 or hs[3] >= 0) 405 else 404;
    }
    pid_out = if (bad_id or pid < 0) 0 else pid;

    if (AUTH[@intCast(rid)]) {
        if (tok_off == std.math.maxInt(usize)) return 401;
        const uid = verifyToken(b, tok_off, tok_len);
        if (uid < 0) return 401;
        if ((rid == 6 or rid == 7) and @mod(uid, 13) == 0) return 403;
    }
    if (bad_id) return 400;
    if ((rid == 4 or rid == 6 or rid == 9) and !bodyValid(rid, b, body_off, body_len)) return 400;
    if (rid == 2 and pid > 90000) return 404;
    if (rid == 4) return 201;
    if (rid == 5 and @mod(pid, 7) == 0) return 404;
    if (rid == 7) return 204;
    return 200;
}

pub fn main(init: std.process.Init) !void {
    for (&KEY, 0..) |*k, i| k.* = @intCast((i * 37 + 11) % 256);
    nodes[0] = Node{};
    addRoute("/health", 0, 0);
    addRoute("/users", 0, 1);
    addRoute("/users/:id", 0, 2);
    addRoute("/users/:id/orders", 0, 3);
    addRoute("/orders", 1, 4);
    addRoute("/orders/:id", 0, 5);
    addRoute("/orders/:id", 2, 6);
    addRoute("/orders/:id", 3, 7);
    addRoute("/static/*path", 0, 8);
    addRoute("/auth/login", 1, 9);
    addRoute("/v1/items/:id/tags/:tag", 0, 10);

    var cs: i64 = 0;
    var sum_pid: i64 = 0;
    var ok2xx: i64 = 0;
    var by_status: [600]i64 = @splat(0);
    var req = Buf{};
    var i: i64 = 0;
    while (i < NREQ) : (i += 1) {
        buildRequest(&req);
        const st = handle(req.bytes());
        by_status[@intCast(st)] += 1;
        cs += st * (@mod(i, 97) + 1);
        sum_pid += pid_out;
        if (st < 300) ok2xx += 1;
    }
    var mix: i64 = 0;
    for (by_status, 0..) |c, s| {
        const si: i64 = @intCast(s);
        mix += c * (@mod(si, 53) + 1) * (@divTrunc(si, 100) + 1);
    }
    cs += sum_pid + 5 * ok2xx + 7 * mix;

    var buf: [64]u8 = undefined;
    var w = std.Io.File.stdout().writer(init.io, &buf);
    try w.interface.print("CHECKSUM={d}\n", .{cs});
    try w.interface.flush();
}
