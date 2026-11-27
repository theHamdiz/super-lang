#!/usr/bin/env python3
"""12_sparse_fem_fft — 128x128 reaction-Poisson CSR, Jacobi-PCG x20, 2D FFT low-pass bank."""

import math

G = 128
N = 16384
ITERS = 20


def build_system():
    indptr, indices, values, diag = [0], [], [], [0.0] * N
    for i in range(N):
        row, col = i // G, i % G
        d = 4.0 + (i * 37 % 11) * 0.25
        diag[i] = d
        if row > 0:
            indices.append(i - G); values.append(-1.0)
        if col > 0:
            indices.append(i - 1); values.append(-1.0)
        indices.append(i); values.append(d)
        if col < G - 1:
            indices.append(i + 1); values.append(-1.0)
        if row < G - 1:
            indices.append(i + G); values.append(-1.0)
        indptr.append(len(indices))
    return indptr, indices, values, diag


def matvec(indptr, indices, values, x):
    out = [0.0] * N
    for r in range(N):
        s = 0.0
        for k in range(indptr[r], indptr[r + 1]):
            s += values[k] * x[indices[k]]
        out[r] = s
    return out


def dot(a, b):
    s = 0.0
    for i in range(N):
        s += a[i] * b[i]
    return s


def pcg(indptr, indices, values, diag, b):
    x = [0.0] * N
    r = list(b)
    z = [r[i] / diag[i] for i in range(N)]
    p = list(z)
    rz = dot(r, z)
    for _ in range(ITERS):
        ap = matvec(indptr, indices, values, p)
        alpha = rz / dot(p, ap)
        for i in range(N):
            x[i] += alpha * p[i]
            r[i] -= alpha * ap[i]
            z[i] = r[i] / diag[i]
        rz_new = dot(r, z)
        beta = rz_new / rz
        for i in range(N):
            p[i] = z[i] + beta * p[i]
        rz = rz_new
    return x, dot(r, r)


BITREV = []
for _i in range(G):
    _r, _v = 0, _i
    for _ in range(7):
        _r = (_r << 1) | (_v & 1)
        _v >>= 1
    BITREV.append(_r)


def build_twiddles():
    tw = []
    span = 2
    while span <= G:
        ang = 0.0 - 2.0 * math.pi / float(span)
        tw.append([(math.cos(ang * k), math.sin(ang * k)) for k in range(span // 2)])
        span *= 2
    return tw


TW = build_twiddles()


def fft1(re, im, inv):
    if inv:
        im = [0.0 - v for v in im]
    tre = [re[BITREV[i]] for i in range(G)]
    tim = [im[BITREV[i]] for i in range(G)]
    span, lvl = 2, 0
    while span <= G:
        half = span // 2
        tw = TW[lvl]
        for start in range(0, G, span):
            for k in range(half):
                wr, wi = tw[k]
                a = start + k
                b = a + half
                tr = tre[b] * wr - tim[b] * wi
                ti = tre[b] * wi + tim[b] * wr
                tre[b] = tre[a] - tr
                tim[b] = tim[a] - ti
                tre[a] = tre[a] + tr
                tim[a] = tim[a] + ti
        span *= 2
        lvl += 1
    if inv:
        s = 1.0 / float(G)
        return [v * s for v in tre], [(0.0 - v) * s for v in tim]
    return tre, tim


def fft2(re, im, inv):
    for r in range(G):
        o_re, o_im = fft1(re[r * G:(r + 1) * G], im[r * G:(r + 1) * G], inv)
        re[r * G:(r + 1) * G] = o_re
        im[r * G:(r + 1) * G] = o_im
    for c in range(G):
        o_re, o_im = fft1(re[c::G], im[c::G], inv)
        re[c::G] = o_re
        im[c::G] = o_im


def main():
    b = [((i * 7 + (i // G) * 13) % 17 - 8) / 8.0 for i in range(N)]
    indptr, indices, values, diag = build_system()
    x, rr = pcg(indptr, indices, values, diag, b)
    res = math.sqrt(rr)

    sre, sim = list(x), [0.0] * N
    fft2(sre, sim, False)

    cuts = [64, 256, 1024]
    energy, corner = 0.0, 0.0
    for q in range(3):
        fre, fim = list(sre), list(sim)
        for k in range(N):
            ky, kx = k // G, k % G
            fy = ky if ky <= G // 2 else G - ky
            fx = kx if kx <= G // 2 else G - kx
            if fx * fx + fy * fy > cuts[q]:
                fre[k] = 0.0
                fim[k] = 0.0
        fft2(fre, fim, True)
        for k in range(N):
            energy += fre[k] * fre[k]
        corner += fre[0] + fre[N // 2 + G // 2] + fre[N - 1]

    xs = dot(x, x)
    cs = (math.floor(math.sqrt(xs) * 1000000.0) + math.floor(energy * 1000.0)
          + math.floor(corner * 1000000.0) + math.floor(res * 1000000000.0))
    print(f"CHECKSUM={cs}")


if __name__ == "__main__":
    main()
