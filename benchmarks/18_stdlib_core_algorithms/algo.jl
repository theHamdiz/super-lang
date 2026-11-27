# 18_stdlib_core_algorithms — one checksum from four std-style kernels:
# (A) KMP-DFA "regex" field extraction over 3000 generated log lines,
# (B) strict UTF-8 validation/counting over 60 documents (some corrupted),
# (C) SQL-ish filter / group-by / order-by-limit / join over 6000 rows x 24 queries,
# (D) HM-style type unification (union-find, iterative) over 8504 type nodes. (Julia 1.13)
# Run: julia -O3 --check-bounds=no algo.jl

const MOD = 1_000_000_007
const NL = 3000
const NDOC = 60
const PERDOC = 200
const NR = 6000
const NQ = 24
const NVARS = 2500
const NCOMP = 6000
const NCON = 5000
const NTOT = NVARS + 4 + NCOMP

mutable struct Ctx
    rng::Int
    cs::Int
end
@inline function next!(c::Ctx)
    c.rng = c.rng * 48271 % 2147483647
    return c.rng
end
@inline fold!(c::Ctx, v::Int) = (c.cs = (c.cs * 131 + v) % MOD)

# ---------- (A) log extraction ----------
function build_dfa(needle::Vector{UInt8})
    n = length(needle)
    dfa = zeros(Int, n * 256)          # dfa[state*256 + byte + 1], 0-based state/byte
    dfa[Int(needle[1])+1] = 1
    x = 0
    for j in 1:n-1
        for c in 0:255
            dfa[j*256+c+1] = dfa[x*256+c+1]
        end
        nj = Int(needle[j+1])
        dfa[j*256+nj+1] = j + 1
        x = dfa[x*256+nj+1]
    end
    return dfa
end

function extract(line::Vector{UInt8}, dfa::Vector{Int}, n::Int)
    st, i, e = 0, 1, length(line)
    @inbounds while i <= e
        st = dfa[st*256+Int(line[i])+1]
        i += 1
        if st == n
            v, nd = 0, 0
            while i <= e && 0x30 <= line[i] <= 0x39
                v = v * 10 + Int(line[i]) - 48
                nd += 1
                i += 1
            end
            nd > 0 && return v
            st = 0
        end
    end
    return -1
end

pad2(v::Int) = lpad(string(v), 2, '0')

function part_a!(c::Ctx)
    levels = ("INFO", "WARN", "ERROR", "DEBUG")
    codes = (200, 201, 204, 301, 400, 401, 404, 500, 503)
    d_status, d_took, d_user = build_dfa(Vector{UInt8}("status=")), build_dfa(Vector{UInt8}("took=")), build_dfa(Vector{UInt8}("user="))
    cls = zeros(Int, 8)
    took_sum = took_max = took_n = user_sum = bad = total = 0
    for i in 0:NL-1
        mo = 1 + next!(c) % 12; dd = 1 + next!(c) % 28; hh = next!(c) % 24
        mi = next!(c) % 60; ss = next!(c) % 60; lv = next!(c) % 4
        svc = next!(c) % 100; uid = next!(c) % 100000; sidx = next!(c) % 9; ms = next!(c) % 3000
        s = "2026-$(pad2(mo))-$(pad2(dd)) $(pad2(hh)):$(pad2(mi)):$(pad2(ss)) $(levels[lv+1]) svc-$(pad2(svc)) user=$uid"
        s *= i % 13 == 5 ? " status=abc" : " status=$(codes[sidx+1])"
        i % 17 != 3 && (s *= " took=$(ms)ms")
        line = Vector{UInt8}(s)
        st, tk, us = extract(line, d_status, 7), extract(line, d_took, 5), extract(line, d_user, 5)
        if st < 0
            bad += 1
        else
            cls[st÷100+1] += 1
        end
        if tk >= 0
            took_sum += tk
            took_n += 1
            took_max = max(took_max, tk)
        end
        user_sum += us
        total += length(line) + 1
    end
    for v in cls
        fold!(c, v)
    end
    for v in (took_sum, took_max, took_n, user_sum, bad, total)
        fold!(c, v)
    end
end

# ---------- (B) UTF-8 ----------
@inline cont(b::Integer) = (b & 0xC0) == 0x80

function utf8_valid(d::Vector{Int}, n::Int)
    i = 1
    @inbounds while i <= n
        b0 = d[i]
        if b0 < 128
            i += 1
        elseif b0 < 194
            return false
        elseif b0 < 224
            (i + 1 > n || !cont(d[i+1])) && return false
            i += 2
        elseif b0 < 240
            (i + 2 > n || !cont(d[i+1]) || !cont(d[i+2])) && return false
            cp = (b0 - 224) * 4096 + (d[i+1] - 128) * 64 + (d[i+2] - 128)
            (cp < 2048 || 55296 <= cp <= 57343) && return false
            i += 3
        elseif b0 < 245
            (i + 3 > n || !cont(d[i+1]) || !cont(d[i+2]) || !cont(d[i+3])) && return false
            cp = (b0 - 240) * 262144 + (d[i+1] - 128) * 4096 + (d[i+2] - 128) * 64 + (d[i+3] - 128)
            (cp < 65536 || cp > 1114111) && return false
            i += 4
        else
            return false
        end
    end
    return true
end

utf8_count(d::Vector{Int}, n::Int) = count(i -> !cont(d[i]), 1:n)

function encode!(cp::Int, o::Vector{Int}, at::Int)   # at is 1-based
    if cp < 128
        o[at] = cp
        return 1
    elseif cp < 2048
        o[at], o[at+1] = 192 + cp ÷ 64, 128 + cp % 64
        return 2
    elseif cp < 65536
        o[at], o[at+1], o[at+2] = 224 + cp ÷ 4096, 128 + cp ÷ 64 % 64, 128 + cp % 64
        return 3
    end
    o[at], o[at+1] = 240 + cp ÷ 262144, 128 + cp ÷ 4096 % 64
    o[at+2], o[at+3] = 128 + cp ÷ 64 % 64, 128 + cp % 64
    return 4
end

function part_b!(c::Ctx)
    badb = (0x80, 0xC0, 0xC1, 0xF5, 0xFF, 0xE0)
    doc = zeros(Int, PERDOC * 4)
    valid_docs = total_cp = 0
    for d in 0:NDOC-1
        n = 0
        for _ in 1:PERDOC
            cat = next!(c) % 4
            r = next!(c)
            cp = cat == 0 ? r % 128 : cat == 1 ? 128 + r % 1920 : cat == 2 ? 2048 + r % 53248 : 65536 + r % 1048576
            n += encode!(cp, doc, n + 1)
        end
        if d % 5 == 2
            p = next!(c) % n
            b = next!(c) % 6
            doc[p+1] = badb[b+1]
        end
        ok = utf8_valid(doc, n) ? 1 : 0
        cnt = ok == 1 ? utf8_count(doc, n) : 0
        fold!(c, ok); fold!(c, cnt); fold!(c, n)
        valid_docs += ok
        total_cp += cnt
    end
    fold!(c, valid_docs)
    fold!(c, total_cp)
end

# ---------- (C) SQL-ish ----------
function part_c!(c::Ctx)
    dept, age, sal, act = zeros(Int, NR), zeros(Int, NR), zeros(Int, NR), zeros(Int, NR)
    for i in 1:NR
        dept[i] = next!(c) % 16
        age[i] = 18 + next!(c) % 50
        sal[i] = 30000 + next!(c) % 90000
        act[i] = next!(c) % 10 != 0 ? 1 : 0
    end
    for _ in 1:NQ
        age_lo = 18 + next!(c) % 30
        sal_max = 40000 + next!(c) % 80000
        need = next!(c) % 2
        cnt, sum, mx, taken = zeros(Int, 16), zeros(Int, 16), zeros(Int, 16), falses(16)
        @inbounds for i in 1:NR
            if age[i] >= age_lo && sal[i] < sal_max && act[i] >= need
                g = dept[i] + 1
                cnt[g] += 1
                sum[g] += sal[i]
                age[i] > mx[g] && (mx[g] = age[i])
            end
        end
        for _ in 1:3
            best = 0
            for g in 1:16
                if cnt[g] >= 5 && !taken[g] && (best == 0 || sum[g] > sum[best])
                    best = g
                end
            end
            if best > 0
                taken[best] = true
                fold!(c, best - 1); fold!(c, cnt[best]); fold!(c, sum[best]); fold!(c, mx[best])
                fold!(c, sum[best] ÷ (best * 1000))
            else
                fold!(c, 999)
            end
        end
    end
end

# ---------- (D) HM unification ----------
mutable struct Types
    kind::Vector{Int}
    na::Vector{Int}
    nb::Vector{Int}
    parent::Vector{Int}
    stk_a::Vector{Int}
    stk_b::Vector{Int}
    unions::Int
end

# node ids are 0-based; arrays are indexed by id + 1
function find!(t::Types, x::Int)
    @inbounds while t.parent[x+1] != x
        t.parent[x+1] = t.parent[t.parent[x+1]+1]
        x = t.parent[x+1]
    end
    return x
end

function unify!(t::Types, x::Int, y::Int)
    sp = 1
    t.stk_a[1], t.stk_b[1] = x, y
    while sp > 0
        a, b = t.stk_a[sp], t.stk_b[sp]
        sp -= 1
        rx, ry = find!(t, a), find!(t, b)
        rx == ry && continue
        kx, ky = t.kind[rx+1], t.kind[ry+1]
        if kx == 0
            t.parent[rx+1] = ry
            t.unions += 1
        elseif ky == 0
            t.parent[ry+1] = rx
            t.unions += 1
        elseif kx == 1 || ky == 1
            if kx == 1 && ky == 1 && t.na[rx+1] == t.na[ry+1]
                t.parent[ry+1] = rx
                t.unions += 1
            else
                return false
            end
        else
            t.parent[ry+1] = rx
            t.unions += 1
            sp += 1
            t.stk_a[sp], t.stk_b[sp] = t.na[rx+1], t.na[ry+1]
            sp += 1
            t.stk_a[sp], t.stk_b[sp] = t.nb[rx+1], t.nb[ry+1]
        end
    end
    return true
end

function part_d!(c::Ctx)
    t = Types(zeros(Int, NTOT), fill(-1, NTOT), fill(-1, NTOT), collect(0:NTOT-1),
              zeros(Int, 2 * NTOT + 16), zeros(Int, 2 * NTOT + 16), 0)
    for k in 0:3
        t.kind[NVARS+k+1] = 1
        t.na[NVARS+k+1] = k
    end
    for i in NVARS+4:NTOT-1
        t.kind[i+1] = 2
        t.na[i+1] = next!(c) % i
        t.nb[i+1] = next!(c) % i
    end
    fails = 0
    for _ in 1:NCON
        tsel = next!(c) % 2
        x = tsel == 0 ? next!(c) % NVARS : next!(c) % NTOT
        y = next!(c) % NTOT
        unify!(t, x, y) || (fails += 1)
    end
    fold!(c, fails)
    fold!(c, t.unions)
    classes = 0
    for i in 0:NTOT-1
        r = find!(t, i)
        r == i && (classes += 1)
        fold!(c, r * 3 + t.kind[r+1])
    end
    fold!(c, classes)
end

function compute()
    c = Ctx(18018, 0)
    part_a!(c)
    part_b!(c)
    part_c!(c)
    part_d!(c)
    return c.cs
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
