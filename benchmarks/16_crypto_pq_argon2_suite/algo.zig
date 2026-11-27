// 16_crypto_pq_argon2_suite — Argon2id-like KDF (m=8KiB t=2 p=1) x32, SHA-256 Merkle of tags,
// toy lattice negacyclic NTT poly-mul (q=12289, n=256)
// Build: zig build-exe -OReleaseFast algo.zig
const std = @import("std");
const Sha256 = std.crypto.hash.sha2.Sha256;

const MOD: u64 = 1_000_000_007;
const NH = 32;
const BW = 64;
const NB = 32;
const PASSES = 2;
const Q: u64 = 12289;
const N = 256;
const PAIRS = 6;
const M32: u64 = 0xFFFF_FFFF;

const Digest = [32]u8;
const Block = [BW]u64;

const Lcg = struct {
    s: u64,
    fn next(self: *Lcg) u64 {
        self.s = self.s * 48271 % 2147483647;
        return self.s;
    }
};

fn sha256(msg: []const u8) Digest {
    var out: Digest = undefined;
    Sha256.hash(msg, &out, .{});
    return out;
}

inline fn rot32(x: u64, comptime r: u6) u64 {
    return ((x >> r) | (x << (32 - r))) & M32;
}
inline fn blamka(x: u64, y: u64) u64 {
    return (x + y + 2 * (x & 0xFFFF) * (y & 0xFFFF)) & M32;
}

fn gMix(z: *Block, ia: usize, ib: usize, ic: usize, id: usize) void {
    var a = z[ia];
    var b = z[ib];
    var c = z[ic];
    var d = z[id];
    a = blamka(a, b);
    d = rot32(d ^ a, 16);
    c = blamka(c, d);
    b = rot32(b ^ c, 12);
    a = blamka(a, b);
    d = rot32(d ^ a, 8);
    c = blamka(c, d);
    b = rot32(b ^ c, 7);
    z[ia] = a;
    z[ib] = b;
    z[ic] = c;
    z[id] = d;
}

fn pRound(z: *Block, base: usize, s: usize) void {
    gMix(z, base, base + 2 * s, base + 4 * s, base + 6 * s);
    gMix(z, base + s, base + 3 * s, base + 5 * s, base + 7 * s);
    gMix(z, base, base + 3 * s, base + 4 * s, base + 7 * s);
    gMix(z, base + s, base + 2 * s, base + 5 * s, base + 6 * s);
}

fn compress(x: *const Block, y: *const Block, out: *Block) void {
    var r: Block = undefined;
    for (0..BW) |i| r[i] = x[i] ^ y[i];
    out.* = r;
    for (0..8) |row| pRound(out, row * 8, 1);
    for (0..8) |col| pRound(out, col, 8);
    for (0..BW) |i| out[i] ^= r[i];
}

fn initBlock(h0: *const Digest, j: u8) Block {
    var msg: [33]u8 = undefined;
    @memcpy(msg[0..32], h0);
    msg[32] = j;
    const d = sha256(&msg);
    var w: Block = undefined;
    for (0..8) |k| w[k] = std.mem.readInt(u32, d[4 * k ..][0..4], .big);
    for (8..BW) |k| w[k] = (w[k - 8] + (w[k - 3] & 0xFFFF) * 40503 + (w[k - 1] >> 16) * 69069) & M32;
    return w;
}

fn argon2Like(idx_pw: usize, rng: *Lcg) Digest {
    var msg: [36]u8 = undefined;
    msg[0] = @intCast(idx_pw);
    msg[1] = NB * 256 / 1024;
    msg[2] = PASSES;
    msg[3] = 1;
    for (msg[4..]) |*b| b.* = @intCast(rng.next() & 255);
    const h0 = sha256(&msg);
    var b: [NB]Block = undefined;
    b[0] = initBlock(&h0, 0);
    b[1] = initBlock(&h0, 1);
    var nw: Block = undefined;
    for (0..PASSES) |pass| {
        for (if (pass == 0) 2 else 0..NB) |idx| {
            const prev = b[(idx + NB - 1) % NB];
            const r: u64 = if (pass == 0 and idx < NB / 2)
                (((pass * 131 + idx) * 40503 + 12345) * 69069) % 4294967296
            else
                prev[0];
            const r16 = r >> 16;
            const x = (r16 * r16) >> 16;
            const rf: usize = if (pass == 0) blk: {
                const area: u64 = idx - 1;
                break :blk @intCast(area - 1 - ((area * x) >> 16));
            } else blk: {
                const area: u64 = NB - 2;
                break :blk @intCast((idx + 1 + (area - 1 - ((area * x) >> 16))) % NB);
            };
            const other = b[rf];
            compress(&prev, &other, &nw);
            if (pass == 0) {
                b[idx] = nw;
            } else {
                for (0..BW) |i| b[idx][i] ^= nw[i];
            }
        }
    }
    var bytes: [BW * 4]u8 = undefined;
    for (0..BW) |i| std.mem.writeInt(u32, bytes[4 * i ..][0..4], @intCast(b[NB - 1][i]), .big);
    return sha256(&bytes);
}

fn powMod(b0: u64, e0: u64) u64 {
    var r: u64 = 1;
    var b = b0 % Q;
    var e = e0;
    while (e > 0) {
        if (e & 1 == 1) r = r * b % Q;
        b = b * b % Q;
        e >>= 1;
    }
    return r;
}

fn ntt(a: *[N]u64, w: u64) void {
    var j: usize = 0;
    for (1..N) |i| {
        var bit: usize = N >> 1;
        while (j & bit != 0) {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if (i < j) std.mem.swap(u64, &a[i], &a[j]);
    }
    var len: usize = 2;
    while (len <= N) : (len <<= 1) {
        const wl = powMod(w, N / len);
        const half = len / 2;
        var i: usize = 0;
        while (i < N) : (i += len) {
            var wn: u64 = 1;
            for (0..half) |k| {
                const u = a[i + k];
                const v = a[i + k + half] * wn % Q;
                a[i + k] = (u + v) % Q;
                a[i + k + half] = (u + Q - v) % Q;
                wn = wn * wl % Q;
            }
        }
    }
}

fn polymul(a0: *const [N]u64, b0: *const [N]u64, psi: u64) [N]u64 {
    const psi_inv = powMod(psi, Q - 2);
    const omega = psi * psi % Q;
    const n_inv = powMod(N, Q - 2);
    var a: [N]u64 = undefined;
    var b: [N]u64 = undefined;
    var c: [N]u64 = undefined;
    var p: u64 = 1;
    for (0..N) |i| {
        a[i] = a0[i] * p % Q;
        b[i] = b0[i] * p % Q;
        p = p * psi % Q;
    }
    ntt(&a, omega);
    ntt(&b, omega);
    for (0..N) |i| c[i] = a[i] * b[i] % Q;
    ntt(&c, powMod(omega, Q - 2));
    p = n_inv;
    for (0..N) |i| {
        c[i] = c[i] * p % Q;
        p = p * psi_inv % Q;
    }
    return c;
}

fn schoolbook(a: *const [N]u64, b: *const [N]u64) [N]u64 {
    var c: [N]u64 = @splat(0);
    for (0..N) |i| {
        for (0..N) |j| {
            const pr = a[i] * b[j] % Q;
            const k = i + j;
            if (k >= N) {
                c[k - N] = (c[k - N] + Q - pr) % Q;
            } else {
                c[k] = (c[k] + pr) % Q;
            }
        }
    }
    return c;
}

fn emit(io: std.Io, cs: u64) void {
    var buf: [64]u8 = undefined;
    const s = std.fmt.bufPrint(&buf, "CHECKSUM={d}\n", .{cs}) catch unreachable;
    std.Io.File.stdout().writeStreamingAll(io, s) catch {};
}

pub fn main(init: std.process.Init) !void {
    var rng = Lcg{ .s = 16001 };
    var level: [NH]Digest = undefined;
    for (0..NH) |i| level[i] = argon2Like(i, &rng);
    var n: usize = NH;
    while (n > 1) {
        const m = (n + 1) / 2;
        var next: [NH]Digest = undefined;
        for (0..m) |i| {
            var cat: [64]u8 = undefined;
            @memcpy(cat[0..32], &level[2 * i]);
            @memcpy(cat[32..], &level[if (2 * i + 1 < n) 2 * i + 1 else 2 * i]);
            next[i] = sha256(&cat);
        }
        level = next;
        n = m;
    }
    var cs: u64 = 0;
    for (level[0]) |byte| cs = (cs * 131 + byte) % MOD;

    const psi = powMod(11, (Q - 1) / (2 * N));
    var ok: u64 = if (powMod(psi, N) == Q - 1) 1 else 0;
    for (0..PAIRS) |pr| {
        var a: [N]u64 = undefined;
        var b: [N]u64 = undefined;
        for (&a) |*v| v.* = rng.next() % Q;
        for (&b) |*v| v.* = rng.next() % Q;
        const c = polymul(&a, &b, psi);
        if (pr < 2) {
            const ref = schoolbook(&a, &b);
            if (!std.mem.eql(u64, &ref, &c)) ok = 0;
        }
        for (c) |v| cs = (cs * 131 + v) % MOD;
    }
    cs = (cs * 131 + ok) % MOD;
    emit(init.io, cs);
}
