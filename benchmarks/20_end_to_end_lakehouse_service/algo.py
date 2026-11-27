#!/usr/bin/env python3
"""20_end_to_end_lakehouse_service — in-process pipeline:
ingest JSONL customers + CSV order batches -> filter/join/group -> fixed-point GD linear fit on the
per-product aggregates -> serve HTTP-like request bytes (bearer-token check via SHA-256, Merkle root of
the group table in every response) -> checksum of response bytes."""
from hashlib import sha256
from zlib import crc32

MOD = 1_000_000_007
NCUST, NPROD, NREG, NTIER = 500, 50, 8, 4
NGRP = NREG * NTIER
BATCHES, ROWS_PB, NREQ, NCLIENT, ITERS = 6, 4000, 3000, 8, 400

rng = 20020


def next_rand() -> int:
    global rng
    rng = rng * 48271 % 2147483647
    return rng


def sdiv(a: int, d: int) -> int:
    return a // d if a >= 0 else -((-a) // d)


def scan_int(b, p):
    """Parse the decimal run at p; returns (value or -1, new p)."""
    v = 0
    have = False
    n = len(b)
    while p < n and 48 <= b[p] <= 57:
        v = v * 10 + b[p] - 48
        p += 1
        have = True
    return (v if have else -1), p


def main() -> None:
    cs = 0

    def fold(v: int) -> None:
        nonlocal cs
        cs = (cs * 131 + v) % MOD

    # ---- ingest customers (JSONL) ----
    cj = bytearray()
    for i in range(NCUST):
        cid = i * 7919 % NCUST
        r = next_rand() % NREG
        t = next_rand() % NTIER
        cj += b'{"id":%d,"region":%d,"tier":%d}\n' % (cid, r, t)
    region_of = [-1] * NCUST
    tier_of = [-1] * NCUST
    ncust = 0
    clen = len(cj)
    cj.append(0)
    p = 0
    while p < clen:
        while p < clen and not 48 <= cj[p] <= 57:
            p += 1
        cid, p = scan_int(cj, p)
        while not 48 <= cj[p] <= 57:
            p += 1
        r, p = scan_int(cj, p)
        while not 48 <= cj[p] <= 57:
            p += 1
        t, p = scan_int(cj, p)
        region_of[cid], tier_of[cid] = r, t
        ncust += 1
        while p < clen and cj[p] != 10:
            p += 1
        p += 1

    # ---- ingest + filter/join/group order batches (CSV) ----
    grp_cnt, grp_qty, grp_rev = [0] * NGRP, [0] * NGRP, [0] * NGRP
    prod_cnt, prod_qty, prod_rev, prod_price = [0] * NPROD, [0] * NPROD, [0] * NPROD, [0] * NPROD
    reg_prod_rev = [0] * (NREG * NPROD)
    oid = rows = null_cust = bad_qty = valid = 0
    for _ in range(BATCHES):
        ob = bytearray(b"order_id,cust_id,product,qty,price_cents\n")
        for _ in range(ROWS_PB):
            nf = next_rand() % 100
            c = next_rand() % NCUST
            pr_ = next_rand() % NPROD
            q = next_rand() % 9
            price = 100 + pr_ * 37 + next_rand() % 200
            ob += (b"%d,%d,%d,%d,%d\n" % (oid, c, pr_, q, price)) if nf != 0 else (b"%d,,%d,%d,%d\n" % (oid, pr_, q, price))
            oid += 1
        olen = len(ob)
        ob.append(0)
        pos = ob.index(10) + 1
        while pos < olen:
            f = [0] * 5
            for k in range(5):
                f[k], pos = scan_int(ob, pos)
                pos += 1
            rows += 1
            if f[1] < 0:
                null_cust += 1
                continue
            if f[3] <= 0:
                bad_qty += 1
                continue
            valid += 1
            rev = f[3] * f[4]
            g = region_of[f[1]] * NTIER + tier_of[f[1]]
            grp_cnt[g] += 1
            grp_qty[g] += f[3]
            grp_rev[g] += rev
            prod_cnt[f[2]] += 1
            prod_qty[f[2]] += f[3]
            prod_rev[f[2]] += rev
            prod_price[f[2]] += f[4]
            reg_prod_rev[region_of[f[1]] * NPROD + f[2]] += rev

    # ---- fit y = w*x + b (w in Q16) by gradient descent on per-product aggregates ----
    xs, ys = [], []
    for p in range(NPROD):
        if prod_cnt[p] > 0:
            xs.append(prod_price[p] // prod_cnt[p] // 16)
            ys.append(prod_rev[p] // prod_cnt[p] // 16)
    n = len(xs)
    w = bias = 0
    for _ in range(ITERS):
        gw = gb = 0
        for i in range(n):
            err = sdiv(w * xs[i], 65536) + bias - ys[i]
            gw += err * xs[i]
            gb += err
        w -= sdiv(gw * 65536, n * 8000)
        bias -= sdiv(gb, n * 4)
    loss = 0
    for i in range(n):
        err = sdiv(w * xs[i], 65536) + bias - ys[i]
        loss += err * err
    pred_cents = []
    for p in range(NPROD):
        x = prod_price[p] // prod_cnt[p] // 16 if prod_cnt[p] > 0 else 0
        y = sdiv(w * x, 65536) + bias
        pred_cents.append((y if y > 0 else 0) * 16)

    # ---- merkle root over the region x tier group table ----
    level = []
    for g in range(NGRP):
        leaf = b"".join(v.to_bytes(8, "big") for v in (g, grp_cnt[g], grp_qty[g], grp_rev[g]))
        level.append(sha256(leaf).digest())
    while len(level) > 1:
        level = [sha256(level[2 * i] + level[2 * i + 1]).digest() for i in range(len(level) // 2)]
    root = level[0]
    root_hex = root.hex().encode()

    # ---- client tokens ----
    tokens = [sha256(b"key-%d" % k).digest()[:8].hex().encode() for k in range(NCLIENT)]

    # ---- serve ----
    ok200 = un401 = nf404 = total_bytes = resp_cs = 0
    for _ in range(NREQ):
        k = next_rand() % NCLIENT
        pq = next_rand() % 60
        rg = next_rand() % NREG
        bad = next_rand() % 100
        at = next_rand() % 16
        tok = bytearray(tokens[k])
        if bad < 8:
            tok[at] = 122
        req = b"GET /v1/product/%d?region=%d HTTP/1.1\r\nHost: lake\r\nX-Client: %d\r\nAuthorization: Bearer " % (pq, rg, k) + bytes(tok) + b"\r\n\r\n"
        # server side: parse request bytes
        status = 400
        pp = rr = -1
        if req.startswith(b"GET /v1/product/"):
            q = 16
            pp, q = scan_int(req, q)
            q += 8
            rr, q = scan_int(req, q)
            xc = req.find(b"X-Client: ", q)
            bt = req.find(b"Bearer ", q)
            if xc >= 0 and bt >= 0:
                kk, _q = scan_int(req, xc + 10)
                if 0 <= kk < NCLIENT and bt + 7 + 16 <= len(req) and req[bt + 7:bt + 23] == tokens[kk]:
                    status = 200 if (0 <= pp < NPROD and 0 <= rr < NREG and prod_cnt[pp] > 0) else 404
                else:
                    status = 401
        if status == 200:
            body = (b'{"product":%d,"region":%d,"orders":%d,"revenue":%d,"region_revenue":%d,"forecast":%d,"root":"%s"}'
                    % (pp, rr, prod_cnt[pp], prod_rev[pp], reg_prod_rev[rr * NPROD + pp], pred_cents[pp], root_hex))
            ok200 += 1
            head = b"HTTP/1.1 200 OK"
        elif status == 404:
            body = b'{"error":"not found"}'
            nf404 += 1
            head = b"HTTP/1.1 404 Not Found"
        elif status == 401:
            body = b'{"error":"unauthorized"}'
            un401 += 1
            head = b"HTTP/1.1 401 Unauthorized"
        else:
            body = b'{"error":"bad request"}'
            head = b"HTTP/1.1 400 Bad Request"
        resp = head + b"\r\nContent-Length: %d\r\n\r\n" % len(body) + body
        total_bytes += len(resp)
        resp_cs = (resp_cs * 131 + crc32(resp)) % MOD

    # ---- checksum ----
    for v in (ncust, rows, null_cust, bad_qty, valid):
        fold(v)
    for g in range(NGRP):
        fold(grp_cnt[g]); fold(grp_qty[g]); fold(grp_rev[g])
    for p in range(NPROD):
        fold(prod_cnt[p]); fold(prod_rev[p])
    for v in (w, bias, loss):
        fold(v)
    for b in root:
        fold(b)
    for v in (ok200, un401, nf404, total_bytes, resp_cs):
        fold(v)
    print(f"CHECKSUM={cs}")


if __name__ == "__main__":
    main()
