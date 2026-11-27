#!/usr/bin/env python3
"""08_ecs — 2000 entities × 20 ticks, spatial grid neighbor queries."""

import math

N = 2000
TICKS = 20
CELL = 10
GRID = 64

LCG_MOD = 2**31
rng = 15482888


def next_u():
    global rng
    rng = (rng * 1103515245 + 12345) % LCG_MOD
    return rng


def next_f():
    return next_u() / 2147483648.0


def main():
    px = [0.0] * N
    py = [0.0] * N
    vx = [0.0] * N
    vy = [0.0] * N
    hp = [0] * N
    for i in range(N):
        px[i] = (next_f() * 600.0)
        py[i] = (next_f() * 600.0)
        vx[i] = ((next_f() - 0.5) * 2.0)
        vy[i] = ((next_f() - 0.5) * 2.0)
        hp[i] = 10 + (next_u() % 20)

    heads = [-1] * (GRID * GRID)
    nxt = [0] * N

    for _ in range(TICKS):
        for i in range(N):
            px[i] = (px[i] + vx[i])
            py[i] = (py[i] + vy[i])
            if px[i] < 0 or px[i] > 600:
                vx[i] = (-vx[i])
            if py[i] < 0 or py[i] > 600:
                vy[i] = (-vy[i])

        heads = [-1] * (GRID * GRID)
        for i in range(N):
            cx = int(px[i] / CELL)
            cy = int(py[i] / CELL)
            if cx < 0:
                cx = 0
            if cy < 0:
                cy = 0
            if cx >= GRID:
                cx = GRID - 1
            if cy >= GRID:
                cy = GRID - 1
            cell = cy * GRID + cx
            nxt[i] = heads[cell]
            heads[cell] = i

        for i in range(N):
            cx = int(px[i] / CELL)
            cy = int(py[i] / CELL)
            if cx < 0:
                cx = 0
            if cy < 0:
                cy = 0
            if cx >= GRID:
                cx = GRID - 1
            if cy >= GRID:
                cy = GRID - 1
            hits = 0
            for oy in range(-1, 2):
                for ox in range(-1, 2):
                    nx_c = cx + ox
                    ny_c = cy + oy
                    if nx_c < 0 or ny_c < 0 or nx_c >= GRID or ny_c >= GRID:
                        continue
                    eid = heads[ny_c * GRID + nx_c]
                    while eid >= 0:
                        if eid != i:
                            dx = (px[i] - px[eid])
                            dy = (py[i] - py[eid])
                            if (dx * dx + dy * dy) < 4.0:
                                hits += 1
                        eid = nxt[eid]
            if hits > 3:
                hp[i] -= 1

    cs = 0
    for i in range(N):
        cs += hp[i]
        cs += math.floor(px[i] * 10.0)
        cs += math.floor(py[i] * 10.0)
    print(f"CHECKSUM={cs}")


if __name__ == "__main__":
    main()