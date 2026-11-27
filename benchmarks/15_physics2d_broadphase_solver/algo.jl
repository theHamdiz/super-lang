# 15_physics2d_broadphase_solver - 6k circles/boxes: uniform spatial-hash broadphase,
# circle/box manifolds, sequential-impulse solver (4 iterations x 10 steps) (Julia)
# Mirrors std.game.physics2d (grid build, candidate order, narrowphase, resolve).
# Run: julia -O3 --check-bounds=no algo.jl

const COLS = 100
const ROWS = 60
const N_DYN = COLS * ROWS
const FLOOR_TILES = 62
const WALL_TILES = 40
const N = FLOOR_TILES + 2 * WALL_TILES + N_DYN
const STEPS = 10
const ITERS = 4
const MOD = 1000000007

mutable struct Rng
    s::Int
end

@inline function draw!(r::Rng, m::Int)::Int
    r.s = (r.s * 1103515245 + 12345) % 2147483648
    return (r.s ÷ 256) % m
end

const CIRCLE = 0
const BOX = 1

mutable struct Body
    x::Float64
    y::Float64
    vx::Float64
    vy::Float64
    inv_mass::Float64
    shape::Int
    r::Float64
    hw::Float64
    hh::Float64
    e::Float64
    mu::Float64
end

inv_of(m::Float64) = m <= 0.0 ? 0.0 : 1.0 / m
circle(x, y, r, mass, vx, vy) = Body(x, y, vx, vy, inv_of(mass), CIRCLE, r, r, r, 0.2, 0.4)
aabb(x, y, hw, hh, mass, vx, vy) = Body(x, y, vx, vy, inv_of(mass), BOX, 0.0, hw, hh, 0.2, 0.4)

@inline sgn(v::Float64) = v < 0.0 ? -1.0 : 1.0
@inline clampf(v::Float64, lo::Float64, hi::Float64) = v < lo ? lo : (v > hi ? hi : v)
@inline pmax(a::Float64, b::Float64) = a > b ? a : b
@inline pmin(a::Float64, b::Float64) = a < b ? a : b

# narrowphase: returns (hit, nx, ny, depth)
const NOHIT = (false, 0.0, 0.0, 0.0)

@inline function circle_circle(a::Body, b::Body)
    dx = b.x - a.x; dy = b.y - a.y
    rr = a.r + b.r
    d2 = dx * dx + dy * dy
    d2 >= rr * rr && return NOHIT
    d2 == 0.0 && return (true, 1.0, 0.0, rr)
    d = sqrt(d2)
    return (true, dx / d, dy / d, rr - d)
end

@inline function box_box(a::Body, b::Body)
    dx = b.x - a.x; dy = b.y - a.y
    ox = a.hw + b.hw - abs(dx)
    oy = a.hh + b.hh - abs(dy)
    ox <= 0.0 && return NOHIT
    oy <= 0.0 && return NOHIT
    ox < oy && return (true, sgn(dx), 0.0, ox)
    return (true, 0.0, sgn(dy), oy)
end

@inline function circle_box(a::Body, b::Body)
    lx = a.x - b.x; ly = a.y - b.y
    cx = clampf(lx, 0.0 - b.hw, b.hw)
    cy = clampf(ly, 0.0 - b.hh, b.hh)
    fx = lx - cx; fy = ly - cy
    d2 = fx * fx + fy * fy
    if d2 > 0.0
        d2 >= a.r * a.r && return NOHIT
        d = sqrt(d2)
        return (true, (0.0 - fx) / d, (0.0 - fy) / d, a.r - d)
    end
    px = b.hw - abs(lx); py = b.hh - abs(ly)
    px < py && return (true, 0.0 - sgn(lx), 0.0, px + a.r)
    return (true, 0.0, 0.0 - sgn(ly), py + a.r)
end

@inline function collide(a::Body, b::Body)
    if a.shape == CIRCLE && b.shape == CIRCLE
        return circle_circle(a, b)
    elseif a.shape == BOX && b.shape == BOX
        return box_box(a, b)
    elseif a.shape == CIRCLE && b.shape == BOX
        return circle_box(a, b)
    end
    c = circle_box(b, a)
    c[1] || return NOHIT
    return (true, 0.0 - c[2], 0.0 - c[3], c[4])
end

function resolve!(a::Body, b::Body, nx::Float64, ny::Float64, depth::Float64)
    ia = a.inv_mass; ib = b.inv_mass
    isum = ia + ib
    isum == 0.0 && return
    rvx = b.vx - a.vx; rvy = b.vy - a.vy
    vn = rvx * nx + rvy * ny
    if vn < 0.0
        e = pmin(a.e, b.e)
        j = (0.0 - (1.0 + e)) * vn / isum
        a.vx = a.vx - j * ia * nx
        a.vy = a.vy - j * ia * ny
        b.vx = b.vx + j * ib * nx
        b.vy = b.vy + j * ib * ny
        tx = rvx - vn * nx; ty = rvy - vn * ny
        tl = sqrt(tx * tx + ty * ty)
        if tl > 0.000001
            tx = tx / tl
            ty = ty / tl
            jt = (0.0 - (rvx * tx + rvy * ty)) / isum
            mu = sqrt(a.mu * b.mu)
            lim = mu * j
            jt = clampf(jt, 0.0 - lim, lim)
            a.vx = a.vx - jt * ia * tx
            a.vy = a.vy - jt * ia * ty
            b.vx = b.vx + jt * ib * tx
            b.vy = b.vy + jt * ib * ty
        end
    end
    corr = pmax(depth - 0.01, 0.0) / isum * 0.8
    a.x = a.x - corr * ia * nx
    a.y = a.y - corr * ia * ny
    b.x = b.x + corr * ib * nx
    b.y = b.y + corr * ib * ny
    return
end

@inline bucket(cx::Int, cy::Int, mask::Int) = ((cx * 73856093) ⊻ (cy * 19349663)) & mask

mutable struct Grid
    heads::Vector{Int}
    eb::Vector{Int}
    ex::Vector{Int}
    ey::Vector{Int}
    en::Vector{Int}
    inv::Float64
    mask::Int
end

function grid_build!(g::Grid, bodies::Vector{Body})
    n = length(bodies)
    s = 0.0; big = 0.0
    for b in bodies
        e = pmax(b.hw, b.hh)
        s += e
        e > big && (big = e)
    end
    cell = pmax(2.0 * s / n, big / 8.0)
    cell < 0.000001 && (cell = 1.0)
    g.inv = 1.0 / cell
    pad = cell * 0.1
    size = 16
    while size < n * 2
        size *= 2
    end
    g.mask = size - 1
    resize!(g.heads, size)
    fill!(g.heads, -1)
    empty!(g.eb); empty!(g.ex); empty!(g.ey); empty!(g.en)
    inv = g.inv
    for i in 1:n
        b = bodies[i]
        cx0 = floor(Int, (b.x - b.hw - pad) * inv); cx1 = floor(Int, (b.x + b.hw + pad) * inv)
        cy0 = floor(Int, (b.y - b.hh - pad) * inv); cy1 = floor(Int, (b.y + b.hh + pad) * inv)
        for cy in cy0:cy1, cx in cx0:cx1
            slot = bucket(cx, cy, g.mask) + 1
            push!(g.en, g.heads[slot])
            g.heads[slot] = length(g.eb)
            push!(g.eb, i - 1); push!(g.ex, cx); push!(g.ey, cy)
        end
    end
    return
end

function solve_grid!(bodies::Vector{Body}, g::Grid, mark::Vector{Int}, cands::Vector{Int})::Int
    n = length(bodies)
    grid_build!(g, bodies)
    fill!(mark, 0)
    inv = g.inv
    qpad = 0.1 / inv
    last = 0
    for i in 0:(n - 1)
        b = bodies[i + 1]
        cx0 = floor(Int, (b.x - b.hw - qpad) * inv); cx1 = floor(Int, (b.x + b.hw + qpad) * inv)
        cy0 = floor(Int, (b.y - b.hh - qpad) * inv); cy1 = floor(Int, (b.y + b.hh + qpad) * inv)
        empty!(cands)
        for cy in cy0:cy1, cx in cx0:cx1
            e = g.heads[bucket(cx, cy, g.mask) + 1]
            while e >= 0
                if g.ex[e + 1] == cx && g.ey[e + 1] == cy
                    j = g.eb[e + 1]
                    if j > i && mark[j + 1] != i + 1
                        mark[j + 1] = i + 1
                        push!(cands, j)
                    end
                end
                e = g.en[e + 1]
            end
        end
        sort!(cands)
        for j in cands
            a = bodies[i + 1]; c = bodies[j + 1]
            if a.inv_mass + c.inv_mass > 0.0 && !(abs(c.x - a.x) >= a.hw + c.hw) && !(abs(c.y - a.y) >= a.hh + c.hh)
                hit, nx, ny, depth = collide(a, c)
                if hit
                    resolve!(a, c, nx, ny, depth)
                    last += 1
                end
            end
        end
    end
    return last
end

posmod(v::Int) = ((v % MOD) + MOD) % MOD

function compute()::Int
    bodies = Body[]
    sizehint!(bodies, N)
    for k in 0:(FLOOR_TILES - 1)
        push!(bodies, aabb(1.0 + 2.0 * k, -1.0, 1.0, 1.0, 0.0, 0.0, 0.0))
    end
    for k in 0:(WALL_TILES - 1)
        push!(bodies, aabb(-1.0, 1.0 + 2.0 * k, 1.0, 1.0, 0.0, 0.0, 0.0))
    end
    for k in 0:(WALL_TILES - 1)
        push!(bodies, aabb(122.0, 1.0 + 2.0 * k, 1.0, 1.0, 0.0, 0.0, 0.0))
    end
    r = Rng(150015)
    for i in 0:(N_DYN - 1)
        col = i % COLS; row = i ÷ COLS
        kind = draw!(r, 3)
        sw = 0.35 + draw!(r, 400001) * 0.000001
        sh = 0.35 + draw!(r, 400001) * 0.000001
        mass = 1.0 + draw!(r, 4)
        jx = (draw!(r, 600001) - 300000) * 0.000001
        jy = (draw!(r, 600001) - 300000) * 0.000001
        vx = (draw!(r, 8000001) - 4000000) * 0.000001
        vy = (draw!(r, 8000001) - 4000000) * 0.000001
        x = 0.75 + col * 1.2 + jx
        y = 1.75 + row * 1.2 + jy
        push!(bodies, kind == 0 ? aabb(x, y, sw, sh, mass, vx, vy) : circle(x, y, sw, mass, vx, vy))
    end

    n = length(bodies)
    g = Grid(Int[], Int[], Int[], Int[], Int[], 1.0, 15)
    mark = zeros(Int, n)
    cands = Int[]
    cs = 0
    dt = 1.0 / 60.0
    damp = 1.0 / (1.0 + dt * 0.0)
    for s in 0:(STEPS - 1)
        for b in bodies
            if b.inv_mass > 0.0
                b.vx = (b.vx + 0.0 * dt) * damp
                b.vy = (b.vy + -9.81 * dt) * damp
                b.x = b.x + b.vx * dt
                b.y = b.y + b.vy * dt
            end
        end
        last = 0
        for _ in 1:ITERS
            last = solve_grid!(bodies, g, mark, cands)
        end
        cs += last * (s + 1)
    end
    h = 0; ke = 0.0
    for b in bodies
        h = (h * 131 + posmod(floor(Int, b.x * 1000.0))) % MOD
        h = (h * 131 + posmod(floor(Int, b.y * 1000.0))) % MOD
        h = (h * 131 + posmod(floor(Int, b.vx * 1000.0))) % MOD
        h = (h * 131 + posmod(floor(Int, b.vy * 1000.0))) % MOD
        b.inv_mass > 0.0 && (ke += 0.5 * (b.vx * b.vx + b.vy * b.vy) / b.inv_mass)
    end
    return cs + h + floor(Int, ke * 10.0)
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
