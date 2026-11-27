# 14_http_router_throughput - 30k simulated HTTP requests: parse bytes, radix route match,
# HMAC-SHA256 bearer-token check, JSON body validation, status-code checksum
import hmac
import hashlib

NREQ = 30000
NOW = 1700000000
KEY = bytes((i * 37 + 11) % 256 for i in range(32))
HEX = b"0123456789abcdef"

rng = 140014


def draw(m):
    global rng
    rng = (rng * 1103515245 + 12345) % 2147483648
    return (rng // 256) % m


def mac_of(msg):
    return hmac.new(KEY, bytes(msg), hashlib.sha256).digest()


def put_id(out):
    bad = draw(50)
    idv = 1 + draw(99999)
    out += b"abc" if bad == 0 else str(idv).encode()


def build_request():
    path = bytearray()
    body = bytearray()
    tok = bytearray()
    method = 0
    has_body = False
    a = draw(100)
    if a < 5:
        path += b"/health"
    elif a < 15:
        path += b"/users"
    elif a < 35:
        path += b"/users/"
        put_id(path)
    elif a < 45:
        path += b"/users/"
        put_id(path)
        path += b"/orders"
    elif a < 55:
        method, has_body = 1, True
        path += b"/orders"
    elif a < 65:
        path += b"/orders/"
        put_id(path)
    elif a < 70:
        method, has_body = 2, True
        path += b"/orders/"
        put_id(path)
    elif a < 75:
        method = 3
        path += b"/orders/"
        put_id(path)
    elif a < 85:
        path += b"/static/a" + str(draw(10)).encode()
        path += b"/f" + str(draw(100)).encode() + b".js"
    elif a < 90:
        method, has_body = 1, True
        path += b"/auth/login"
    elif a < 95:
        path += b"/v1/items/"
        put_id(path)
        path += b"/tags/t" + str(draw(50)).encode()
    elif a < 98:
        path += b"/nope/"
        put_id(path)
    else:
        method = 3
        path += b"/users"

    t = draw(20)
    uid = 1 + draw(9999)
    ex = draw(1000)
    has_tok = t >= 2
    if has_tok:
        exp = NOW - 1 - ex if t == 4 else NOW + 1000 + ex
        sb = str(uid).encode() + b"." + str(exp).encode()
        mac = bytearray(mac_of(sb))
        if t < 4:
            mac[0] ^= 1
        tok += sb + b"."
        for i in range(4):
            tok.append(HEX[mac[i] >> 4])
            tok.append(HEX[mac[i] & 15])

    if has_body:
        v = draw(10)
        ws = draw(2)
        sp = b" " if ws else b""
        if 45 <= a < 55:
            user = 1 + draw(9999)
            qty = 1 + draw(100)
            if v == 0:
                qty = 0
            if v == 3:
                user = 10000
            body += b'{"user":' + sp + str(user).encode() + b"," + sp
            body += b'"qty":' + sp + str(qty).encode()
            sku = bytes(97 + draw(26) for _ in range(4))
            if v != 1:
                body += b"," + sp + b'"sku":' + sp + b'"' + sku + b'"'
            body += b"}"
            if v == 2:
                body.pop()
        elif 65 <= a < 70:
            qty = 1 + draw(100)
            nl = 3 + draw(5)
            if v == 0:
                qty = 101
            body += b"{"
            if v != 1:
                body += b'"qty":' + sp + str(qty).encode() + b"," + sp
            body += b'"note":' + sp + b'"'
            for _ in range(nl):
                body.append(97 + draw(26))
            body += b'"}'
            if v == 2:
                body.pop()
        else:
            ul = 4 + draw(6)
            un = bytes(97 + draw(26) for _ in range(ul))
            pwl = 6 + draw(7)
            pw = bytes(97 + draw(26) for _ in range(pwl))
            if v == 0:
                pw = b""
            body += b"{"
            if v != 1:
                body += b'"user":' + sp + b'"' + un + b'",' + sp
            body += b'"pass":' + sp + b'"' + pw + b'"}'
            if v == 2:
                body += b"x"

    r = bytearray((b"GET ", b"POST ", b"PUT ", b"DELETE ")[method])
    r += path + b" HTTP/1.1\r\nHost: api.local\r\n"
    if has_tok:
        r += b"Authorization: Bearer " + tok + b"\r\n"
    if has_body:
        r += b"Content-Length: " + str(len(body)).encode() + b"\r\n"
    r += b"\r\n" + body
    return bytes(r)


# ---- radix router: node = [kind, bytes, first_child, next_sibling, handlers[4]] ----
nodes = [[0, b"", -1, -1, [-1, -1, -1, -1]]]


def add_route(tpl, method, rid):
    cur = 0
    parts = tpl[1:].split("/")
    for seg in parts:
        kind = 1 if seg[0] == ":" else 2 if seg[0] == "*" else 0
        lit = seg.encode() if kind == 0 else b""
        found = -1
        c = nodes[cur][2]
        while c >= 0 and found < 0:
            nd = nodes[c]
            if nd[0] == kind and (kind != 0 or nd[1] == lit):
                found = c
            c = nd[3]
        if found < 0:
            nodes.append([kind, lit, -1, nodes[cur][2], [-1, -1, -1, -1]])
            found = len(nodes) - 1
            nodes[cur][2] = found
        cur = found
    nodes[cur][4][method] = rid


AUTH = (False, True, True, True, True, True, True, True, False, False, True)


def parse_digits(b, p, e):
    if e <= p or e - p > 10:
        return -1
    v = 0
    for i in range(p, e):
        c = b[i]
        if c < 48 or c > 57:
            return -1
        v = v * 10 + (c - 48)
    return v


def verify_token(b, off, ln):
    e = off + ln
    d1 = d2 = -1
    for i in range(off, e):
        if b[i] == 46:
            if d1 < 0:
                d1 = i
            elif d2 < 0:
                d2 = i
            else:
                return -1
    if d1 < 0 or d2 < 0 or e - d2 - 1 != 8:
        return -1
    u = parse_digits(b, off, d1)
    ex = parse_digits(b, d1 + 1, d2)
    if u < 0 or ex < 0:
        return -1
    mac = mac_of(b[off:d2])
    for i in range(4):
        if b[d2 + 1 + 2 * i] != HEX[mac[i] >> 4] or b[d2 + 2 + 2 * i] != HEX[mac[i] & 15]:
            return -1
    if ex <= NOW:
        return -1
    return u


WS = (32, 9, 10, 13)
KEYS = {b"user": 1, b"qty": 2, b"sku": 3, b"note": 4, b"pass": 5}


def skip_ws(b, p, e):
    while p < e and b[p] in WS:
        p += 1
    return p


def parse_json(b, off, ln):
    """Returns field table f (id -> (type, val)) or None when malformed."""
    e = off + ln
    f = {}
    p = skip_ws(b, off, e)
    if p >= e or b[p] != 123:
        return None
    p = skip_ws(b, p + 1, e)
    if p < e and b[p] == 125:
        p = skip_ws(b, p + 1, e)
        return f if p == e else None
    while True:
        if p >= e or b[p] != 34:
            return None
        p += 1
        ks = p
        while p < e and b[p] != 34:
            if b[p] == 92:
                return None
            p += 1
        if p >= e:
            return None
        kid = KEYS.get(bytes(b[ks:p]), 0)
        p = skip_ws(b, p + 1, e)
        if p >= e or b[p] != 58:
            return None
        p = skip_ws(b, p + 1, e)
        if p >= e:
            return None
        if b[p] == 34:
            p += 1
            vs = p
            while p < e and b[p] != 34:
                if b[p] == 92:
                    return None
                p += 1
            if p >= e:
                return None
            ty, val = 2, p - vs
            p += 1
        else:
            neg = False
            if b[p] == 45:
                neg = True
                p += 1
            ds = p
            while p < e and 48 <= b[p] <= 57:
                p += 1
            v = parse_digits(b, ds, p)
            if v < 0:
                return None
            ty, val = 1, -v if neg else v
        if kid > 0:
            f[kid] = (ty, val)
        p = skip_ws(b, p, e)
        if p >= e:
            return None
        if b[p] == 44:
            p = skip_ws(b, p + 1, e)
            continue
        if b[p] == 125:
            p += 1
            break
        return None
    p = skip_ws(b, p, e)
    return f if p == e else None


def has_int(f, i, lo, hi):
    x = f.get(i)
    return x is not None and x[0] == 1 and lo <= x[1] <= hi


def has_str(f, i, lo, hi):
    x = f.get(i)
    return x is not None and x[0] == 2 and lo <= x[1] <= hi


def body_valid(rid, b, off, ln):
    f = parse_json(b, off, ln)
    if f is None:
        return False
    if rid == 4:
        return has_int(f, 1, 1, 9999) and has_int(f, 2, 1, 100) and has_str(f, 3, 4, 4)
    if rid == 6:
        return has_int(f, 2, 1, 100) and (4 not in f or has_str(f, 4, 0, 1000))
    return has_str(f, 1, 1, 64) and has_str(f, 5, 1, 64)


def handle(b):
    """Returns (status, path id)."""
    n = len(b)
    if b.startswith(b"GET "):
        method, p = 0, 4
    elif b.startswith(b"POST "):
        method, p = 1, 5
    elif b.startswith(b"PUT "):
        method, p = 2, 4
    elif b.startswith(b"DELETE "):
        method, p = 3, 7
    else:
        return 400, 0
    po = p
    while p < n and b[p] != 32:
        p += 1
    pe = p
    while p < n and b[p] != 13:
        p += 1
    p += 2

    tok_off, tok_len, clen = -1, 0, -1
    while p < n:
        if b[p] == 13:
            p += 2
            break
        le = p
        while le < n and b[le] != 13:
            le += 1
        if b.startswith(b"Authorization: Bearer ", p):
            tok_off, tok_len = p + 22, le - (p + 22)
        elif b.startswith(b"Content-Length: ", p):
            clen = parse_digits(b, p + 16, le)
        p = le + 2
    body_off, body_len = p, n - p
    if clen >= 0 and clen != body_len:
        return 400, 0

    plen = pe - po
    cur, found, nparam, bad_id, pid = 0, True, 0, False, -1
    if plen < 1 or b[po] != 47:
        found = False
    i = 1
    while found and i <= plen:
        e = i
        while e < plen and b[po + e] != 47:
            e += 1
        sl = e - i
        if sl == 0:
            found = False
            break
        seg = b[po + i:po + e]
        st = pm = wc = -1
        c = nodes[cur][2]
        while c >= 0:
            nd = nodes[c]
            k = nd[0]
            if k == 0:
                if nd[1] == seg:
                    st = c
                    break
            elif k == 1:
                pm = c
            else:
                wc = c
            c = nd[3]
        if st >= 0:
            cur = st
        elif pm >= 0:
            cur = pm
            if nparam == 0:
                pid = parse_digits(b, po + i, po + e)
                if pid < 0:
                    bad_id = True
            nparam += 1
        elif wc >= 0:
            cur = wc
            break
        else:
            found = False
            break
        i = e + 1
    if not found:
        return 404, 0
    hs = nodes[cur][4]
    rid = hs[method]
    if rid < 0:
        return (405 if max(hs) >= 0 else 404), 0
    pout = 0 if bad_id or pid < 0 else pid

    uid = 0
    if AUTH[rid]:
        if tok_off < 0:
            return 401, pout
        uid = verify_token(b, tok_off, tok_len)
        if uid < 0:
            return 401, pout
        if rid in (6, 7) and uid % 13 == 0:
            return 403, pout
    if bad_id:
        return 400, pout
    if rid in (4, 6, 9) and not body_valid(rid, b, body_off, body_len):
        return 400, pout
    if rid == 2:
        return (404 if pid > 90000 else 200), pout
    if rid == 4:
        return 201, pout
    if rid == 5:
        return (404 if pid % 7 == 0 else 200), pout
    if rid == 7:
        return 204, pout
    return 200, pout


def main():
    add_route("/health", 0, 0)
    add_route("/users", 0, 1)
    add_route("/users/:id", 0, 2)
    add_route("/users/:id/orders", 0, 3)
    add_route("/orders", 1, 4)
    add_route("/orders/:id", 0, 5)
    add_route("/orders/:id", 2, 6)
    add_route("/orders/:id", 3, 7)
    add_route("/static/*path", 0, 8)
    add_route("/auth/login", 1, 9)
    add_route("/v1/items/:id/tags/:tag", 0, 10)

    cs = sum_pid = ok2xx = 0
    by_status = [0] * 600
    for i in range(NREQ):
        req = build_request()
        st, pid = handle(req)
        by_status[st] += 1
        cs += st * (i % 97 + 1)
        sum_pid += pid
        if st < 300:
            ok2xx += 1
    mix = 0
    for s in range(600):
        mix += by_status[s] * (s % 53 + 1) * (s // 100 + 1)
    cs += sum_pid + 5 * ok2xx + 7 * mix
    print(f"CHECKSUM={cs}")


main()
