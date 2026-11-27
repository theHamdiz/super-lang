# 02_mandelbrot_parallel — 1024×1024, maxiter 128, threaded rows (Julia 1.13)
# Run: julia -O3 --check-bounds=no -t auto algo.jl

const W = 1024
const H = 1024
const MAXIT = 128

@inline function row_iters(y::Int)::Int
    cy = (y / H) * 2.0 - 1.0
    local_sum = 0
    @inbounds for x in 0:W-1
        cx = (x / W) * 3.5 - 2.5
        zx = 0.0
        zy = 0.0
        it = 0
        while it < MAXIT
            zx2 = zx * zx
            zy2 = zy * zy
            zx2 + zy2 > 4.0 && break
            nzy = 2.0 * zx * zy + cy
            zx = zx2 - zy2 + cx
            zy = nzy
            it += 1
        end
        local_sum += it
    end
    return local_sum
end

function compute()::Int
    rows = Vector{Int}(undef, H)
    Threads.@threads :static for y in 0:H-1
        @inbounds rows[y+1] = row_iters(y)
    end
    return sum(rows)
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
