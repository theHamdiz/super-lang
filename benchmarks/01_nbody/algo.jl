# 01_nbody — 64 bodies × 500 steps, soft gravity, SoA (Julia 1.13)
# Run: julia -O3 --check-bounds=no algo.jl      (JULIA_BENCH_WARM=N adds N in-process warmup runs)

const N = 64
const STEPS = 500
const DT = 0.001
const SOFT = 0.01

@inline lcg(r::Int) = (r * 1103515245 + 12345) & 0x7fffffff
@inline unit(r::Int) = r / 2147483648.0

function step!(x::Vector{Float64}, y::Vector{Float64}, vx::Vector{Float64},
               vy::Vector{Float64}, m::Vector{Float64})
    @inbounds for i in 1:N
        xi, yi = x[i], y[i]
        ax = 0.0
        ay = 0.0
        # i == j contributes exactly 0 (dx == dy == 0), so the branch-free loop vectorizes.
        @simd for j in 1:N
            dx = x[j] - xi
            dy = y[j] - yi
            inv = 1.0 / sqrt(dx * dx + dy * dy + SOFT)
            inv3 = inv * inv * inv
            ax += m[j] * dx * inv3
            ay += m[j] * dy * inv3
        end
        vx[i] += ax * DT
        vy[i] += ay * DT
    end
    @inbounds @simd for i in 1:N
        x[i] += vx[i] * DT
        y[i] += vy[i] * DT
    end
    return nothing
end

function compute()::Int
    x = zeros(N); y = zeros(N); vx = zeros(N); vy = zeros(N); m = zeros(N)
    rng = 12648430
    @inbounds for i in 1:N
        rng = lcg(rng); x[i] = unit(rng) * 2.0 - 1.0
        rng = lcg(rng); y[i] = unit(rng) * 2.0 - 1.0
        rng = lcg(rng); vx[i] = (unit(rng) - 0.5) * 0.01
        rng = lcg(rng); vy[i] = (unit(rng) - 0.5) * 0.01
        rng = lcg(rng); m[i] = 0.5 + unit(rng)
    end
    for _ in 1:STEPS
        step!(x, y, vx, vy, m)
    end
    cs = 0
    @inbounds for i in 1:N
        cs += floor(Int, x[i] * 1000.0)
        cs += floor(Int, y[i] * 1000.0)
        cs += floor(Int, vx[i] * 1e6)
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
