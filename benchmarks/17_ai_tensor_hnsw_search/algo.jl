# 17_ai_tensor_hnsw_search — fixed-point (Q10) transformer blocks (causal attention, softmax,
# ReLU FFN, RMS norm) on 12 sequences, then brute-force top-10 cosine search over 4096 unit
# vectors of dim 32 using the pooled embeddings as queries. Integer-only => bit-exact everywhere. (Julia 1.13)
# Run: julia -O3 --check-bounds=no algo.jl

const MOD = 1_000_000_007
const BIAS = 1 << 40
const D = 32
const T = 16
const FF = 64
const LAYERS = 2
const SEQS = 12
const NV = 4096
const K = 10

mutable struct Lcg
    s::Int
end
@inline function next!(g::Lcg)
    g.s = g.s * 48271 % 2147483647
    return g.s
end

function isqrt_fp(n::Int)
    n == 0 && return 0
    x, y = n, (n + 1) >> 1
    while y < x
        x = y
        y = (x + n ÷ x) >> 1
    end
    return x
end

function exp_frac(f::Int)
    term, sum = 1024, 1024
    for k in 1:6
        term = ((term * f) >> 10) ÷ k
        sum = isodd(k) ? sum - term : sum + term
    end
    return max(sum, 0)
end

function exp_q(x::Int, e1::Int)
    a = -x
    n = a >> 10
    n >= 16 && return 0
    r = exp_frac(a & 1023)
    for _ in 1:n
        r = (r * e1) >> 10
    end
    return r
end

# y[t][o] = (sum_i x[t][i] * w[i][o]) >> 10 ; row-major flat vectors, 1-based
function matmul!(y::Vector{Int}, x::Vector{Int}, w::Vector{Int}, din::Int, dout::Int)
    @inbounds for t in 0:T-1, o in 0:dout-1
        s = 0
        for i in 0:din-1
            s += x[t*din+i+1] * w[i*dout+o+1]
        end
        y[t*dout+o+1] = s >> 10
    end
    return nothing
end

function rms_norm!(h::Vector{Int})
    @inbounds for t in 0:T-1
        ss = 0
        for d in 1:D
            ss += h[t*D+d] * h[t*D+d]
        end
        rms = isqrt_fp(ss ÷ D)
        inv = (1 << 20) ÷ (rms + 1)
        for d in 1:D
            h[t*D+d] = (h[t*D+d] * inv) >> 10
        end
    end
    return nothing
end

struct Layer
    wq::Vector{Int}
    wk::Vector{Int}
    wv::Vector{Int}
    wo::Vector{Int}
    w1::Vector{Int}
    w2::Vector{Int}
end

fill_w(rng::Lcg, n::Int) = [next!(rng) % 257 - 128 for _ in 1:n]

function block!(ly::Layer, h::Vector{Int}, e1::Int)
    q, k, v = zeros(Int, T * D), zeros(Int, T * D), zeros(Int, T * D)
    at, o, g = zeros(Int, T * D), zeros(Int, T * D), zeros(Int, T * D)
    f = zeros(Int, T * FF)
    matmul!(q, h, ly.wq, D, D)
    matmul!(k, h, ly.wk, D, D)
    matmul!(v, h, ly.wv, D, D)
    sc, pr = zeros(Int, T), zeros(Int, T)
    @inbounds for t in 0:T-1
        mx = typemin(Int)
        sum = 0
        for j in 0:t
            s = 0
            for d in 1:D
                s += q[t*D+d] * k[j*D+d]
            end
            sc[j+1] = s >> 13
            sc[j+1] > mx && (mx = sc[j+1])
        end
        for j in 0:t
            pr[j+1] = exp_q(sc[j+1] - mx, e1)
            sum += pr[j+1]
        end
        for j in 0:t
            pr[j+1] = pr[j+1] * 1024 ÷ sum
        end
        for d in 1:D
            s = 0
            for j in 0:t
                s += pr[j+1] * v[j*D+d]
            end
            at[t*D+d] = s >> 10
        end
    end
    matmul!(o, at, ly.wo, D, D)
    h .+= o
    rms_norm!(h)
    matmul!(f, h, ly.w1, D, FF)
    f .= max.(f, 0)
    matmul!(g, f, ly.w2, FF, D)
    h .+= g
    rms_norm!(h)
    return nothing
end

function normalize_fp(raw::Vector{Int})
    ss = 0
    for v in raw
        ss += v * v
    end
    inv = (1 << 20) ÷ (isqrt_fp(ss) + 1)
    return [(v * inv) >> 10 for v in raw]
end

function compute()
    rng = Lcg(17017)
    e1 = exp_frac(1024)
    layers = [Layer(fill_w(rng, D * D), fill_w(rng, D * D), fill_w(rng, D * D),
                    fill_w(rng, D * D), fill_w(rng, D * FF), fill_w(rng, FF * D)) for _ in 1:LAYERS]
    db = [normalize_fp([next!(rng) % 2049 - 1024 for _ in 1:D]) for _ in 1:NV]
    cs = 0
    fold(v) = (cs = (cs * 131 + v + BIAS) % MOD)
    for _ in 1:SEQS
        h = [next!(rng) % 2049 - 1024 for _ in 1:T*D]
        for ly in layers
            block!(ly, h, e1)
        end
        pooled = zeros(Int, D)
        for d in 1:D
            sum = 0
            for t in 0:T-1
                sum += h[t*D+d]
            end
            pooled[d] = sum >> 4
            fold(pooled[d])
        end
        query = normalize_fp(pooled)
        top_s, top_i = zeros(Int, K), zeros(Int, K)
        cnt = 0
        @inbounds for i in 0:NV-1
            row = db[i+1]
            sc = 0
            for d in 1:D
                sc += query[d] * row[d]
            end
            if cnt < K || sc > top_s[K]
                p = cnt < K ? cnt : K - 1
                while p > 0 && top_s[p] < sc
                    top_s[p+1] = top_s[p]
                    top_i[p+1] = top_i[p]
                    p -= 1
                end
                top_s[p+1] = sc
                top_i[p+1] = i
                cnt < K && (cnt += 1)
            end
        end
        for j in 1:K
            fold(top_i[j])
            fold(top_s[j])
        end
    end
    return cs
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
