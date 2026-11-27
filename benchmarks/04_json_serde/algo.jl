# 04_json_serde — LCG field checksum (parity with C/Super) (Julia 1.13)
# Run: julia -O3 --check-bounds=no algo.jl

const N = 10000

@inline lcg(r::Int) = (r * 1103515245 + 12345) & 0x7fffffff

function compute()::Int
    rng = 4861952
    cs = 0
    for _ in 1:N
        rng = lcg(rng); id = rng % 1_000_000
        rng = lcg(rng); total = rng % 100_000
        rng = lcg(rng); items = rng % 8 + 1
        rec = id + total + items * 17
        cs += rec + rec
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
