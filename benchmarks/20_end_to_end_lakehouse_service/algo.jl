# 20_end_to_end_lakehouse_service — in-process pipeline:
# ingest JSONL customers + CSV order batches -> filter/join/group -> fixed-point GD linear fit on the
# per-product aggregates -> serve HTTP-like request bytes (bearer-token check via SHA-256, Merkle root of
# the group table in every response) -> checksum of response bytes. (Julia 1.13)
# Run: julia -O3 --check-bounds=no algo.jl
using SHA

const MOD = 1_000_000_007
const NCUST, NPROD, NREG, NTIER = 500, 50, 8, 4
const NGRP = NREG * NTIER
const BATCHES, ROWS_PB, NREQ, NCLIENT, ITERS = 6, 4000, 3000, 8, 400

sdiv(a::Int, d::Int) = a >= 0 ? a ÷ d : -((-a) ÷ d)

const CRC = let t = zeros(UInt32, 256)
    for i in 0:255
        c = UInt32(i)
        for _ in 1:8
            c = (c & 1) == 1 ? (c >> 1) ⊻ 0xEDB88320 : c >> 1
        end
        t[i+1] = c
    end
    t
end
function crc32(d::Vector{UInt8})
    c = 0xFFFFFFFF
    @inbounds for b in d
        c = CRC[Int((c ⊻ b) & 0xFF)+1] ⊻ (c >> 8)
    end
    return Int(c ⊻ 0xFFFFFFFF)
end

hex(d) = join(string(b, base = 16, pad = 2) for b in d)

# Parse the decimal run at 1-based p; returns (value or -1, new p).
function scan_int(b::Vector{UInt8}, p::Int)
    v = 0
    have = false
    n = length(b)
    while p <= n && 0x30 <= b[p] <= 0x39
        v = v * 10 + Int(b[p]) - 48
        p += 1
        have = true
    end
    return (have ? v : -1), p
end
isdig(c::UInt8) = 0x30 <= c <= 0x39

# 1-based start of the first occurrence of pat in b at or after `from`, or 0.
function index_of(b::Vector{UInt8}, pat::Vector{UInt8}, from::Int)
    m = length(pat)
    for i in from:(length(b)-m+1)
        match = true
        for j in 1:m
            if b[i+j-1] != pat[j]
                match = false
                break
            end
        end
        match && return i
    end
    return 0
end

function compute()
    rng = 20020
    next_rand() = (rng = rng * 48271 % 2147483647)
    cs = 0
    fold(v) = (cs = (cs * 131 + v) % MOD)

    # ---- ingest customers (JSONL) ----
    io = IOBuffer()
    for i in 0:NCUST-1
        id = i * 7919 % NCUST
        r = next_rand() % NREG
        t = next_rand() % NTIER
        print(io, "{\"id\":", id, ",\"region\":", r, ",\"tier\":", t, "}\n")
    end
    cb = take!(io)
    clen = length(cb)
    push!(cb, 0x00)
    region_of = fill(-1, NCUST)
    tier_of = fill(-1, NCUST)
    ncust = 0
    p = 1
    while p <= clen
        while p <= clen && !isdig(cb[p])
            p += 1
        end
        id, p = scan_int(cb, p)
        while !isdig(cb[p])
            p += 1
        end
        r, p = scan_int(cb, p)
        while !isdig(cb[p])
            p += 1
        end
        t, p = scan_int(cb, p)
        region_of[id+1] = r
        tier_of[id+1] = t
        ncust += 1
        while p <= clen && cb[p] != 0x0a
            p += 1
        end
        p += 1
    end

    # ---- ingest + filter/join/group order batches (CSV) ----
    grp_cnt, grp_qty, grp_rev = zeros(Int, NGRP), zeros(Int, NGRP), zeros(Int, NGRP)
    prod_cnt, prod_rev, prod_price = zeros(Int, NPROD), zeros(Int, NPROD), zeros(Int, NPROD)
    reg_prod_rev = zeros(Int, NREG * NPROD)
    oid = rows = null_cust = bad_qty = valid = 0
    f = zeros(Int, 5)
    for _ in 1:BATCHES
        io = IOBuffer()
        print(io, "order_id,cust_id,product,qty,price_cents\n")
        for _ in 1:ROWS_PB
            nf = next_rand() % 100
            c = next_rand() % NCUST
            pr = next_rand() % NPROD
            q = next_rand() % 9
            price = 100 + pr * 37 + next_rand() % 200
            print(io, oid, ",", nf != 0 ? string(c) : "", ",", pr, ",", q, ",", price, "\n")
            oid += 1
        end
        ob = take!(io)
        olen = length(ob)
        push!(ob, 0x00)
        pos = findfirst(==(0x0a), ob) + 1
        while pos <= olen
            for k in 1:5
                f[k], pos = scan_int(ob, pos)
                pos += 1
            end
            rows += 1
            if f[2] < 0
                null_cust += 1
                continue
            end
            if f[4] <= 0
                bad_qty += 1
                continue
            end
            valid += 1
            rev = f[4] * f[5]
            reg = region_of[f[2]+1]
            g = reg * NTIER + tier_of[f[2]+1]
            grp_cnt[g+1] += 1
            grp_qty[g+1] += f[4]
            grp_rev[g+1] += rev
            pd = f[3]
            prod_cnt[pd+1] += 1
            prod_rev[pd+1] += rev
            prod_price[pd+1] += f[5]
            reg_prod_rev[reg*NPROD+pd+1] += rev
        end
    end

    # ---- fit y = w*x + b (w in Q16) by gradient descent on per-product aggregates ----
    xs, ys = Int[], Int[]
    for p in 1:NPROD
        if prod_cnt[p] > 0
            push!(xs, prod_price[p] ÷ prod_cnt[p] ÷ 16)
            push!(ys, prod_rev[p] ÷ prod_cnt[p] ÷ 16)
        end
    end
    n = length(xs)
    w = bias = 0
    for _ in 1:ITERS
        gw = gb = 0
        for i in 1:n
            err = sdiv(w * xs[i], 65536) + bias - ys[i]
            gw += err * xs[i]
            gb += err
        end
        w -= sdiv(gw * 65536, n * 8000)
        bias -= sdiv(gb, n * 4)
    end
    loss = 0
    for i in 1:n
        err = sdiv(w * xs[i], 65536) + bias - ys[i]
        loss += err * err
    end
    pred_cents = zeros(Int, NPROD)
    for p in 1:NPROD
        x = prod_cnt[p] > 0 ? prod_price[p] ÷ prod_cnt[p] ÷ 16 : 0
        y = sdiv(w * x, 65536) + bias
        pred_cents[p] = max(y, 0) * 16
    end

    # ---- merkle root over the region x tier group table ----
    level = Vector{Vector{UInt8}}()
    for g in 0:NGRP-1
        leaf = UInt8[]
        for v in (g, grp_cnt[g+1], grp_qty[g+1], grp_rev[g+1])
            append!(leaf, reinterpret(UInt8, [hton(UInt64(v))]))
        end
        push!(level, sha256(leaf))
    end
    while length(level) > 1
        level = [sha256(vcat(level[2i-1], level[2i])) for i in 1:length(level)÷2]
    end
    root = level[1]
    root_hex = hex(root)

    # ---- client tokens ----
    tokens = [Vector{UInt8}(codeunits(hex(sha256(codeunits("key-$k"))[1:8]))) for k in 0:NCLIENT-1]

    # ---- serve ----
    ok200 = un401 = nf404 = total_bytes = resp_cs = 0
    p_xc, p_bearer = Vector{UInt8}("X-Client: "), Vector{UInt8}("Bearer ")
    p_get = Vector{UInt8}("GET /v1/product/")
    for _ in 1:NREQ
        k = next_rand() % NCLIENT
        pq = next_rand() % 60
        rg = next_rand() % NREG
        bad = next_rand() % 100
        at = next_rand() % 16
        tok = copy(tokens[k+1])
        bad < 8 && (tok[at+1] = 0x7a)
        req = vcat(Vector{UInt8}("GET /v1/product/$pq?region=$rg HTTP/1.1\r\nHost: lake\r\nX-Client: $k\r\nAuthorization: Bearer "),
            tok, Vector{UInt8}("\r\n\r\n"))
        rl = length(req)
        # server side: parse request bytes
        status = 400
        pp = rr = -1
        if rl > 16 && req[1:16] == p_get
            q = 17
            pp, q = scan_int(req, q)
            q += 8
            rr, q = scan_int(req, q)
            xc = index_of(req, p_xc, q)
            bt = index_of(req, p_bearer, q)
            if xc > 0 && bt > 0
                kk, _ = scan_int(req, xc + 10)
                good = 0 <= kk < NCLIENT && bt + 6 + 16 <= rl && req[bt+7:bt+22] == tokens[kk+1]
                if good
                    status = (0 <= pp < NPROD && 0 <= rr < NREG && prod_cnt[pp+1] > 0) ? 200 : 404
                else
                    status = 401
                end
            end
        end
        if status == 200
            body = "{\"product\":$pp,\"region\":$rr,\"orders\":$(prod_cnt[pp+1]),\"revenue\":$(prod_rev[pp+1])," *
                   "\"region_revenue\":$(reg_prod_rev[rr*NPROD+pp+1]),\"forecast\":$(pred_cents[pp+1]),\"root\":\"$root_hex\"}"
            ok200 += 1
            head = "HTTP/1.1 200 OK"
        elseif status == 404
            body = "{\"error\":\"not found\"}"
            nf404 += 1
            head = "HTTP/1.1 404 Not Found"
        elseif status == 401
            body = "{\"error\":\"unauthorized\"}"
            un401 += 1
            head = "HTTP/1.1 401 Unauthorized"
        else
            body = "{\"error\":\"bad request\"}"
            head = "HTTP/1.1 400 Bad Request"
        end
        resp = Vector{UInt8}("$head\r\nContent-Length: $(length(body))\r\n\r\n$body")
        total_bytes += length(resp)
        resp_cs = (resp_cs * 131 + crc32(resp)) % MOD
    end

    # ---- checksum ----
    for v in (ncust, rows, null_cust, bad_qty, valid)
        fold(v)
    end
    for g in 1:NGRP
        fold(grp_cnt[g]); fold(grp_qty[g]); fold(grp_rev[g])
    end
    for p in 1:NPROD
        fold(prod_cnt[p]); fold(prod_rev[p])
    end
    for v in (w, bias, loss)
        fold(v)
    end
    for b in root
        fold(Int(b))
    end
    for v in (ok200, un401, nf404, total_bytes, resp_cs)
        fold(v)
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
