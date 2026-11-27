#!/usr/bin/env python3
"""17_ai_tensor_hnsw_search — fixed-point (Q10) transformer blocks (causal attention, softmax,
ReLU FFN, RMS norm) on 12 sequences, then brute-force top-10 cosine search over 4096 unit
vectors of dim 32 using the pooled embeddings as queries. Integer-only => bit-exact everywhere."""

MOD = 1_000_000_007
BIAS = 1 << 40
D, T, FF, LAYERS, SEQS, NV, K = 32, 16, 64, 2, 12, 4096, 10

rng = 17017


def next_rand() -> int:
    global rng
    rng = rng * 48271 % 2147483647
    return rng


def isqrt(n: int) -> int:
    if n == 0:
        return 0
    x, y = n, (n + 1) >> 1
    while y < x:
        x = y
        y = (x + n // x) >> 1
    return x


def exp_frac(f: int) -> int:
    term = total = 1024
    for k in range(1, 7):
        term = ((term * f) >> 10) // k
        total = total - term if k & 1 else total + term
    return max(total, 0)


def exp_q(x: int, e1: int) -> int:
    a = -x
    n, f = a >> 10, a & 1023
    if n >= 16:
        return 0
    r = exp_frac(f)
    for _ in range(n):
        r = (r * e1) >> 10
    return r


def matmul(x: list[int], w: list[int], din: int, dout: int) -> list[int]:
    y = [0] * (T * dout)
    for t in range(T):
        xr = x[t * din:(t + 1) * din]
        for o in range(dout):
            s = 0
            for i in range(din):
                s += xr[i] * w[i * dout + o]
            y[t * dout + o] = s >> 10
    return y


def rms_norm(h: list[int]) -> None:
    for t in range(T):
        row = h[t * D:(t + 1) * D]
        rms = isqrt(sum(v * v for v in row) // D)
        inv = (1 << 20) // (rms + 1)
        for d in range(D):
            h[t * D + d] = (row[d] * inv) >> 10


def fill_w(n: int) -> list[int]:
    return [next_rand() % 257 - 128 for _ in range(n)]


def block(ly: tuple, h: list[int], e1: int) -> None:
    wq, wk, wv, wo, w1, w2 = ly
    q, k, v = matmul(h, wq, D, D), matmul(h, wk, D, D), matmul(h, wv, D, D)
    at = [0] * (T * D)
    for t in range(T):
        sc = []
        for j in range(t + 1):
            s = 0
            for d in range(D):
                s += q[t * D + d] * k[j * D + d]
            sc.append(s >> 13)
        mx = max(sc)
        pr = [exp_q(s - mx, e1) for s in sc]
        total = sum(pr)
        pr = [p * 1024 // total for p in pr]
        for d in range(D):
            s = 0
            for j in range(t + 1):
                s += pr[j] * v[j * D + d]
            at[t * D + d] = s >> 10
    o = matmul(at, wo, D, D)
    for i in range(T * D):
        h[i] += o[i]
    rms_norm(h)
    f = [max(x, 0) for x in matmul(h, w1, D, FF)]
    g = matmul(f, w2, FF, D)
    for i in range(T * D):
        h[i] += g[i]
    rms_norm(h)


def normalize(raw: list[int]) -> list[int]:
    nrm = isqrt(sum(v * v for v in raw))
    inv = (1 << 20) // (nrm + 1)
    return [(v * inv) >> 10 for v in raw]


def main() -> None:
    e1 = exp_frac(1024)
    layers = []
    for _ in range(LAYERS):
        layers.append((fill_w(D * D), fill_w(D * D), fill_w(D * D), fill_w(D * D), fill_w(D * FF), fill_w(FF * D)))
    db = []
    for _ in range(NV):
        db.append(normalize([next_rand() % 2049 - 1024 for _ in range(D)]))
    cs = 0
    for _ in range(SEQS):
        h = [next_rand() % 2049 - 1024 for _ in range(T * D)]
        for ly in layers:
            block(ly, h, e1)
        pooled = [sum(h[t * D + d] for t in range(T)) >> 4 for d in range(D)]
        for v in pooled:
            cs = (cs * 131 + v + BIAS) % MOD
        query = normalize(pooled)
        top_s: list[int] = []
        top_i: list[int] = []
        for i in range(NV):
            row = db[i]
            sc = 0
            for d in range(D):
                sc += query[d] * row[d]
            if len(top_s) < K or sc > top_s[K - 1]:
                p = len(top_s) if len(top_s) < K else K - 1
                if len(top_s) < K:
                    top_s.append(0)
                    top_i.append(0)
                while p > 0 and top_s[p - 1] < sc:
                    top_s[p], top_i[p] = top_s[p - 1], top_i[p - 1]
                    p -= 1
                top_s[p], top_i[p] = sc, i
        for j in range(K):
            cs = (cs * 131 + top_i[j] + BIAS) % MOD
            cs = (cs * 131 + top_s[j] + BIAS) % MOD
    print(f"CHECKSUM={cs}")


if __name__ == "__main__":
    main()
