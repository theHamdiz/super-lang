#!/usr/bin/env python3
"""09_matmul_tiled — 256×256 f64 tiled multiply."""

import math

N = 256
TILE = 32


def main():
    a = [0.0] * (N * N)
    b = [0.0] * (N * N)
    c = [0.0] * (N * N)
    for i in range(N * N):
        a[i] = ((i * 17) % 100) / 100.0
        b[i] = ((i * 31) % 100) / 100.0

    for ii in range(0, N, TILE):
        for jj in range(0, N, TILE):
            for kk in range(0, N, TILE):
                iimax = min(ii + TILE, N)
                jjmax = min(jj + TILE, N)
                kkmax = min(kk + TILE, N)
                for i in range(ii, iimax):
                    for k in range(kk, kkmax):
                        aik = a[i * N + k]
                        for j in range(jj, jjmax):
                            c[i * N + j] += aik * b[k * N + j]

    fro = sum(v * v for v in c)
    cs = math.floor(math.sqrt(fro) * 1000.0)
    print(f"CHECKSUM={cs}")


if __name__ == "__main__":
    main()