# 09_matmul_tiled — 256×256 f64 tiled multiply, threaded over row tiles (Julia 1.13)
# Run: julia -O3 --check-bounds=no -t auto algo.jl

const N = 256
const TILE = 32

# Row-major flat storage (index i*N + j, 0-based -> +1), as in algo.c.
function matmul_tiled!(C::Vector{Float64}, A::Vector{Float64}, B::Vector{Float64})
    Threads.@threads :static for ti in 0:(N ÷ TILE)-1
        ii = ti * TILE
        @inbounds for jj in 0:TILE:N-1, kk in 0:TILE:N-1
            for i in ii:min(ii + TILE, N)-1
                for k in kk:min(kk + TILE, N)-1
                    aik = A[i*N+k+1]
                    @simd for j in jj:min(jj + TILE, N)-1
                        C[i*N+j+1] += aik * B[k*N+j+1]
                    end
                end
            end
        end
    end
    return C
end

function compute()::Int
    A = Vector{Float64}(undef, N * N)
    B = Vector{Float64}(undef, N * N)
    @inbounds for i in 0:N*N-1
        A[i+1] = ((i * 17) % 100) / 100.0
        B[i+1] = ((i * 31) % 100) / 100.0
    end
    C = zeros(N * N)
    matmul_tiled!(C, A, B)
    fro = 0.0
    @inbounds @simd for i in 1:N*N
        fro += C[i] * C[i]
    end
    return floor(Int, sqrt(fro) * 1000.0)
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
