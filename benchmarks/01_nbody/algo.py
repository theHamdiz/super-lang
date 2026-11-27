#!/usr/bin/env python3
"""01_nbody — 64 bodies × 500 steps, soft gravity, SoA."""

import math

N = 64
STEPS = 500
DT = 0.001
SOFT = 0.01

LCG_MOD = 2**31
rng = 12648430


def next_u():
    global rng
    rng = (rng * 1103515245 + 12345) % LCG_MOD
    return rng


def next_f():
    return next_u() / 2147483648.0


def main():
    x = [0.0] * N
    y = [0.0] * N
    vx = [0.0] * N
    vy = [0.0] * N
    m = [0.0] * N
    for i in range(N):
        x[i] = next_f() * 2.0 - 1.0
        y[i] = next_f() * 2.0 - 1.0
        vx[i] = (next_f() - 0.5) * 0.01
        vy[i] = (next_f() - 0.5) * 0.01
        m[i] = 0.5 + next_f()

    for _ in range(STEPS):
        for i in range(N):
            ax = 0.0
            ay = 0.0
            for j in range(N):
                if i == j:
                    continue
                dx = x[j] - x[i]
                dy = y[j] - y[i]
                d2 = dx * dx + dy * dy + SOFT
                inv = 1.0 / math.sqrt(d2)
                inv3 = inv * inv * inv
                ax += m[j] * dx * inv3
                ay += m[j] * dy * inv3
            vx[i] += ax * DT
            vy[i] += ay * DT
        for i in range(N):
            x[i] += vx[i] * DT
            y[i] += vy[i] * DT

    cs = 0
    for i in range(N):
        cs += math.floor(x[i] * 1000.0)
        cs += math.floor(y[i] * 1000.0)
        cs += math.floor(vx[i] * 1e6)
    print(f"CHECKSUM={cs}")


if __name__ == "__main__":
    main()