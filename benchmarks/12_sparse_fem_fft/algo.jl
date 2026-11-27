# 12_sparse_fem_fft — 128x128 reaction-Poisson CSR, Jacobi-PCG x20, 2D FFT low-pass bank (Julia)
# Run: julia -O3 --check-bounds=no algo.jl

const G = 128
const N = 16384
const ITERS = 20

# 0-based CSR columns, 1-based storage (index + 1).
function build_system()
    indptr = zeros(Int32, N + 1)
    indices = zeros(Int32, 5 * N)
    values = zeros(Float64, 5 * N)
    diag = zeros(Float64, N)
    nz = 0
    for i in 0:N-1
        row, col = i ÷ G, i % G
        d = 4.0 + (i * 37 % 11) * 0.25
        diag[i+1] = d
        if row > 0; nz += 1; indices[nz] = i - G; values[nz] = -1.0; end
        if col > 0; nz += 1; indices[nz] = i - 1; values[nz] = -1.0; end
        nz += 1; indices[nz] = i; values[nz] = d
        if col < G - 1; nz += 1; indices[nz] = i + 1; values[nz] = -1.0; end
        if row < G - 1; nz += 1; indices[nz] = i + G; values[nz] = -1.0; end
        indptr[i+2] = nz
    end
    return indptr, indices, values, diag
end

function matvec!(out, indptr, indices, values, x)
    @inbounds for r in 1:N
        s = 0.0
        for k in (indptr[r]+1):indptr[r+1]
            s += values[k] * x[indices[k]+1]
        end
        out[r] = s
    end
end

function dot_seq(a, b)
    s = 0.0
    @inbounds for i in 1:N
        s += a[i] * b[i]
    end
    return s
end

function pcg!(x, b, indptr, indices, values, diag)
    r = copy(b); z = similar(b); p = similar(b); ap = zeros(N)
    @inbounds for i in 1:N
        x[i] = 0.0; z[i] = r[i] / diag[i]; p[i] = z[i]
    end
    rz = dot_seq(r, z)
    for _ in 1:ITERS
        matvec!(ap, indptr, indices, values, p)
        alpha = rz / dot_seq(p, ap)
        @inbounds for i in 1:N
            x[i] += alpha * p[i]
            r[i] -= alpha * ap[i]
            z[i] = r[i] / diag[i]
        end
        rz_new = dot_seq(r, z)
        beta = rz_new / rz
        @inbounds for i in 1:N
            p[i] = z[i] + beta * p[i]
        end
        rz = rz_new
    end
    return dot_seq(r, r)
end

const BITREV = let t = zeros(Int, G)
    for i in 0:G-1
        r = 0; v = i
        for _ in 1:7
            r = (r << 1) | (v & 1); v >>= 1
        end
        t[i+1] = r + 1
    end
    t
end

const TW = let t = Vector{Tuple{Vector{Float64},Vector{Float64}}}()
    span = 2
    while span <= G
        ang = 0.0 - 2.0 * pi / span
        push!(t, ([cos(ang * k) for k in 0:span÷2-1], [sin(ang * k) for k in 0:span÷2-1]))
        span *= 2
    end
    t
end

function fft1!(re, im, tre, tim, inv::Bool)
    if inv
        @inbounds for i in 1:G; im[i] = 0.0 - im[i]; end
    end
    @inbounds for i in 1:G
        s = BITREV[i]; tre[i] = re[s]; tim[i] = im[s]
    end
    span = 2; lvl = 1
    @inbounds while span <= G
        half = span ÷ 2
        wrt, wit = TW[lvl]
        for start in 0:span:G-1
            for k in 0:half-1
                wr = wrt[k+1]; wi = wit[k+1]
                a = start + k + 1; b = a + half
                tr = tre[b] * wr - tim[b] * wi
                ti = tre[b] * wi + tim[b] * wr
                tre[b] = tre[a] - tr; tim[b] = tim[a] - ti
                tre[a] = tre[a] + tr; tim[a] = tim[a] + ti
            end
        end
        span *= 2; lvl += 1
    end
    if inv
        s = 1.0 / G
        @inbounds for i in 1:G
            re[i] = tre[i] * s; im[i] = (0.0 - tim[i]) * s
        end
    else
        @inbounds for i in 1:G
            re[i] = tre[i]; im[i] = tim[i]
        end
    end
end

function fft2!(re, im, inv::Bool)
    lr = zeros(G); li = zeros(G); tre = zeros(G); tim = zeros(G)
    for r in 0:G-1
        @inbounds for c in 1:G; lr[c] = re[r*G+c]; li[c] = im[r*G+c]; end
        fft1!(lr, li, tre, tim, inv)
        @inbounds for c in 1:G; re[r*G+c] = lr[c]; im[r*G+c] = li[c]; end
    end
    for c in 1:G
        @inbounds for r in 0:G-1; lr[r+1] = re[r*G+c]; li[r+1] = im[r*G+c]; end
        fft1!(lr, li, tre, tim, inv)
        @inbounds for r in 0:G-1; re[r*G+c] = lr[r+1]; im[r*G+c] = li[r+1]; end
    end
end

function compute()::Int
    b = [(((i * 7 + (i ÷ G) * 13) % 17) - 8) / 8.0 for i in 0:N-1]
    x = zeros(N)
    indptr, indices, values, diag = build_system()
    res = sqrt(pcg!(x, b, indptr, indices, values, diag))

    sre = copy(x); sim = zeros(N)
    fft2!(sre, sim, false)

    cuts = (64, 256, 1024)
    fre = zeros(N); fim = zeros(N)
    energy = 0.0; corner = 0.0
    for q in 1:3
        copyto!(fre, sre); copyto!(fim, sim)
        for k in 0:N-1
            ky, kx = k ÷ G, k % G
            fy = ky <= G ÷ 2 ? ky : G - ky
            fx = kx <= G ÷ 2 ? kx : G - kx
            if fx * fx + fy * fy > cuts[q]
                fre[k+1] = 0.0; fim[k+1] = 0.0
            end
        end
        fft2!(fre, fim, true)
        for k in 1:N
            energy += fre[k] * fre[k]
        end
        corner += fre[1] + fre[N ÷ 2 + G ÷ 2 + 1] + fre[N]
    end

    xs = dot_seq(x, x)
    return floor(Int, sqrt(xs) * 1000000.0) + floor(Int, energy * 1000.0) +
           floor(Int, corner * 1000000.0) + floor(Int, res * 1000000000.0)
end

function main()
    t0 = time_ns()
    cs = compute()
    elapsed = (time_ns() - t0) / 1e9
    println("CHECKSUM=", cs)
    println("ELAPSED_S=", elapsed)
end

if abspath(PROGRAM_FILE) == @__FILE__
    warm = parse(Int, get(ENV, "JULIA_BENCH_WARM", "2"))
    for _ in 1:warm
        compute()
    end
    main()
end
