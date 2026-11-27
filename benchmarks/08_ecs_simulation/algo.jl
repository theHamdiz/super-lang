# 08_ecs_simulation — 2000 entities × 20 ticks, spatial-grid neighbour queries (Julia 1.13)
# Distance test uses Float32 deltas exactly like algo.c.
# Run: julia -O3 --check-bounds=no algo.jl

const N = 2000
const TICKS = 20
const CELL = 10
const GRID = 64

@inline lcg(r::Int) = (r * 1103515245 + 12345) & 0x7fffffff
@inline unit(r::Int) = r / 2147483648.0
@inline cell_of(p::Float64)::Int = clamp(trunc(Int, p / CELL), 0, GRID - 1)

function integrate!(px, py, vx, vy)
    @inbounds for i in 1:N
        px[i] += vx[i]
        py[i] += vy[i]
        (px[i] < 0 || px[i] > 600) && (vx[i] = -vx[i])
        (py[i] < 0 || py[i] > 600) && (vy[i] = -vy[i])
    end
end

function rebuild!(heads::Vector{Int32}, nxt::Vector{Int32}, px, py)
    fill!(heads, Int32(-1))
    @inbounds for i in 1:N
        cell = cell_of(py[i]) * GRID + cell_of(px[i]) + 1
        nxt[i] = heads[cell]
        heads[cell] = i % Int32
    end
end

function collide!(hp::Vector{Int}, heads::Vector{Int32}, nxt::Vector{Int32}, px, py)
    @inbounds for i in 1:N
        cx = cell_of(px[i])
        cy = cell_of(py[i])
        hits = 0
        for oy in -1:1, ox in -1:1
            nx = cx + ox
            ny = cy + oy
            (nx < 0 || ny < 0 || nx >= GRID || ny >= GRID) && continue
            id = Int(heads[ny * GRID + nx + 1])
            while id >= 1
                if id != i
                    dx = Float32(px[i] - px[id])
                    dy = Float32(py[i] - py[id])
                    hits += (dx * dx + dy * dy < 4.0f0)
                end
                id = Int(nxt[id])
            end
        end
        hits > 3 && (hp[i] -= 1)
    end
end

function compute()::Int
    px = zeros(N); py = zeros(N); vx = zeros(N); vy = zeros(N)
    hp = zeros(Int, N)
    rng = 15482888
    @inbounds for i in 1:N
        rng = lcg(rng); px[i] = unit(rng) * 600.0
        rng = lcg(rng); py[i] = unit(rng) * 600.0
        rng = lcg(rng); vx[i] = (unit(rng) - 0.5) * 2.0
        rng = lcg(rng); vy[i] = (unit(rng) - 0.5) * 2.0
        rng = lcg(rng); hp[i] = 10 + rng % 20
    end
    heads = Vector{Int32}(undef, GRID * GRID)
    nxt = Vector{Int32}(undef, N)
    for _ in 1:TICKS
        integrate!(px, py, vx, vy)
        rebuild!(heads, nxt, px, py)
        collide!(hp, heads, nxt, px, py)
    end
    cs = 0
    @inbounds for i in 1:N
        cs += hp[i] + floor(Int, px[i] * 10.0) + floor(Int, py[i] * 10.0)
    end
    return cs
end

function main()
    # In-process wall time (excludes Julia process startup / sysimage load).
    t0 = time_ns()
    cs = compute()
    elapsed = (time_ns() - t0) / 1e9
    println("CHECKSUM=", cs)
    println("ELAPSED_S=", elapsed)
end

if abspath(PROGRAM_FILE) == @__FILE__
    # Default 2 warmups so timed ELAPSED_S reflects steady-state, not first-compile.
    warm = parse(Int, get(ENV, "JULIA_BENCH_WARM", "2"))
    for _ in 1:warm
        compute()
    end
    main()
end
