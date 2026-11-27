#!/usr/bin/env python3
"""11_symbolic_autodiff_ode — 20k random expressions: simplify + forward dual AD,
then dual-RK4 sensitivities of a stiff-ish ODE."""

import math

OP_CONST, OP_VAR, OP_ADD, OP_SUB, OP_MUL, OP_NEG, OP_SIN = range(7)
EXPRS = 20000
POINTS = 7
TRAJ = 1024
STEPS = 1000
MAXN = 64
LEAF = (0.0, 1.0, 2.0, 0.5, -1.0)

rng = 20240611


def lcg():
    global rng
    rng = (rng * 1103515245 + 12345) % 2147483648
    return rng


def gen(t, depth):
    op, a, b, cv = t
    k = lcg() // 256 % 10
    if k < 2 or depth == 0:
        m = lcg() // 256 % 8
        n = len(op)
        if m < 3:
            op.append(OP_VAR); a.append(0); b.append(0); cv.append(0.0)
        else:
            op.append(OP_CONST); a.append(0); b.append(0); cv.append(LEAF[m - 3])
        return n
    ia = gen(t, depth - 1)
    kind = OP_SIN
    if k == 8:
        kind = OP_NEG
    if 5 <= k < 8:
        kind = OP_MUL
    if k == 4:
        kind = OP_SUB
    if k < 4:
        kind = OP_ADD
    ib = 0
    if kind in (OP_ADD, OP_SUB, OP_MUL):
        ib = gen(t, depth - 1)
    n = len(op)
    op.append(kind); a.append(ia); b.append(ib); cv.append(0.0)
    return n


def push(s, kind, a, b, c):
    n = len(s[0])
    s[0].append(kind); s[1].append(a); s[2].append(b); s[3].append(c)
    return n


def simp_node(s, kind, cv, l, r):
    sop, sa, _sb, scv = s
    if kind == OP_CONST:
        return push(s, OP_CONST, 0, 0, cv)
    if kind == OP_VAR:
        return push(s, OP_VAR, 0, 0, 0.0)
    if kind == OP_ADD:
        if sop[l] == OP_CONST and scv[l] == 0.0:
            return r
        if sop[r] == OP_CONST and scv[r] == 0.0:
            return l
        if sop[l] == OP_CONST and sop[r] == OP_CONST:
            return push(s, OP_CONST, 0, 0, scv[l] + scv[r])
        return push(s, OP_ADD, l, r, 0.0)
    if kind == OP_SUB:
        if sop[r] == OP_CONST and scv[r] == 0.0:
            return l
        if sop[l] == OP_CONST and sop[r] == OP_CONST:
            return push(s, OP_CONST, 0, 0, scv[l] - scv[r])
        if l == r:
            return push(s, OP_CONST, 0, 0, 0.0)
        return push(s, OP_SUB, l, r, 0.0)
    if kind == OP_MUL:
        if (sop[l] == OP_CONST and scv[l] == 0.0) or (sop[r] == OP_CONST and scv[r] == 0.0):
            return push(s, OP_CONST, 0, 0, 0.0)
        if sop[l] == OP_CONST and scv[l] == 1.0:
            return r
        if sop[r] == OP_CONST and scv[r] == 1.0:
            return l
        if sop[l] == OP_CONST and sop[r] == OP_CONST:
            return push(s, OP_CONST, 0, 0, scv[l] * scv[r])
        return push(s, OP_MUL, l, r, 0.0)
    if kind == OP_NEG:
        if sop[l] == OP_CONST:
            return push(s, OP_CONST, 0, 0, -scv[l])
        if sop[l] == OP_NEG:
            return sa[l]
        return push(s, OP_NEG, l, 0, 0.0)
    if sop[l] == OP_CONST and scv[l] == 0.0:
        return push(s, OP_CONST, 0, 0, 0.0)
    return push(s, OP_SIN, l, 0, 0.0)


def simplify(t, rep):
    s = ([], [], [], [])
    op, a, b, cv = t
    for i in range(len(op)):
        rep[i] = simp_node(s, op[i], cv[i], rep[a[i]], rep[b[i]])
    return s


def eval_dual(t, x, ev, ed):
    op, ca, cb, cv = t
    sin, cos = math.sin, math.cos
    for i in range(len(op)):
        k = op[i]
        if k == OP_CONST:
            rv, rd = cv[i], 0.0
        elif k == OP_VAR:
            rv, rd = x, 1.0
        elif k == OP_ADD:
            rv, rd = ev[ca[i]] + ev[cb[i]], ed[ca[i]] + ed[cb[i]]
        elif k == OP_SUB:
            rv, rd = ev[ca[i]] - ev[cb[i]], ed[ca[i]] - ed[cb[i]]
        elif k == OP_MUL:
            av, ad, bv, bd = ev[ca[i]], ed[ca[i]], ev[cb[i]], ed[cb[i]]
            rv, rd = av * bv, av * bd + ad * bv
        elif k == OP_NEG:
            rv, rd = -ev[ca[i]], -ed[ca[i]]
        else:
            av = ev[ca[i]]
            rv, rd = sin(av), cos(av) * ed[ca[i]]
        ev[i] = rv
        ed[i] = rd


def rk4_final(lam, y0, h, steps):
    sin, cos = math.sin, math.cos
    yv, yd = y0, 0.0
    hh, h6 = 0.5 * h, h / 6.0

    def rhs(t, v, d):
        s, c = sin(t), cos(t)
        dv = v - s
        pv = lam * dv
        pd = lam * d + 1.0 * dv
        return -pv + 0.5 * c, -pd

    for k in range(steps):
        t = float(k) * h
        k1v, k1d = rhs(t, yv, yd)
        k2v, k2d = rhs(t + hh, yv + k1v * hh, yd + k1d * hh)
        k3v, k3d = rhs(t + hh, yv + k2v * hh, yd + k2d * hh)
        k4v, k4d = rhs(t + h, yv + k3v * h, yd + k3d * h)
        yv = yv + (((k1v + k2v * 2.0) + k3v * 2.0) + k4v) * h6
        yd = yd + (((k1d + k2d * 2.0) + k3d * 2.0) + k4d) * h6
    return yv, yd


def main():
    rep = [0] * MAXN
    ev = [0.0] * MAXN
    ed = [0.0] * MAXN
    sev = [0.0] * MAXN
    sed = [0.0] * MAXN
    orig_nodes = simp_nodes = mismatch = 0
    acc_v = acc_d = 0.0
    for _ in range(EXPRS):
        t = ([], [], [], [])
        gen(t, 4)
        n = len(t[0])
        s = simplify(t, rep)
        m = len(s[0])
        orig_nodes += n
        simp_nodes += m
        for p in range(POINTS):
            x = (p - 3) / 4.0
            eval_dual(t, x, ev, ed)
            eval_dual(s, x, sev, sed)
            vo, dro, vs, ds = ev[n - 1], ed[n - 1], sev[rep[n - 1]], sed[rep[n - 1]]
            if abs(vo - vs) > 0.000000001 * (1.0 + abs(vo)):
                mismatch += 1
            if abs(dro - ds) > 0.000000001 * (1.0 + abs(dro)):
                mismatch += 1
            acc_v += vs
            acc_d += ds

    sum_y = sum_dy = 0.0
    for j in range(TRAJ):
        yv, yd = rk4_final(10.0 + j * 0.25, 1.0, 0.002, STEPS)
        sum_y += yv
        sum_dy += yd

    q = math.floor(acc_v * 10000.0)
    qd = math.floor(acc_d * 10000.0)
    y1 = math.floor(sum_y * 1000000.0)
    y2 = math.floor(sum_dy * 1000000000.0)
    cs = orig_nodes + simp_nodes * 7 + mismatch * 13 + q * 3 + qd * 5 + y1 * 11 + y2 * 17
    print(f"CHECKSUM={cs}")


if __name__ == "__main__":
    main()
