# 10_sha256_merkle_tree — 1024 leaves × 64 bytes, SHA-256 Merkle root (Julia 1.13)
# Run: julia -O3 --check-bounds=no -t auto algo.jl

const LEAVES = 1024
const LEAF_SZ = 64

const K = UInt32[
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
    0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
    0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
    0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
    0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
    0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
    0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
    0xc67178f2]

@inline rotr(x::UInt32, n::Int) = (x >> n) | (x << (32 - n))

# Per-task scratch: padded message buffer (<= 128 B for <= 119 B msgs) and schedule.
struct Scratch
    buf::Vector{UInt8}
    w::Vector{UInt32}
end
Scratch() = Scratch(zeros(UInt8, 192), zeros(UInt32, 64))

# SHA-256 of msg[1:len]; digest (32 B) written to out[o+1:o+32].
function sha256!(out::Vector{UInt8}, o::Int, msg::AbstractVector{UInt8}, len::Int, s::Scratch)
    buf, w = s.buf, s.w
    h0 = 0x6a09e667; h1 = 0xbb67ae85; h2 = 0x3c6ef372; h3 = 0xa54ff53a
    h4 = 0x510e527f; h5 = 0x9b05688c; h6 = 0x1f83d9ab; h7 = 0x5be0cd19
    new_len = len + 1
    while new_len % 64 != 56
        new_len += 1
    end
    total = new_len + 8
    @inbounds begin
        fill!(view(buf, 1:total), 0x00)
        copyto!(buf, 1, msg, 1, len)
        buf[len+1] = 0x80
        bits = UInt64(len) * 8
        for i in 0:7
            buf[new_len+i+1] = UInt8((bits >> (56 - 8i)) & 0xff)
        end
        for off in 0:64:total-1
            for i in 0:15
                b = off + 4i
                w[i+1] = (UInt32(buf[b+1]) << 24) | (UInt32(buf[b+2]) << 16) |
                         (UInt32(buf[b+3]) << 8) | UInt32(buf[b+4])
            end
            for i in 16:63
                w15 = w[i-14]; w2 = w[i-1]
                s0 = rotr(w15, 7) ⊻ rotr(w15, 18) ⊻ (w15 >> 3)
                s1 = rotr(w2, 17) ⊻ rotr(w2, 19) ⊻ (w2 >> 10)
                w[i+1] = w[i-15] + s0 + w[i-6] + s1
            end
            a, b, c, d, e, f, g, h = h0, h1, h2, h3, h4, h5, h6, h7
            for i in 1:64
                S1 = rotr(e, 6) ⊻ rotr(e, 11) ⊻ rotr(e, 25)
                ch = (e & f) ⊻ (~e & g)
                t1 = h + S1 + ch + K[i] + w[i]
                S0 = rotr(a, 2) ⊻ rotr(a, 13) ⊻ rotr(a, 22)
                maj = (a & b) ⊻ (a & c) ⊻ (b & c)
                t2 = S0 + maj
                h = g; g = f; f = e; e = d + t1
                d = c; c = b; b = a; a = t1 + t2
            end
            h0 += a; h1 += b; h2 += c; h3 += d
            h4 += e; h5 += f; h6 += g; h7 += h
        end
        hs = (h0, h1, h2, h3, h4, h5, h6, h7)
        for i in 1:8
            v = hs[i]
            out[o+4i-3] = UInt8(v >> 24)
            out[o+4i-2] = UInt8((v >> 16) & 0xff)
            out[o+4i-1] = UInt8((v >> 8) & 0xff)
            out[o+4i]   = UInt8(v & 0xff)
        end
    end
    return nothing
end

# Run body(i, scratch, tmp64) for i in 0:n-1, split into one contiguous chunk per worker.
function parallel_chunks(body::F, n::Int) where {F}
    nchunks = n >= 64 ? Threads.nthreads() : 1
    Threads.@threads :static for c in 1:nchunks
        scratch = Scratch()
        tmp = Vector{UInt8}(undef, 64)
        lo = (c - 1) * n ÷ nchunks
        hi = c * n ÷ nchunks
        for i in lo:hi-1
            body(i, scratch, tmp)
        end
    end
    return nothing
end

function compute()::Int
    level = zeros(UInt8, 32 * LEAVES)
    nxt = zeros(UInt8, 32 * LEAVES)

    parallel_chunks(LEAVES) do i, scratch, leaf
        @inbounds for b in 0:LEAF_SZ-1
            leaf[b+1] = UInt8((i * 131 + b * 17) & 255)
        end
        sha256!(level, 32i, leaf, LEAF_SZ, scratch)
    end

    n = LEAVES
    while n > 1
        m = (n + 1) ÷ 2
        nn = n
        parallel_chunks(m) do i, scratch, cat
            @inbounds begin
                copyto!(cat, 1, level, 32 * (2i) + 1, 32)
                src = 2i + 1 < nn ? 2i + 1 : 2i
                copyto!(cat, 33, level, 32 * src + 1, 32)
            end
            sha256!(nxt, 32i, cat, 64, scratch)
        end
        copyto!(level, 1, nxt, 1, 32m)
        n = m
    end

    cs = 0
    @inbounds for i in 1:32
        cs = cs * 131 + Int(level[i])
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
