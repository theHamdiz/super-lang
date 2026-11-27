# 15_physics2d_broadphase_solver - 6k circles/boxes: uniform spatial-hash broadphase,
# circle/box manifolds, sequential-impulse solver (4 iterations x 10 steps).
# Mirrors std.game.physics2d (grid build, candidate order, narrowphase, resolve).
from math import floor, sqrt

COLS, ROWS = 100, 60
N_DYN = COLS * ROWS
FLOOR_TILES, WALL_TILES = 62, 40
STEPS, ITERS = 10, 4
MOD = 1000000007

rng = 150015


def draw(m):
    global rng
    rng = (rng * 1103515245 + 12345) % 2147483648
    return (rng // 256) % m


# parallel arrays: x y vx vy inv_mass shape r hw hh (e = 0.2, mu = 0.4 for every body)
X, Y, VX, VY, IM, SH, R, HW, HH = [], [], [], [], [], [], [], [], []


def add(x, y, vx, vy, mass, shape, r, hw, hh):
    X.append(x); Y.append(y); VX.append(vx); VY.append(vy)
    IM.append(0.0 if mass <= 0.0 else 1.0 / mass)
    SH.append(shape); R.append(r); HW.append(hw); HH.append(hh)


def sign(v):
    return -1.0 if v < 0.0 else 1.0


def clamp(v, lo, hi):
    return lo if v < lo else hi if v > hi else v


def circle_circle(a, b):
    dx = X[b] - X[a]
    dy = Y[b] - Y[a]
    rr = R[a] + R[b]
    d2 = dx * dx + dy * dy
    if d2 >= rr * rr:
        return None
    if d2 == 0.0:
        return (1.0, 0.0, rr)
    d = sqrt(d2)
    return (dx / d, dy / d, rr - d)


def box_box(a, b):
    dx = X[b] - X[a]
    dy = Y[b] - Y[a]
    ox = HW[a] + HW[b] - abs(dx)
    oy = HH[a] + HH[b] - abs(dy)
    if ox <= 0.0 or oy <= 0.0:
        return None
    if ox < oy:
        return (sign(dx), 0.0, ox)
    return (0.0, sign(dy), oy)


def circle_box(a, b):
    lx = X[a] - X[b]
    ly = Y[a] - Y[b]
    cx = clamp(lx, 0.0 - HW[b], HW[b])
    cy = clamp(ly, 0.0 - HH[b], HH[b])
    fx = lx - cx
    fy = ly - cy
    d2 = fx * fx + fy * fy
    if d2 > 0.0:
        if d2 >= R[a] * R[a]:
            return None
        d = sqrt(d2)
        return ((0.0 - fx) / d, (0.0 - fy) / d, R[a] - d)
    px = HW[b] - abs(lx)
    py = HH[b] - abs(ly)
    if px < py:
        return (0.0 - sign(lx), 0.0, px + R[a])
    return (0.0, 0.0 - sign(ly), py + R[a])


def collide(a, b):
    sa, sb = SH[a], SH[b]
    if sa == 0 and sb == 0:
        return circle_circle(a, b)
    if sa == 1 and sb == 1:
        return box_box(a, b)
    if sa == 0 and sb == 1:
        return circle_box(a, b)
    c = circle_box(b, a)
    if c is None:
        return None
    return (0.0 - c[0], 0.0 - c[1], c[2])


def resolve(a, b, nx, ny, depth):
    ia, ib = IM[a], IM[b]
    isum = ia + ib
    if isum == 0.0:
        return
    rvx = VX[b] - VX[a]
    rvy = VY[b] - VY[a]
    vn = rvx * nx + rvy * ny
    if vn < 0.0:
        e = 0.2
        j = (0.0 - (1.0 + e)) * vn / isum
        VX[a] = VX[a] - j * ia * nx
        VY[a] = VY[a] - j * ia * ny
        VX[b] = VX[b] + j * ib * nx
        VY[b] = VY[b] + j * ib * ny
        tx = rvx - vn * nx
        ty = rvy - vn * ny
        tl = sqrt(tx * tx + ty * ty)
        if tl > 0.000001:
            tx = tx / tl
            ty = ty / tl
            jt = (0.0 - (rvx * tx + rvy * ty)) / isum
            mu = sqrt(0.4 * 0.4)
            lim = mu * j
            jt = clamp(jt, 0.0 - lim, lim)
            VX[a] = VX[a] - jt * ia * tx
            VY[a] = VY[a] - jt * ia * ty
            VX[b] = VX[b] + jt * ib * tx
            VY[b] = VY[b] + jt * ib * ty
    corr = max(depth - 0.01, 0.0) / isum * 0.8
    X[a] = X[a] - corr * ia * nx
    Y[a] = Y[a] - corr * ia * ny
    X[b] = X[b] + corr * ib * nx
    Y[b] = Y[b] + corr * ib * ny


def solve_grid(n):
    # cell edge: twice the mean half-extent, at least 1/8 of the largest body
    s = 0.0
    big = 0.0
    for i in range(n):
        e = max(HW[i], HH[i])
        s += e
        if e > big:
            big = e
    cell = max(2.0 * s / n, big / 8.0)
    if cell < 0.000001:
        cell = 1.0
    inv = 1.0 / cell
    pad = cell * 0.1
    size = 16
    while size < n * 2:
        size *= 2
    mask = size - 1
    heads = [-1] * size
    eb, ex, ey, en = [], [], [], []
    for i in range(n):
        cx0 = floor((X[i] - HW[i] - pad) * inv)
        cx1 = floor((X[i] + HW[i] + pad) * inv)
        cy0 = floor((Y[i] - HH[i] - pad) * inv)
        cy1 = floor((Y[i] + HH[i] + pad) * inv)
        for cy in range(cy0, cy1 + 1):
            for cx in range(cx0, cx1 + 1):
                slot = ((cx * 73856093) ^ (cy * 19349663)) & mask
                en.append(heads[slot])
                heads[slot] = len(eb)
                eb.append(i)
                ex.append(cx)
                ey.append(cy)
    mark = [0] * n
    qpad = 0.1 / inv
    last = 0
    for i in range(n):
        cx0 = floor((X[i] - HW[i] - qpad) * inv)
        cx1 = floor((X[i] + HW[i] + qpad) * inv)
        cy0 = floor((Y[i] - HH[i] - qpad) * inv)
        cy1 = floor((Y[i] + HH[i] + qpad) * inv)
        cands = []
        for cy in range(cy0, cy1 + 1):
            for cx in range(cx0, cx1 + 1):
                e = heads[((cx * 73856093) ^ (cy * 19349663)) & mask]
                while e >= 0:
                    if ex[e] == cx and ey[e] == cy:
                        j = eb[e]
                        if j > i and mark[j] != i + 1:
                            mark[j] = i + 1
                            cands.append(j)
                    e = en[e]
        cands.sort()
        for j in cands:
            if IM[i] + IM[j] > 0.0:
                if abs(X[j] - X[i]) < HW[i] + HW[j] and abs(Y[j] - Y[i]) < HH[i] + HH[j]:
                    c = collide(i, j)
                    if c is not None:
                        resolve(i, j, c[0], c[1], c[2])
                        last += 1
    return last


def posmod(v):
    return ((v % MOD) + MOD) % MOD


def main():
    for k in range(FLOOR_TILES):
        add(1.0 + 2.0 * k, -1.0, 0.0, 0.0, 0.0, 1, 0.0, 1.0, 1.0)
    for k in range(WALL_TILES):
        add(-1.0, 1.0 + 2.0 * k, 0.0, 0.0, 0.0, 1, 0.0, 1.0, 1.0)
    for k in range(WALL_TILES):
        add(122.0, 1.0 + 2.0 * k, 0.0, 0.0, 0.0, 1, 0.0, 1.0, 1.0)
    for i in range(N_DYN):
        col, row = i % COLS, i // COLS
        kind = draw(3)
        sw = 0.35 + draw(400001) * 0.000001
        sh = 0.35 + draw(400001) * 0.000001
        mass = 1.0 + draw(4)
        jx = (draw(600001) - 300000) * 0.000001
        jy = (draw(600001) - 300000) * 0.000001
        vx = (draw(8000001) - 4000000) * 0.000001
        vy = (draw(8000001) - 4000000) * 0.000001
        x = 0.75 + col * 1.2 + jx
        y = 1.75 + row * 1.2 + jy
        if kind == 0:
            add(x, y, vx, vy, mass, 1, 0.0, sw, sh)
        else:
            add(x, y, vx, vy, mass, 0, sw, sw, sw)
    n = len(X)
    cs = 0
    dt = 1.0 / 60.0
    damp = 1.0 / (1.0 + dt * 0.0)
    for s in range(STEPS):
        for i in range(n):
            if IM[i] > 0.0:
                VX[i] = (VX[i] + 0.0 * dt) * damp
                VY[i] = (VY[i] + -9.81 * dt) * damp
                X[i] = X[i] + VX[i] * dt
                Y[i] = Y[i] + VY[i] * dt
        last = 0
        for _ in range(ITERS):
            last = solve_grid(n)
        cs += last * (s + 1)
    h = 0
    ke = 0.0
    for i in range(n):
        h = (h * 131 + posmod(floor(X[i] * 1000.0))) % MOD
        h = (h * 131 + posmod(floor(Y[i] * 1000.0))) % MOD
        h = (h * 131 + posmod(floor(VX[i] * 1000.0))) % MOD
        h = (h * 131 + posmod(floor(VY[i] * 1000.0))) % MOD
        if IM[i] > 0.0:
            ke += 0.5 * (VX[i] * VX[i] + VY[i] * VY[i]) / IM[i]
    cs += h + floor(ke * 10.0)
    print(f"CHECKSUM={cs}")


main()
