# 07_crc32_lz4_compress — IEEE CRC32 on expanded LCG-scrambled block (Julia 1.13)
# Run: julia -O3 --check-bounds=no algo.jl

const SEED = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.."

@inline lcg(r::Int) = (r * 1103515245 + 12345) & 0x7fffffff

function crc_table()::Vector{UInt32}
    t = Vector{UInt32}(undef, 256)
    for i in 0:255
        c = UInt32(i)
        for _ in 1:8
            c = (c & 1) != 0 ? (0xedb88320 ⊻ (c >> 1)) : (c >> 1)
        end
        t[i+1] = c
    end
    return t
end

function crc32(table::Vector{UInt32}, data::Vector{UInt8})::UInt32
    crc = 0xffffffff
    @inbounds for b in data
        crc = table[Int((crc ⊻ b) & 0xff)+1] ⊻ (crc >> 8)
    end
    return crc ⊻ 0xffffffff
end

function compute()::Int
    table = crc_table()
    seed = Vector{UInt8}(codeunits(SEED))
    block = repeat(seed, 64)               # 6 doublings
    block_len = length(block)
    rng = 8137778
    acc = 0
    @inbounds for i in 0:4095
        rng = lcg(rng)
        acc += rng % 255 + Int(block[i % block_len + 1])
    end
    payload = repeat(block, 9)
    return Int(crc32(table, payload)) + Int(crc32(table, block)) + acc % 1000
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
