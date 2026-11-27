#!/usr/bin/env python3
"""13_polars_data_pipeline — 64k orders: filter -> inner join -> group_by aggregates -> rolling-mean window."""

import math

ROWS = 64000
CUSTOMERS = 2000
KNOWN = 1800
REGIONS = 16

rng = 130013


def lcg():
    global rng
    rng = (rng * 1103515245 + 12345) % 2147483648
    return rng


def main():
    cust, qty, amtq, amt = [], [], [], []
    for _ in range(ROWS):
        c = lcg() // 256 % CUSTOMERS
        q = 1 + lcg() // 256 % 9
        p = lcg() // 256 % 400
        cust.append(c)
        qty.append(q)
        amtq.append(q * p)
        amt.append(q * p * 0.25)
    # dimension table keyed by customer id (hash join build side)
    region_of = {c: lcg() // 256 % 16 for c in range(KNOWN)}

    big = joined = roll_sum = roll_cnt = 0
    tot = [0] * REGIONS
    n = [0] * REGIONS
    mx = [0] * REGIONS
    sum_amt = [0.0] * REGIONS
    rings = [[0.0] * 8 for _ in range(REGIONS)]
    seen = [0] * REGIONS
    for i in range(ROWS):
        if not (amt[i] >= 20.0 and qty[i] > 1):
            continue
        big += 1
        r = region_of.get(cust[i], -1)
        if r < 0:
            continue
        joined += 1
        tot[r] += amtq[i]
        n[r] += 1
        sum_amt[r] += amt[i]
        if n[r] == 1 or amtq[i] > mx[r]:
            mx[r] = amtq[i]
        rings[r][seen[r] % 8] = amt[i]
        seen[r] += 1
        if seen[r] >= 8:
            s = 0.0
            for k in range(8):
                s += rings[r][(seen[r] - 8 + k) % 8]
            roll_sum += math.floor(s / 8.0 * 1024.0)
            roll_cnt += 1
    cs = big + joined * 3
    for r in range(REGIONS):
        if n[r] == 0:
            continue
        avg = sum_amt[r] / float(n[r])
        cs += (r + 1) * (tot[r] + 7 * n[r] + math.floor(avg * 1024.0) + 13 * mx[r])
    cs += roll_sum + 5 * roll_cnt
    print(f"CHECKSUM={cs}")


if __name__ == "__main__":
    main()
