# 16_crypto_pq_argon2_suite — Argon2id-like KDF (m=8KiB t=2 p=1) x32, SHA-256 Merkle of tags,
# toy lattice negacyclic NTT poly-mul (q=12289, n=256) (Julia 1.13)
# Run: julia -O3 --check-bounds=no algo.jl

using SHA

const MOD = 1_000_000_007
const NH = 32
const BW = 64
const NB = 32
const PASSES = 2
const Q = 12289
const N = 256
const PAIRS = 6
const M32 = 0xFFFFFFFF

mutable struct Lcg
    s::Int
end
@inline function next!(g::Lcg)
    g.s = g.s * 48271 % 2147483647
    return g.s
end

@inline rot32(x::Int, r::Int) = ((x >> r) | (x << (32 - r))) & M32
@inline blamka(x::Int, y::Int) = (x + y + 2 * (x & 0xFFFF) * (y & 0xFFFF)) & M32

@inline function g_mix!(z::Vector{Int}, ia::Int, ib::Int, ic::Int, id::Int)
    @inbounds begin
        a, b, c, d = z[ia], z[ib], z[ic], z[id]
        a = blamka(a, b); d = rot32(d ⊻ a, 16)
        c = blamka(c, d); b = rot32(b ⊻ c, 12)
        a = blamka(a, b); d = rot32(d ⊻ a, 8)
        c = blamka(c, d); b = rot32(b ⊻ c, 7)
        z[ia] = a; z[ib] = b; z[ic] = c; z[id] = d
    end
    return nothing
end

# base is a 1-based index of the first word of the 8-word lane; s is the stride
function p_round!(z::Vector{Int}, base::Int, s::Int)
    g_mix!(z, base, base + 2s, base + 4s, base + 6s)
    g_mix!(z, base + s, base + 3s, base + 5s, base + 7s)
    g_mix!(z, base, base + 3s, base + 4s, base + 7s)
    g_mix!(z, base + s, base + 2s, base + 5s, base + 6s)
    return nothing
end

function compress!(out::Vector{Int}, x::Vector{Int}, y::Vector{Int}, r::Vector{Int})
    @inbounds begin
        for i in 1:BW
            r[i] = x[i] ⊻ y[i]
            out[i] = r[i]
        end
        for row in 0:7
            p_round!(out, row * 8 + 1, 1)
        end
        for col in 1:8
            p_round!(out, col, 8)
        end
        for i in 1:BW
            out[i] = out[i] ⊻ r[i]
        end
    end
    return nothing
end

function init_block(h0::Vector{UInt8}, j::Int)
    d = sha256(vcat(h0, UInt8[j]))
    w = zeros(Int, BW)
    @inbounds begin
        for k in 0:7
            w[k+1] = (Int(d[4k+1]) << 24) | (Int(d[4k+2]) << 16) | (Int(d[4k+3]) << 8) | Int(d[4k+4])
        end
        for k in 9:BW
            w[k] = (w[k-8] + (w[k-3] & 0xFFFF) * 40503 + (w[k-1] >> 16) * 69069) & M32
        end
    end
    return w
end

function argon2_like(idx_pw::Int, rng::Lcg)
    msg = zeros(UInt8, 36)
    msg[1], msg[2], msg[3], msg[4] = idx_pw, NB * 256 ÷ 1024, PASSES, 1
    for i in 5:36
        msg[i] = UInt8(next!(rng) & 255)
    end
    h0 = sha256(msg)
    b = [zeros(Int, BW) for _ in 1:NB]
    b[1] = init_block(h0, 0)
    b[2] = init_block(h0, 1)
    nw = zeros(Int, BW)
    r_tmp = zeros(Int, BW)
    for pass in 0:PASSES-1
        for idx in (pass == 0 ? 2 : 0):NB-1
            prev = b[(idx + NB - 1) % NB + 1]
            r = (pass == 0 && idx < NB ÷ 2) ?
                ((pass * 131 + idx) * 40503 + 12345) * 69069 % 4294967296 : prev[1]
            r16 = r >> 16
            x = (r16 * r16) >> 16
            if pass == 0
                area = idx - 1
                ref = area - 1 - ((area * x) >> 16)
            else
                area = NB - 2
                ref = (idx + 1 + (area - 1 - ((area * x) >> 16))) % NB
            end
            compress!(nw, prev, b[ref+1], r_tmp)
            if pass == 0
                copyto!(b[idx+1], nw)
            else
                @inbounds for i in 1:BW
                    b[idx+1][i] = b[idx+1][i] ⊻ nw[i]
                end
            end
        end
    end
    bytes = zeros(UInt8, BW * 4)
    for i in 0:BW-1
        w = b[NB][i+1]
        bytes[4i+1] = UInt8(w >> 24); bytes[4i+2] = UInt8((w >> 16) & 255)
        bytes[4i+3] = UInt8((w >> 8) & 255); bytes[4i+4] = UInt8(w & 255)
    end
    return sha256(bytes)
end

function pow_mod(b0::Int, e0::Int)
    r, b, e = 1, b0 % Q, e0
    while e > 0
        (e & 1 == 1) && (r = r * b % Q)
        b = b * b % Q
        e >>= 1
    end
    return r
end

# a is 1-based; the 0-based butterfly layout is preserved via +1 offsets
function ntt!(a::Vector{Int}, w::Int)
    j = 0
    @inbounds for i in 1:N-1
        bit = N >> 1
        while j & bit != 0
            j ⊻= bit
            bit >>= 1
        end
        j ⊻= bit
        if i < j
            a[i+1], a[j+1] = a[j+1], a[i+1]
        end
    end
    len = 2
    while len <= N
        wl = pow_mod(w, N ÷ len)
        half = len ÷ 2
        i = 0
        @inbounds while i < N
            wn = 1
            for k in 0:half-1
                u = a[i+k+1]
                v = a[i+k+half+1] * wn % Q
                a[i+k+1] = (u + v) % Q
                a[i+k+half+1] = (u + Q - v) % Q
                wn = wn * wl % Q
            end
            i += len
        end
        len <<= 1
    end
    return nothing
end

function polymul(a0::Vector{Int}, b0::Vector{Int}, psi::Int)
    psi_inv = pow_mod(psi, Q - 2)
    omega = psi * psi % Q
    n_inv = pow_mod(N, Q - 2)
    a, b, c = zeros(Int, N), zeros(Int, N), zeros(Int, N)
    p = 1
    for i in 1:N
        a[i] = a0[i] * p % Q
        b[i] = b0[i] * p % Q
        p = p * psi % Q
    end
    ntt!(a, omega)
    ntt!(b, omega)
    for i in 1:N
        c[i] = a[i] * b[i] % Q
    end
    ntt!(c, pow_mod(omega, Q - 2))
    p = n_inv
    for i in 1:N
        c[i] = c[i] * p % Q
        p = p * psi_inv % Q
    end
    return c
end

function schoolbook(a::Vector{Int}, b::Vector{Int})
    c = zeros(Int, N)
    @inbounds for i in 0:N-1, j in 0:N-1
        pr = a[i+1] * b[j+1] % Q
        k = i + j
        if k >= N
            c[k-N+1] = (c[k-N+1] + Q - pr) % Q
        else
            c[k+1] = (c[k+1] + pr) % Q
        end
    end
    return c
end

function compute()
    rng = Lcg(16001)
    level = [argon2_like(i, rng) for i in 0:NH-1]
    while length(level) > 1
        n = length(level)
        level = [sha256(vcat(level[2i+1], level[2i+2 <= n ? 2i+2 : 2i+1])) for i in 0:(n+1)÷2-1]
    end
    cs = 0
    for byte in level[1]
        cs = (cs * 131 + Int(byte)) % MOD
    end
    psi = pow_mod(11, (Q - 1) ÷ (2 * N))
    ok = pow_mod(psi, N) == Q - 1 ? 1 : 0
    for pr in 0:PAIRS-1
        a = [next!(rng) % Q for _ in 1:N]
        b = [next!(rng) % Q for _ in 1:N]
        c = polymul(a, b, psi)
        if pr < 2 && schoolbook(a, b) != c
            ok = 0
        end
        for v in c
            cs = (cs * 131 + v) % MOD
        end
    end
    return (cs * 131 + ok) % MOD
end

function main()
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
