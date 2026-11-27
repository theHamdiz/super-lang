# 13_polars_data_pipeline — 64k orders: filter -> inner join -> group_by aggregates -> rolling-mean window (Julia)
# Run: julia -O3 --check-bounds=no algo.jl

const ROWS = 64000
const CUSTOMERS = 2000
const KNOWN = 1800
const REGIONS = 16

mutable struct Rng; s::Int; end
@inline function lcg!(r::Rng)
    r.s = (r.s * 1103515245 + 12345) % 2147483648
    return r.s
end

function compute()::Int
    r = Rng(130013)
    cust = zeros(Int, ROWS); qty = zeros(Int, ROWS); amtq = zeros(Int, ROWS); amt = zeros(ROWS)
    for i in 1:ROWS
        c = lcg!(r) ÷ 256 % CUSTOMERS
        q = 1 + lcg!(r) ÷ 256 % 9
        p = lcg!(r) ÷ 256 % 400
        cust[i] = c; qty[i] = q; amtq[i] = q * p; amt[i] = (q * p) * 0.25
    end
    # dimension table: direct-address hash on customer id (0-based id -> index + 1)
    region_of = fill(-1, CUSTOMERS)
    for c in 0:KNOWN-1
        region_of[c+1] = lcg!(r) ÷ 256 % 16
    end

    big = 0; joined = 0; roll_sum = 0; roll_cnt = 0
    tot = zeros(Int, REGIONS); n = zeros(Int, REGIONS); mx = zeros(Int, REGIONS)
    sum_amt = zeros(REGIONS)
    ring = zeros(8, REGIONS)
    seen = zeros(Int, REGIONS)
    for i in 1:ROWS
        (amt[i] >= 20.0 && qty[i] > 1) || continue
        big += 1
        rg = region_of[cust[i]+1]
        rg < 0 && continue
        joined += 1
        k = rg + 1
        tot[k] += amtq[i]
        n[k] += 1
        sum_amt[k] += amt[i]
        if n[k] == 1 || amtq[i] > mx[k]
            mx[k] = amtq[i]
        end
        ring[seen[k] % 8 + 1, k] = amt[i]
        seen[k] += 1
        if seen[k] >= 8
            s = 0.0
            for j in 0:7
                s += ring[(seen[k] - 8 + j) % 8 + 1, k]
            end
            roll_sum += floor(Int, s / 8.0 * 1024.0)
            roll_cnt += 1
        end
    end
    cs = big + joined * 3
    for k in 1:REGIONS
        n[k] == 0 && continue
        avg = sum_amt[k] / n[k]
        cs += k * (tot[k] + 7 * n[k] + floor(Int, avg * 1024.0) + 13 * mx[k])
    end
    cs += roll_sum + 5 * roll_cnt
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
    warm = parse(Int, get(ENV, "JULIA_BENCH_WARM", "2"))
    for _ in 1:warm
        compute()
    end
    main()
end
