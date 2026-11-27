#!/usr/bin/env python3
"""16_crypto_pq_argon2_suite — Argon2id-like KDF (m=8KiB t=2 p=1) x32, SHA-256 Merkle of tags,
toy lattice negacyclic NTT poly-mul (q=12289, n=256)."""
from hashlib import sha256

MOD = 1_000_000_007
NH, BW, NB, PASSES = 32, 64, 32, 2
Q, N, PAIRS = 12289, 256, 6
M32 = 0xFFFFFFFF

rng = 16001


def next_rand() -> int:
    global rng
    rng = rng * 48271 % 2147483647
    return rng


def rot32(x: int, r: int) -> int:
    return ((x >> r) | (x << (32 - r))) & M32


def blamka(x: int, y: int) -> int:
    return (x + y + 2 * (x & 0xFFFF) * (y & 0xFFFF)) & M32


def g_mix(z: list[int], ia: int, ib: int, ic: int, id_: int) -> None:
    a, b, c, d = z[ia], z[ib], z[ic], z[id_]
    a = blamka(a, b); d = rot32(d ^ a, 16)
    c = blamka(c, d); b = rot32(b ^ c, 12)
    a = blamka(a, b); d = rot32(d ^ a, 8)
    c = blamka(c, d); b = rot32(b ^ c, 7)
    z[ia], z[ib], z[ic], z[id_] = a, b, c, d


def p_round(z: list[int], base: int, stride: int) -> None:
    v = [base + k * stride for k in range(8)]
    g_mix(z, v[0], v[2], v[4], v[6])
    g_mix(z, v[1], v[3], v[5], v[7])
    g_mix(z, v[0], v[3], v[4], v[7])
    g_mix(z, v[1], v[2], v[5], v[6])


def compress(x: list[int], y: list[int]) -> list[int]:
    r = [a ^ b for a, b in zip(x, y)]
    out = r[:]
    for row in range(8):
        p_round(out, row * 8, 1)
    for col in range(8):
        p_round(out, col, 8)
    return [a ^ b for a, b in zip(out, r)]


def init_block(h0: bytes, j: int) -> list[int]:
    d = sha256(h0 + bytes([j])).digest()
    w = [int.from_bytes(d[4 * k:4 * k + 4], "big") for k in range(8)]
    for k in range(8, BW):
        w.append((w[k - 8] + (w[k - 3] & 0xFFFF) * 40503 + (w[k - 1] >> 16) * 69069) & M32)
    return w


def argon2_like(idx_pw: int) -> bytes:
    msg = bytes([idx_pw, NB * 256 // 1024, PASSES, 1] + [next_rand() & 255 for _ in range(32)])
    h0 = sha256(msg).digest()
    blocks = [[0] * BW for _ in range(NB)]
    blocks[0], blocks[1] = init_block(h0, 0), init_block(h0, 1)
    for p in range(PASSES):
        for idx in range(2 if p == 0 else 0, NB):
            prev = blocks[(idx + NB - 1) % NB]
            if p == 0 and idx < NB // 2:
                r = ((p * 131 + idx) * 40503 + 12345) * 69069 % 4294967296
            else:
                r = prev[0]
            r16 = r >> 16
            x = (r16 * r16) >> 16
            if p == 0:
                area = idx - 1
                ref = area - 1 - ((area * x) >> 16)
            else:
                area = NB - 2
                rel = area - 1 - ((area * x) >> 16)
                ref = (idx + 1 + rel) % NB
            nw = compress(prev, blocks[ref])
            if p == 0:
                blocks[idx] = nw
            else:
                blocks[idx] = [a ^ b for a, b in zip(blocks[idx], nw)]
    return sha256(b"".join(w.to_bytes(4, "big") for w in blocks[NB - 1])).digest()


def ntt(a: list[int], w: int) -> None:
    j = 0
    for i in range(1, N):
        bit = N >> 1
        while j & bit:
            j ^= bit
            bit >>= 1
        j ^= bit
        if i < j:
            a[i], a[j] = a[j], a[i]
    ln = 2
    while ln <= N:
        wl = pow(w, N // ln, Q)
        half = ln // 2
        for i in range(0, N, ln):
            wn = 1
            for j in range(half):
                u = a[i + j]
                v = a[i + j + half] * wn % Q
                a[i + j] = (u + v) % Q
                a[i + j + half] = (u + Q - v) % Q
                wn = wn * wl % Q
        ln <<= 1


def polymul(a0: list[int], b0: list[int], psi: int) -> list[int]:
    psi_inv, omega, n_inv = pow(psi, Q - 2, Q), psi * psi % Q, pow(N, Q - 2, Q)
    a, b = [0] * N, [0] * N
    p = 1
    for i in range(N):
        a[i], b[i] = a0[i] * p % Q, b0[i] * p % Q
        p = p * psi % Q
    ntt(a, omega)
    ntt(b, omega)
    c = [a[i] * b[i] % Q for i in range(N)]
    ntt(c, pow(omega, Q - 2, Q))
    p = n_inv
    for i in range(N):
        c[i] = c[i] * p % Q
        p = p * psi_inv % Q
    return c


def schoolbook(a: list[int], b: list[int]) -> list[int]:
    c = [0] * N
    for i in range(N):
        for j in range(N):
            pr = a[i] * b[j] % Q
            k = i + j
            if k >= N:
                c[k - N] = (c[k - N] + Q - pr) % Q
            else:
                c[k] = (c[k] + pr) % Q
    return c


def main() -> None:
    level = [argon2_like(i) for i in range(NH)]
    while len(level) > 1:
        level = [sha256(level[i] + level[i + 1 if i + 1 < len(level) else i]).digest()
                 for i in range(0, len(level), 2)]
    cs = 0
    for byte in level[0]:
        cs = (cs * 131 + byte) % MOD
    psi = pow(11, (Q - 1) // (2 * N), Q)
    ok = 1 if pow(psi, N, Q) == Q - 1 else 0
    for pr in range(PAIRS):
        a = [next_rand() % Q for _ in range(N)]
        b = [next_rand() % Q for _ in range(N)]
        c = polymul(a, b, psi)
        if pr < 2 and schoolbook(a, b) != c:
            ok = 0
        for v in c:
            cs = (cs * 131 + v) % MOD
    cs = (cs * 131 + ok) % MOD
    print(f"CHECKSUM={cs}")


if __name__ == "__main__":
    main()
