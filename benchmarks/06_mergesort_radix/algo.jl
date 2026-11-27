# 06_mergesort_radix — sort 50000 u64, verify + checksum (Julia 1.13)
# Run: julia -O3 --check-bounds=no algo.jl

const N = 50000

@inline lcg(r::Int) = (r * 1103515245 + 12345) & 0x7fffffff

function merge_runs!(a::Vector{UInt64}, tmp::Vector{UInt64}, lo::Int, mid::Int, hi::Int)
    i, j, k = lo, mid, lo
    @inbounds begin
        while i < mid && j < hi
            if a[i] <= a[j]
                tmp[k] = a[i]; i += 1
            else
                tmp[k] = a[j]; j += 1
            end
            k += 1
        end
        while i < mid; tmp[k] = a[i]; i += 1; k += 1; end
        while j < hi;  tmp[k] = a[j]; j += 1; k += 1; end
        copyto!(a, lo, tmp, lo, hi - lo)
    end
    return nothing
end

# Half-open [lo, hi), 1-based.
function mergesort!(a::Vector{UInt64}, tmp::Vector{UInt64}, lo::Int, hi::Int)
    hi - lo <= 1 && return nothing
    mid = lo + (hi - lo) >> 1
    mergesort!(a, tmp, lo, mid)
    mergesort!(a, tmp, mid, hi)
    merge_runs!(a, tmp, lo, mid, hi)
end

function compute()::Int
    a = Vector{UInt64}(undef, N)
    tmp = similar(a)
    rng = 395799
    @inbounds for i in 1:N
        rng = lcg(rng)
        a[i] = UInt64(rng)
    end
    mergesort!(a, tmp, 1, N + 1)
    cs = 0
    ok = true
    @inbounds for i in 1:N
        v = a[i]
        cs ⊻= Int(v & 0xffffffff)
        cs = cs * 131 + Int(v >> 32)
        i > 1 && v < a[i-1] && (ok = false)
    end
    return cs ⊻ (ok ? 1 : 0)
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
