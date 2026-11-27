# 11_symbolic_autodiff_ode — 20k random expressions: simplify + forward dual AD,
# then dual-RK4 sensitivities of a stiff-ish ODE (Julia)
# Run: julia -O3 --check-bounds=no algo.jl

const OP_CONST = 0; const OP_VAR = 1; const OP_ADD = 2; const OP_SUB = 3
const OP_MUL = 4; const OP_NEG = 5; const OP_SIN = 6
const EXPRS = 20000; const POINTS = 7; const TRAJ = 1024; const STEPS = 1000; const MAXN = 64
const LEAF = (0.0, 1.0, 2.0, 0.5, -1.0)

mutable struct Tree
    op::Vector{Int}; a::Vector{Int}; b::Vector{Int}; cv::Vector{Float64}; cnt::Int
end
Tree() = Tree(zeros(Int, MAXN), zeros(Int, MAXN), zeros(Int, MAXN), zeros(MAXN), 0)

mutable struct Rng; s::Int; end
@inline function lcg!(r::Rng)
    r.s = (r.s * 1103515245 + 12345) % 2147483648
    return r.s
end

# Node indices are 0-based; storage is index + 1.
function gen!(t::Tree, r::Rng, depth::Int)::Int
    k = lcg!(r) ÷ 256 % 10
    if k < 2 || depth == 0
        m = lcg!(r) ÷ 256 % 8
        n = t.cnt; t.cnt += 1
        if m < 3
            t.op[n+1] = OP_VAR; t.a[n+1] = 0; t.b[n+1] = 0; t.cv[n+1] = 0.0
        else
            t.op[n+1] = OP_CONST; t.a[n+1] = 0; t.b[n+1] = 0; t.cv[n+1] = LEAF[m-3+1]
        end
        return n
    end
    a = gen!(t, r, depth - 1)
    kind = OP_SIN
    k == 8 && (kind = OP_NEG)
    (k >= 5 && k < 8) && (kind = OP_MUL)
    k == 4 && (kind = OP_SUB)
    k < 4 && (kind = OP_ADD)
    b = 0
    if kind == OP_ADD || kind == OP_SUB || kind == OP_MUL
        b = gen!(t, r, depth - 1)
    end
    n = t.cnt; t.cnt += 1
    t.op[n+1] = kind; t.a[n+1] = a; t.b[n+1] = b; t.cv[n+1] = 0.0
    return n
end

@inline is_const(s::Tree, i::Int, c::Float64) = s.op[i+1] == OP_CONST && s.cv[i+1] == c

@inline function push!(s::Tree, kind::Int, a::Int, b::Int, c::Float64)::Int
    n = s.cnt; s.cnt += 1
    s.op[n+1] = kind; s.a[n+1] = a; s.b[n+1] = b; s.cv[n+1] = c
    return n
end

function simp_node!(s::Tree, kind::Int, cv::Float64, l::Int, r::Int)::Int
    if kind == OP_CONST
        return push!(s, OP_CONST, 0, 0, cv)
    elseif kind == OP_VAR
        return push!(s, OP_VAR, 0, 0, 0.0)
    elseif kind == OP_ADD
        is_const(s, l, 0.0) && return r
        is_const(s, r, 0.0) && return l
        (s.op[l+1] == OP_CONST && s.op[r+1] == OP_CONST) && return push!(s, OP_CONST, 0, 0, s.cv[l+1] + s.cv[r+1])
        return push!(s, OP_ADD, l, r, 0.0)
    elseif kind == OP_SUB
        is_const(s, r, 0.0) && return l
        (s.op[l+1] == OP_CONST && s.op[r+1] == OP_CONST) && return push!(s, OP_CONST, 0, 0, s.cv[l+1] - s.cv[r+1])
        l == r && return push!(s, OP_CONST, 0, 0, 0.0)
        return push!(s, OP_SUB, l, r, 0.0)
    elseif kind == OP_MUL
        (is_const(s, l, 0.0) || is_const(s, r, 0.0)) && return push!(s, OP_CONST, 0, 0, 0.0)
        is_const(s, l, 1.0) && return r
        is_const(s, r, 1.0) && return l
        (s.op[l+1] == OP_CONST && s.op[r+1] == OP_CONST) && return push!(s, OP_CONST, 0, 0, s.cv[l+1] * s.cv[r+1])
        return push!(s, OP_MUL, l, r, 0.0)
    elseif kind == OP_NEG
        s.op[l+1] == OP_CONST && return push!(s, OP_CONST, 0, 0, -s.cv[l+1])
        s.op[l+1] == OP_NEG && return s.a[l+1]
        return push!(s, OP_NEG, l, 0, 0.0)
    end
    is_const(s, l, 0.0) && return push!(s, OP_CONST, 0, 0, 0.0)
    return push!(s, OP_SIN, l, 0, 0.0)
end

function simplify!(t::Tree, s::Tree, rep::Vector{Int})
    s.cnt = 0
    for i in 0:t.cnt-1
        rep[i+1] = simp_node!(s, t.op[i+1], t.cv[i+1], rep[t.a[i+1]+1], rep[t.b[i+1]+1])
    end
end

function eval_dual!(t::Tree, x::Float64, ev::Vector{Float64}, ed::Vector{Float64})
    @inbounds for i in 1:t.cnt
        ia = t.a[i] + 1; ib = t.b[i] + 1
        k = t.op[i]
        if k == OP_CONST
            rv = t.cv[i]; rd = 0.0
        elseif k == OP_VAR
            rv = x; rd = 1.0
        elseif k == OP_ADD
            rv = ev[ia] + ev[ib]; rd = ed[ia] + ed[ib]
        elseif k == OP_SUB
            rv = ev[ia] - ev[ib]; rd = ed[ia] - ed[ib]
        elseif k == OP_MUL
            rv = ev[ia] * ev[ib]; rd = ev[ia] * ed[ib] + ed[ia] * ev[ib]
        elseif k == OP_NEG
            rv = -ev[ia]; rd = -ed[ia]
        else
            rv = sin(ev[ia]); rd = cos(ev[ia]) * ed[ia]
        end
        ev[i] = rv; ed[i] = rd
    end
end

@inline function rhs(lam::Float64, t::Float64, v::Float64, d::Float64)
    s = sin(t); c = cos(t)
    dv = v - s
    pv = lam * dv
    pd = lam * d + 1.0 * dv
    return (-pv + 0.5 * c, -pd)
end

function rk4_final(lam::Float64, y0::Float64, h::Float64, steps::Int)
    yv = y0; yd = 0.0
    hh = 0.5 * h; h6 = h / 6.0
    for k in 0:steps-1
        t = Float64(k) * h
        k1v, k1d = rhs(lam, t, yv, yd)
        k2v, k2d = rhs(lam, t + hh, yv + k1v * hh, yd + k1d * hh)
        k3v, k3d = rhs(lam, t + hh, yv + k2v * hh, yd + k2d * hh)
        k4v, k4d = rhs(lam, t + h, yv + k3v * h, yd + k3d * h)
        yv = yv + (((k1v + k2v * 2.0) + k3v * 2.0) + k4v) * h6
        yd = yd + (((k1d + k2d * 2.0) + k3d * 2.0) + k4d) * h6
    end
    return yv, yd
end

function compute()::Int
    r = Rng(20240611)
    t = Tree(); s = Tree()
    rep = zeros(Int, MAXN)
    ev = zeros(MAXN); ed = zeros(MAXN); sev = zeros(MAXN); sed = zeros(MAXN)
    orig_nodes = 0; simp_nodes = 0; mismatch = 0
    acc_v = 0.0; acc_d = 0.0
    for _ in 1:EXPRS
        t.cnt = 0
        gen!(t, r, 4)
        n = t.cnt
        simplify!(t, s, rep)
        m = s.cnt
        orig_nodes += n
        simp_nodes += m
        for p in 0:POINTS-1
            x = (p - 3) / 4.0
            eval_dual!(t, x, ev, ed)
            eval_dual!(s, x, sev, sed)
            vo = ev[n]; dro = ed[n]; vs = sev[rep[n]+1]; ds = sed[rep[n]+1]
            abs(vo - vs) > 0.000000001 * (1.0 + abs(vo)) && (mismatch += 1)
            abs(dro - ds) > 0.000000001 * (1.0 + abs(dro)) && (mismatch += 1)
            acc_v += vs
            acc_d += ds
        end
    end

    sum_y = 0.0; sum_dy = 0.0
    for j in 0:TRAJ-1
        yv, yd = rk4_final(10.0 + j * 0.25, 1.0, 0.002, STEPS)
        sum_y += yv
        sum_dy += yd
    end

    q = floor(Int, acc_v * 10000.0)
    qd = floor(Int, acc_d * 10000.0)
    y1 = floor(Int, sum_y * 1000000.0)
    y2 = floor(Int, sum_dy * 1000000000.0)
    return orig_nodes + simp_nodes * 7 + mismatch * 13 + q * 3 + qd * 5 + y1 * 11 + y2 * 17
end

function main()
    t0 = time_ns()
    cs = compute()
    elapsed = (time_ns() - t0) / 1e9
    println("CHECKSUM=", cs)
    println("ELAPSED_S=", elapsed)
end

if abspath(PROGRAM_FILE) == @__FILE__
    warm = parse(Int, get(ENV, "JULIA_BENCH_WARM", "2"))
    for _ in 1:warm
        compute()
    end
    main()
end
