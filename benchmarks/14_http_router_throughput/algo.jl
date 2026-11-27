# 14_http_router_throughput - 30k simulated HTTP requests: parse bytes, radix route match,
# HMAC-SHA256 bearer-token check, JSON body validation, status-code checksum (Julia)
# Run: julia -O3 --check-bounds=no algo.jl
using SHA

const NREQ = 30000
const NOW = 1700000000
const HEXC = Vector{UInt8}(codeunits("0123456789abcdef"))
const KEY = UInt8[(i * 37 + 11) % 256 for i in 0:31]
const AUTH = (false, true, true, true, true, true, true, true, false, false, true)

mutable struct Rng
    s::Int
end

@inline function draw!(r::Rng, m::Int)::Int
    r.s = (r.s * 1103515245 + 12345) % 2147483648
    return (r.s ÷ 256) % m
end

const Buf = Vector{UInt8}
lit!(b::Buf, s::AbstractString) = append!(b, codeunits(s))
dec!(b::Buf, v::Integer) = append!(b, codeunits(string(v)))
add!(b::Buf, o::Buf) = append!(b, o)

function chars!(b::Buf, r::Rng, cnt::Int)
    for _ in 1:cnt
        push!(b, UInt8(97 + draw!(r, 26)))
    end
end

function id!(b::Buf, r::Rng)
    bad = draw!(r, 50)
    idv = 1 + draw!(r, 99999)
    bad == 0 ? lit!(b, "abc") : dec!(b, idv)
end

const METHODS = ("GET ", "POST ", "PUT ", "DELETE ")

function build_request!(r::Rng, req::Buf)
    empty!(req)
    path, body, tok = Buf(), Buf(), Buf()
    method = 0
    has_body = false
    a = draw!(r, 100)
    if a < 5
        lit!(path, "/health")
    elseif a < 15
        lit!(path, "/users")
    elseif a < 35
        lit!(path, "/users/"); id!(path, r)
    elseif a < 45
        lit!(path, "/users/"); id!(path, r); lit!(path, "/orders")
    elseif a < 55
        method = 1; has_body = true; lit!(path, "/orders")
    elseif a < 65
        lit!(path, "/orders/"); id!(path, r)
    elseif a < 70
        method = 2; has_body = true; lit!(path, "/orders/"); id!(path, r)
    elseif a < 75
        method = 3; lit!(path, "/orders/"); id!(path, r)
    elseif a < 85
        lit!(path, "/static/a"); dec!(path, draw!(r, 10))
        lit!(path, "/f"); dec!(path, draw!(r, 100)); lit!(path, ".js")
    elseif a < 90
        method = 1; has_body = true; lit!(path, "/auth/login")
    elseif a < 95
        lit!(path, "/v1/items/"); id!(path, r)
        lit!(path, "/tags/t"); dec!(path, draw!(r, 50))
    elseif a < 98
        lit!(path, "/nope/"); id!(path, r)
    else
        method = 3; lit!(path, "/users")
    end

    t = draw!(r, 20)
    uid = 1 + draw!(r, 9999)
    ex = draw!(r, 1000)
    has_tok = t >= 2
    if has_tok
        exp = t == 4 ? NOW - 1 - ex : NOW + 1000 + ex
        sb = Buf()
        dec!(sb, uid); push!(sb, UInt8('.')); dec!(sb, exp)
        mac = hmac_sha256(KEY, sb)
        if t < 4
            mac[1] ⊻= 0x01
        end
        add!(tok, sb); push!(tok, UInt8('.'))
        for i in 1:4
            push!(tok, HEXC[(mac[i] >> 4) + 1])
            push!(tok, HEXC[(mac[i] & 0x0f) + 1])
        end
    end

    if has_body
        v = draw!(r, 10)
        ws = draw!(r, 2)
        sp = ws == 1 ? " " : ""
        if a >= 45 && a < 55
            user = 1 + draw!(r, 9999)
            qty = 1 + draw!(r, 100)
            v == 0 && (qty = 0)
            v == 3 && (user = 10000)
            lit!(body, "{\"user\":"); lit!(body, sp); dec!(body, user)
            lit!(body, ","); lit!(body, sp)
            lit!(body, "\"qty\":"); lit!(body, sp); dec!(body, qty)
            sku = Buf(); chars!(sku, r, 4)
            if v != 1
                lit!(body, ","); lit!(body, sp)
                lit!(body, "\"sku\":"); lit!(body, sp); push!(body, UInt8('"')); add!(body, sku); push!(body, UInt8('"'))
            end
            push!(body, UInt8('}'))
            v == 2 && pop!(body)
        elseif a >= 65 && a < 70
            qty = 1 + draw!(r, 100)
            nl = 3 + draw!(r, 5)
            v == 0 && (qty = 101)
            lit!(body, "{")
            if v != 1
                lit!(body, "\"qty\":"); lit!(body, sp); dec!(body, qty)
                lit!(body, ","); lit!(body, sp)
            end
            lit!(body, "\"note\":"); lit!(body, sp); push!(body, UInt8('"'))
            chars!(body, r, nl)
            lit!(body, "\"}")
            v == 2 && pop!(body)
        else
            ul = 4 + draw!(r, 6)
            un = Buf(); chars!(un, r, ul)
            pwl = 6 + draw!(r, 7)
            pw = Buf(); chars!(pw, r, pwl)
            v == 0 && empty!(pw)
            lit!(body, "{")
            if v != 1
                lit!(body, "\"user\":"); lit!(body, sp); push!(body, UInt8('"')); add!(body, un)
                lit!(body, "\","); lit!(body, sp)
            end
            lit!(body, "\"pass\":"); lit!(body, sp); push!(body, UInt8('"')); add!(body, pw); lit!(body, "\"}")
            v == 2 && push!(body, UInt8('x'))
        end
    end

    lit!(req, METHODS[method + 1]); add!(req, path)
    lit!(req, " HTTP/1.1\r\nHost: api.local\r\n")
    if has_tok
        lit!(req, "Authorization: Bearer "); add!(req, tok); lit!(req, "\r\n")
    end
    if has_body
        lit!(req, "Content-Length: "); dec!(req, length(body)); lit!(req, "\r\n")
    end
    lit!(req, "\r\n"); add!(req, body)
    return nothing
end

# ---- radix router ----
mutable struct Node
    kind::Int
    lit::Vector{UInt8}
    first::Int
    next::Int
    h::Vector{Int}
end

function add_route!(nodes::Vector{Node}, tpl::String, method::Int, rid::Int)
    cur = 0
    for seg in split(tpl[2:end], '/')
        kind = seg[1] == ':' ? 1 : seg[1] == '*' ? 2 : 0
        lit = kind == 0 ? Vector{UInt8}(codeunits(seg)) : UInt8[]
        found = -1
        c = nodes[cur + 1].first
        while c >= 0 && found < 0
            nd = nodes[c + 1]
            if nd.kind == kind && (kind != 0 || nd.lit == lit)
                found = c
            end
            c = nd.next
        end
        if found < 0
            push!(nodes, Node(kind, lit, -1, nodes[cur + 1].first, [-1, -1, -1, -1]))
            found = length(nodes) - 1
            nodes[cur + 1].first = found
        end
        cur = found
    end
    nodes[cur + 1].h[method + 1] = rid
end

# 0-based byte access on 1-based vectors
@inline at(b::Buf, i::Int) = @inbounds b[i + 1]

function parse_digits(b::Buf, p::Int, e::Int)::Int
    (e <= p || e - p > 10) && return -1
    v = 0
    for i in p:(e - 1)
        c = at(b, i)
        (c < 0x30 || c > 0x39) && return -1
        v = v * 10 + (c - 0x30)
    end
    return v
end

function starts_at(b::Buf, n::Int, p::Int, s::String)::Bool
    cu = codeunits(s)
    p + length(cu) > n && return false
    for i in 1:length(cu)
        at(b, p + i - 1) != cu[i] && return false
    end
    return true
end

function verify_token(b::Buf, off::Int, len::Int)::Int
    e = off + len
    d1 = -1; d2 = -1
    for i in off:(e - 1)
        if at(b, i) == 0x2e
            if d1 < 0
                d1 = i
            elseif d2 < 0
                d2 = i
            else
                return -1
            end
        end
    end
    (d1 < 0 || d2 < 0 || e - d2 - 1 != 8) && return -1
    u = parse_digits(b, off, d1)
    ex = parse_digits(b, d1 + 1, d2)
    (u < 0 || ex < 0) && return -1
    mac = hmac_sha256(KEY, b[(off + 1):d2])
    for i in 0:3
        if at(b, d2 + 1 + 2i) != HEXC[(mac[i + 1] >> 4) + 1] || at(b, d2 + 2 + 2i) != HEXC[(mac[i + 1] & 0x0f) + 1]
            return -1
        end
    end
    ex <= NOW && return -1
    return u
end

@inline isws(c::UInt8) = c == 0x20 || c == 0x09 || c == 0x0a || c == 0x0d
function skip_ws(b::Buf, p::Int, e::Int)::Int
    while p < e && isws(at(b, p))
        p += 1
    end
    return p
end

function key_id(b::Buf, p::Int, e::Int)::Int
    k = String(b[(p + 1):e])
    return k == "user" ? 1 : k == "qty" ? 2 : k == "sku" ? 3 : k == "note" ? 4 : k == "pass" ? 5 : 0
end

# f[id*3+1] present, f[id*3+2] type (1 int, 2 str), f[id*3+3] value / string length
function parse_json!(b::Buf, off::Int, len::Int, f::Vector{Int})::Bool
    e = off + len
    fill!(f, 0)
    p = skip_ws(b, off, e)
    (p >= e || at(b, p) != 0x7b) && return false
    p = skip_ws(b, p + 1, e)
    if p < e && at(b, p) == 0x7d
        p = skip_ws(b, p + 1, e)
        return p == e
    end
    while true
        (p >= e || at(b, p) != 0x22) && return false
        p += 1
        ks = p
        while p < e && at(b, p) != 0x22
            at(b, p) == 0x5c && return false
            p += 1
        end
        p >= e && return false
        id = key_id(b, ks, p)
        p = skip_ws(b, p + 1, e)
        (p >= e || at(b, p) != 0x3a) && return false
        p = skip_ws(b, p + 1, e)
        p >= e && return false
        if at(b, p) == 0x22
            p += 1
            vs = p
            while p < e && at(b, p) != 0x22
                at(b, p) == 0x5c && return false
                p += 1
            end
            p >= e && return false
            ty = 2; val = p - vs
            p += 1
        else
            neg = false
            if at(b, p) == 0x2d
                neg = true
                p += 1
            end
            ds = p
            while p < e && at(b, p) >= 0x30 && at(b, p) <= 0x39
                p += 1
            end
            v = parse_digits(b, ds, p)
            v < 0 && return false
            ty = 1; val = neg ? -v : v
        end
        if id > 0
            f[id * 3 + 1] = 1; f[id * 3 + 2] = ty; f[id * 3 + 3] = val
        end
        p = skip_ws(b, p, e)
        p >= e && return false
        if at(b, p) == 0x2c
            p = skip_ws(b, p + 1, e)
            continue
        end
        if at(b, p) == 0x7d
            p += 1
            break
        end
        return false
    end
    p = skip_ws(b, p, e)
    return p == e
end

has_int(f, id, lo, hi) = f[id * 3 + 1] != 0 && f[id * 3 + 2] == 1 && lo <= f[id * 3 + 3] <= hi
has_str(f, id, lo, hi) = f[id * 3 + 1] != 0 && f[id * 3 + 2] == 2 && lo <= f[id * 3 + 3] <= hi

function body_valid(rid::Int, b::Buf, off::Int, len::Int, f::Vector{Int})::Bool
    parse_json!(b, off, len, f) || return false
    rid == 4 && return has_int(f, 1, 1, 9999) && has_int(f, 2, 1, 100) && has_str(f, 3, 4, 4)
    rid == 6 && return has_int(f, 2, 1, 100) && (f[4 * 3 + 1] == 0 || has_str(f, 4, 0, 1000))
    return has_str(f, 1, 1, 64) && has_str(f, 5, 1, 64)
end

# returns (status, path id)
function handle(b::Buf, nodes::Vector{Node}, f::Vector{Int})::Tuple{Int,Int}
    n = length(b)
    if starts_at(b, n, 0, "GET ")
        method = 0; p = 4
    elseif starts_at(b, n, 0, "POST ")
        method = 1; p = 5
    elseif starts_at(b, n, 0, "PUT ")
        method = 2; p = 4
    elseif starts_at(b, n, 0, "DELETE ")
        method = 3; p = 7
    else
        return (400, 0)
    end
    po = p
    while p < n && at(b, p) != 0x20
        p += 1
    end
    pe = p
    while p < n && at(b, p) != 0x0d
        p += 1
    end
    p += 2

    tok_off = -1; tok_len = 0; clen = -1
    while p < n
        if at(b, p) == 0x0d
            p += 2
            break
        end
        le = p
        while le < n && at(b, le) != 0x0d
            le += 1
        end
        if starts_at(b, n, p, "Authorization: Bearer ")
            tok_off = p + 22; tok_len = le - (p + 22)
        elseif starts_at(b, n, p, "Content-Length: ")
            clen = parse_digits(b, p + 16, le)
        end
        p = le + 2
    end
    body_off = p; body_len = n - p
    (clen >= 0 && clen != body_len) && return (400, 0)

    plen = pe - po
    cur = 0; found = true; nparam = 0; bad_id = false; pid = -1
    (plen < 1 || at(b, po) != 0x2f) && (found = false)
    i = 1
    while found && i <= plen
        e = i
        while e < plen && at(b, po + e) != 0x2f
            e += 1
        end
        sl = e - i
        if sl == 0
            found = false
            break
        end
        st = -1; pm = -1; wc = -1
        c = nodes[cur + 1].first
        while c >= 0
            nd = nodes[c + 1]
            if nd.kind == 0
                if length(nd.lit) == sl
                    same = true
                    for q in 0:(sl - 1)
                        if nd.lit[q + 1] != at(b, po + i + q)
                            same = false
                            break
                        end
                    end
                    if same
                        st = c
                        break
                    end
                end
            elseif nd.kind == 1
                pm = c
            else
                wc = c
            end
            c = nd.next
        end
        if st >= 0
            cur = st
        elseif pm >= 0
            cur = pm
            if nparam == 0
                pid = parse_digits(b, po + i, po + e)
                pid < 0 && (bad_id = true)
            end
            nparam += 1
        elseif wc >= 0
            cur = wc
            break
        else
            found = false
            break
        end
        i = e + 1
    end
    found || return (404, 0)
    hs = nodes[cur + 1].h
    rid = hs[method + 1]
    if rid < 0
        return (maximum(hs) >= 0 ? 405 : 404, 0)
    end
    pout = (bad_id || pid < 0) ? 0 : pid

    if AUTH[rid + 1]
        tok_off < 0 && return (401, pout)
        uid = verify_token(b, tok_off, tok_len)
        uid < 0 && return (401, pout)
        ((rid == 6 || rid == 7) && uid % 13 == 0) && return (403, pout)
    end
    bad_id && return (400, pout)
    ((rid == 4 || rid == 6 || rid == 9) && !body_valid(rid, b, body_off, body_len, f)) && return (400, pout)
    (rid == 2 && pid > 90000) && return (404, pout)
    rid == 4 && return (201, pout)
    (rid == 5 && pid % 7 == 0) && return (404, pout)
    rid == 7 && return (204, pout)
    return (200, pout)
end

function compute()::Int
    nodes = Node[Node(0, UInt8[], -1, -1, [-1, -1, -1, -1])]
    add_route!(nodes, "/health", 0, 0)
    add_route!(nodes, "/users", 0, 1)
    add_route!(nodes, "/users/:id", 0, 2)
    add_route!(nodes, "/users/:id/orders", 0, 3)
    add_route!(nodes, "/orders", 1, 4)
    add_route!(nodes, "/orders/:id", 0, 5)
    add_route!(nodes, "/orders/:id", 2, 6)
    add_route!(nodes, "/orders/:id", 3, 7)
    add_route!(nodes, "/static/*path", 0, 8)
    add_route!(nodes, "/auth/login", 1, 9)
    add_route!(nodes, "/v1/items/:id/tags/:tag", 0, 10)

    r = Rng(140014)
    cs = 0; sum_pid = 0; ok2xx = 0
    by_status = zeros(Int, 600)
    req = Buf()
    f = zeros(Int, 18)
    for i in 0:(NREQ - 1)
        build_request!(r, req)
        st, pid = handle(req, nodes, f)
        by_status[st + 1] += 1
        cs += st * (i % 97 + 1)
        sum_pid += pid
        st < 300 && (ok2xx += 1)
    end
    mix = 0
    for s in 0:599
        mix += by_status[s + 1] * (s % 53 + 1) * (s ÷ 100 + 1)
    end
    return cs + sum_pid + 5 * ok2xx + 7 * mix
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
