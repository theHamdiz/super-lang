-- 14_http_router_throughput - 30k simulated HTTP requests: parse bytes, radix route match,
-- HMAC-SHA256 bearer-token check, JSON body validation, status-code checksum (LuaJIT)
local bit = require("bit")
local band, bor, bxor, bnot = bit.band, bit.bor, bit.bxor, bit.bnot
local lshift, rshift, ror, tobit = bit.lshift, bit.rshift, bit.ror, bit.tobit
local floor, sbyte, schar = math.floor, string.byte, string.char

local NREQ = 30000
local NOW = 1700000000

local K = {
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
}
for i = 1, 64 do K[i] = tobit(K[i]) end

local W = {}
-- SHA-256 of msg[0..len-1] (0-based byte table) -> 32-byte 0-based table
local function sha256(msg, len)
  local h0, h1, h2, h3 = tobit(0x6a09e667), tobit(0xbb67ae85), tobit(0x3c6ef372), tobit(0xa54ff53a)
  local h4, h5, h6, h7 = tobit(0x510e527f), tobit(0x9b05688c), tobit(0x1f83d9ab), tobit(0x5be0cd19)
  local total = floor((len + 9 + 63) / 64) * 64
  local buf = {}
  for i = 0, total - 1 do buf[i] = 0 end
  for i = 0, len - 1 do buf[i] = msg[i] end
  buf[len] = 0x80
  local bits = len * 8
  for i = 0, 7 do buf[total - 1 - i] = floor(bits / 2 ^ (8 * i)) % 256 end
  for off = 0, total - 1, 64 do
    for i = 0, 15 do
      local o = off + 4 * i
      W[i] = bor(lshift(buf[o], 24), lshift(buf[o + 1], 16), lshift(buf[o + 2], 8), buf[o + 3])
    end
    for i = 16, 63 do
      local a, b = W[i - 15], W[i - 2]
      local s0 = bxor(ror(a, 7), ror(a, 18), rshift(a, 3))
      local s1 = bxor(ror(b, 17), ror(b, 19), rshift(b, 10))
      W[i] = tobit(W[i - 16] + s0 + W[i - 7] + s1)
    end
    local a, b, c, d, e, f, g, h = h0, h1, h2, h3, h4, h5, h6, h7
    for i = 0, 63 do
      local t1 = tobit(h + bxor(ror(e, 6), ror(e, 11), ror(e, 25)) + bxor(band(e, f), band(bnot(e), g)) + K[i + 1] + W[i])
      local t2 = tobit(bxor(ror(a, 2), ror(a, 13), ror(a, 22)) + bxor(band(a, b), band(a, c), band(b, c)))
      h, g, f, e, d, c, b, a = g, f, e, tobit(d + t1), c, b, a, tobit(t1 + t2)
    end
    h0, h1, h2, h3 = tobit(h0 + a), tobit(h1 + b), tobit(h2 + c), tobit(h3 + d)
    h4, h5, h6, h7 = tobit(h4 + e), tobit(h5 + f), tobit(h6 + g), tobit(h7 + h)
  end
  local hs, out = { h0, h1, h2, h3, h4, h5, h6, h7 }, {}
  for i = 0, 7 do
    local v = hs[i + 1]
    out[4 * i], out[4 * i + 1] = band(rshift(v, 24), 255), band(rshift(v, 16), 255)
    out[4 * i + 2], out[4 * i + 3] = band(rshift(v, 8), 255), band(v, 255)
  end
  return out
end

local KEY = {}
for i = 0, 31 do KEY[i] = (i * 37 + 11) % 256 end

-- HMAC-SHA256 of msg[0..len-1]
local function hmac(msg, len)
  local inner, outer = {}, {}
  for i = 0, 63 do
    local k = i < 32 and KEY[i] or 0
    inner[i], outer[i] = bxor(k, 0x36), bxor(k, 0x5c)
  end
  for i = 0, len - 1 do inner[64 + i] = msg[i] end
  local d = sha256(inner, 64 + len)
  for i = 0, 31 do outer[64 + i] = d[i] end
  return sha256(outer, 96)
end

-- (rng * 1103515245 + 12345) mod 2^31, exact in doubles via a 16-bit split of rng.
local rng = 140014
local function draw(m)
  local al = rng % 65536
  local ah = (rng - al) / 65536
  local hi = (ah * 1103515245) % 32768
  rng = (hi * 65536 + al * 1103515245 + 12345) % 2147483648
  return floor(rng / 256) % m
end

local HEX = "0123456789abcdef"
local function hexb(v) local i = v + 1 return sbyte(HEX, i) end

-- byte-table builders: each returns the new length
local function put_lit(b, n, s)
  for i = 1, #s do b[n + i - 1] = sbyte(s, i) end
  return n + #s
end
local function put_dec(b, n, v) return put_lit(b, n, string.format("%d", v)) end
local function put_arr(b, n, src, cnt)
  for i = 0, cnt - 1 do b[n + i] = src[i] end
  return n + cnt
end
local function put_chars(b, n, cnt)
  for i = 0, cnt - 1 do b[n + i] = 97 + draw(26) end
  return n + cnt
end
local function put_id(b, n)
  local bad = draw(50)
  local idv = 1 + draw(99999)
  if bad == 0 then return put_lit(b, n, "abc") end
  return put_dec(b, n, idv)
end

local METHODS = { [0] = "GET ", "POST ", "PUT ", "DELETE " }

local function build_request(r)
  local path, body, tok = {}, {}, {}
  local pl, bl, tl, method, has_body = 0, 0, 0, 0, false
  local a = draw(100)
  if a < 5 then pl = put_lit(path, pl, "/health")
  elseif a < 15 then pl = put_lit(path, pl, "/users")
  elseif a < 35 then pl = put_lit(path, pl, "/users/"); pl = put_id(path, pl)
  elseif a < 45 then pl = put_lit(path, pl, "/users/"); pl = put_id(path, pl); pl = put_lit(path, pl, "/orders")
  elseif a < 55 then method, has_body = 1, true; pl = put_lit(path, pl, "/orders")
  elseif a < 65 then pl = put_lit(path, pl, "/orders/"); pl = put_id(path, pl)
  elseif a < 70 then method, has_body = 2, true; pl = put_lit(path, pl, "/orders/"); pl = put_id(path, pl)
  elseif a < 75 then method = 3; pl = put_lit(path, pl, "/orders/"); pl = put_id(path, pl)
  elseif a < 85 then
    pl = put_lit(path, pl, "/static/a"); pl = put_dec(path, pl, draw(10))
    pl = put_lit(path, pl, "/f"); pl = put_dec(path, pl, draw(100)); pl = put_lit(path, pl, ".js")
  elseif a < 90 then method, has_body = 1, true; pl = put_lit(path, pl, "/auth/login")
  elseif a < 95 then
    pl = put_lit(path, pl, "/v1/items/"); pl = put_id(path, pl)
    pl = put_lit(path, pl, "/tags/t"); pl = put_dec(path, pl, draw(50))
  elseif a < 98 then pl = put_lit(path, pl, "/nope/"); pl = put_id(path, pl)
  else method = 3; pl = put_lit(path, pl, "/users") end

  local t = draw(20)
  local uid = 1 + draw(9999)
  local ex = draw(1000)
  local has_tok = t >= 2
  if has_tok then
    local exp = (t == 4) and (NOW - 1 - ex) or (NOW + 1000 + ex)
    local sb = {}
    local sn = put_dec(sb, 0, uid)
    sb[sn] = 46; sn = sn + 1
    sn = put_dec(sb, sn, exp)
    local mac = hmac(sb, sn)
    if t < 4 then mac[0] = bxor(mac[0], 1) end
    tl = put_arr(tok, 0, sb, sn)
    tok[tl] = 46; tl = tl + 1
    for i = 0, 3 do
      tok[tl], tok[tl + 1] = hexb(rshift(mac[i], 4)), hexb(band(mac[i], 15))
      tl = tl + 2
    end
  end

  if has_body then
    local v = draw(10)
    local ws = draw(2)
    local sp = (ws == 1) and " " or ""
    if a >= 45 and a < 55 then
      local user, qty = 1 + draw(9999), 1 + draw(100)
      if v == 0 then qty = 0 end
      if v == 3 then user = 10000 end
      bl = put_lit(body, bl, "{\"user\":"); bl = put_lit(body, bl, sp); bl = put_dec(body, bl, user)
      bl = put_lit(body, bl, ","); bl = put_lit(body, bl, sp)
      bl = put_lit(body, bl, "\"qty\":"); bl = put_lit(body, bl, sp); bl = put_dec(body, bl, qty)
      local sku = {}
      put_chars(sku, 0, 4)
      if v ~= 1 then
        bl = put_lit(body, bl, ","); bl = put_lit(body, bl, sp)
        bl = put_lit(body, bl, "\"sku\":"); bl = put_lit(body, bl, sp)
        body[bl] = 34; bl = put_arr(body, bl + 1, sku, 4); body[bl] = 34; bl = bl + 1
      end
      body[bl] = 125; bl = bl + 1
      if v == 2 then bl = bl - 1 end
    elseif a >= 65 and a < 70 then
      local qty, nl = 1 + draw(100), 3 + draw(5)
      if v == 0 then qty = 101 end
      bl = put_lit(body, bl, "{")
      if v ~= 1 then
        bl = put_lit(body, bl, "\"qty\":"); bl = put_lit(body, bl, sp); bl = put_dec(body, bl, qty)
        bl = put_lit(body, bl, ","); bl = put_lit(body, bl, sp)
      end
      bl = put_lit(body, bl, "\"note\":"); bl = put_lit(body, bl, sp)
      body[bl] = 34; bl = put_chars(body, bl + 1, nl)
      body[bl], body[bl + 1] = 34, 125; bl = bl + 2
      if v == 2 then bl = bl - 1 end
    else
      local ul = 4 + draw(6)
      local un = {}
      put_chars(un, 0, ul)
      local pwl = 6 + draw(7)
      local pw = {}
      put_chars(pw, 0, pwl)
      if v == 0 then pwl = 0 end
      bl = put_lit(body, bl, "{")
      if v ~= 1 then
        bl = put_lit(body, bl, "\"user\":"); bl = put_lit(body, bl, sp)
        body[bl] = 34; bl = put_arr(body, bl + 1, un, ul); body[bl] = 34; bl = bl + 1
        bl = put_lit(body, bl, ","); bl = put_lit(body, bl, sp)
      end
      bl = put_lit(body, bl, "\"pass\":"); bl = put_lit(body, bl, sp)
      body[bl] = 34; bl = put_arr(body, bl + 1, pw, pwl)
      body[bl], body[bl + 1] = 34, 125; bl = bl + 2
      if v == 2 then body[bl] = 120; bl = bl + 1 end
    end
  end

  local n = put_lit(r, 0, METHODS[method])
  n = put_arr(r, n, path, pl)
  n = put_lit(r, n, " HTTP/1.1\r\nHost: api.local\r\n")
  if has_tok then
    n = put_lit(r, n, "Authorization: Bearer "); n = put_arr(r, n, tok, tl); n = put_lit(r, n, "\r\n")
  end
  if has_body then
    n = put_lit(r, n, "Content-Length: "); n = put_dec(r, n, bl); n = put_lit(r, n, "\r\n")
  end
  n = put_lit(r, n, "\r\n")
  return put_arr(r, n, body, bl)
end

-- ---- radix router: node = { kind, lit (byte table), len, first, next, h = {[0..3]} } ----
local nodes = { [0] = { kind = 0, lit = {}, len = 0, first = -1, next = -1, h = { [0] = -1, -1, -1, -1 } } }
local nn = 1

local function add_route(tpl, method, rid)
  local cur = 0
  for seg in tpl:sub(2):gmatch("[^/]+") do
    local c0 = sbyte(seg, 1)
    local kind = (c0 == 58) and 1 or (c0 == 42) and 2 or 0
    local found = -1
    local c = nodes[cur].first
    while c >= 0 and found < 0 do
      local nd = nodes[c]
      if nd.kind == kind then
        if kind ~= 0 then found = c
        else
          local same = nd.len == #seg
          if same then
            for q = 1, #seg do if nd.lit[q - 1] ~= sbyte(seg, q) then same = false end end
          end
          if same then found = c end
        end
      end
      c = nd.next
    end
    if found < 0 then
      local lit = {}
      if kind == 0 then for q = 1, #seg do lit[q - 1] = sbyte(seg, q) end end
      nodes[nn] = { kind = kind, lit = lit, len = kind == 0 and #seg or 0, first = -1, next = nodes[cur].first, h = { [0] = -1, -1, -1, -1 } }
      found = nn
      nn = nn + 1
      nodes[cur].first = found
    end
    cur = found
  end
  nodes[cur].h[method] = rid
end

local AUTH = { [0] = false, true, true, true, true, true, true, true, false, false, true }

local function parse_digits(b, p, e)
  if e <= p or e - p > 10 then return -1 end
  local v = 0
  for i = p, e - 1 do
    local c = b[i]
    if c < 48 or c > 57 then return -1 end
    v = v * 10 + (c - 48)
  end
  return v
end

local function starts_at(b, n, p, s)
  if p + #s > n then return false end
  for i = 1, #s do if b[p + i - 1] ~= sbyte(s, i) then return false end end
  return true
end

local function verify_token(b, off, len)
  local e, d1, d2 = off + len, -1, -1
  for i = off, e - 1 do
    if b[i] == 46 then
      if d1 < 0 then d1 = i elseif d2 < 0 then d2 = i else return -1 end
    end
  end
  if d1 < 0 or d2 < 0 or e - d2 - 1 ~= 8 then return -1 end
  local u, ex = parse_digits(b, off, d1), parse_digits(b, d1 + 1, d2)
  if u < 0 or ex < 0 then return -1 end
  local msg = {}
  for i = 0, d2 - off - 1 do msg[i] = b[off + i] end
  local mac = hmac(msg, d2 - off)
  for i = 0, 3 do
    if b[d2 + 1 + 2 * i] ~= hexb(rshift(mac[i], 4)) or b[d2 + 2 + 2 * i] ~= hexb(band(mac[i], 15)) then return -1 end
  end
  if ex <= NOW then return -1 end
  return u
end

local function skip_ws(b, p, e)
  while p < e do
    local c = b[p]
    if c == 32 or c == 9 or c == 10 or c == 13 then p = p + 1 else break end
  end
  return p
end

local KEYS = { user = 1, qty = 2, sku = 3, note = 4, pass = 5 }
local function key_id(b, p, e)
  local t = {}
  for i = p, e - 1 do t[#t + 1] = schar(b[i]) end
  return KEYS[table.concat(t)] or 0
end

-- f[id*3] present, f[id*3+1] type (1 int, 2 str), f[id*3+2] value / string length
local F = {}
local function parse_json(b, off, len)
  local e = off + len
  for i = 0, 17 do F[i] = 0 end
  local p = skip_ws(b, off, e)
  if p >= e or b[p] ~= 123 then return false end
  p = skip_ws(b, p + 1, e)
  if p < e and b[p] == 125 then
    p = skip_ws(b, p + 1, e)
    return p == e
  end
  while true do
    if p >= e or b[p] ~= 34 then return false end
    p = p + 1
    local ks = p
    while p < e and b[p] ~= 34 do
      if b[p] == 92 then return false end
      p = p + 1
    end
    if p >= e then return false end
    local id = key_id(b, ks, p)
    p = skip_ws(b, p + 1, e)
    if p >= e or b[p] ~= 58 then return false end
    p = skip_ws(b, p + 1, e)
    if p >= e then return false end
    local ty, val
    if b[p] == 34 then
      p = p + 1
      local vs = p
      while p < e and b[p] ~= 34 do
        if b[p] == 92 then return false end
        p = p + 1
      end
      if p >= e then return false end
      ty, val = 2, p - vs
      p = p + 1
    else
      local neg = false
      if b[p] == 45 then neg = true; p = p + 1 end
      local ds = p
      while p < e and b[p] >= 48 and b[p] <= 57 do p = p + 1 end
      local v = parse_digits(b, ds, p)
      if v < 0 then return false end
      ty, val = 1, neg and -v or v
    end
    if id > 0 then F[id * 3], F[id * 3 + 1], F[id * 3 + 2] = 1, ty, val end
    p = skip_ws(b, p, e)
    if p >= e then return false end
    if b[p] == 44 then
      p = skip_ws(b, p + 1, e)
    elseif b[p] == 125 then
      p = p + 1
      break
    else
      return false
    end
  end
  p = skip_ws(b, p, e)
  return p == e
end

local function has_int(id, lo, hi) return F[id * 3] ~= 0 and F[id * 3 + 1] == 1 and F[id * 3 + 2] >= lo and F[id * 3 + 2] <= hi end
local function has_str(id, lo, hi) return F[id * 3] ~= 0 and F[id * 3 + 1] == 2 and F[id * 3 + 2] >= lo and F[id * 3 + 2] <= hi end

local function body_valid(rid, b, off, len)
  if not parse_json(b, off, len) then return false end
  if rid == 4 then return has_int(1, 1, 9999) and has_int(2, 1, 100) and has_str(3, 4, 4) end
  if rid == 6 then return has_int(2, 1, 100) and (F[12] == 0 or has_str(4, 0, 1000)) end
  return has_str(1, 1, 64) and has_str(5, 1, 64)
end

local pid_out = 0

local function handle(b, n)
  pid_out = 0
  local method, p
  if starts_at(b, n, 0, "GET ") then method, p = 0, 4
  elseif starts_at(b, n, 0, "POST ") then method, p = 1, 5
  elseif starts_at(b, n, 0, "PUT ") then method, p = 2, 4
  elseif starts_at(b, n, 0, "DELETE ") then method, p = 3, 7
  else return 400 end
  local po = p
  while p < n and b[p] ~= 32 do p = p + 1 end
  local pe = p
  while p < n and b[p] ~= 13 do p = p + 1 end
  p = p + 2

  local tok_off, tok_len, clen = -1, 0, -1
  while p < n do
    if b[p] == 13 then p = p + 2; break end
    local le = p
    while le < n and b[le] ~= 13 do le = le + 1 end
    if starts_at(b, n, p, "Authorization: Bearer ") then tok_off, tok_len = p + 22, le - (p + 22)
    elseif starts_at(b, n, p, "Content-Length: ") then clen = parse_digits(b, p + 16, le) end
    p = le + 2
  end
  local body_off, body_len = p, n - p
  if clen >= 0 and clen ~= body_len then return 400 end

  local plen = pe - po
  local cur, found, nparam, bad_id, pid = 0, true, 0, false, -1
  if plen < 1 or b[po] ~= 47 then found = false end
  local i = 1
  while found and i <= plen do
    local e = i
    while e < plen and b[po + e] ~= 47 do e = e + 1 end
    local sl = e - i
    if sl == 0 then found = false; break end
    local st, pm, wc = -1, -1, -1
    local c = nodes[cur].first
    while c >= 0 do
      local nd = nodes[c]
      local k = nd.kind
      if k == 0 then
        if nd.len == sl then
          local same, lit = true, nd.lit
          for q = 0, sl - 1 do
            if lit[q] ~= b[po + i + q] then same = false; break end
          end
          if same then st = c; break end
        end
      elseif k == 1 then pm = c
      else wc = c end
      c = nd.next
    end
    if st >= 0 then cur = st
    elseif pm >= 0 then
      cur = pm
      if nparam == 0 then
        pid = parse_digits(b, po + i, po + e)
        if pid < 0 then bad_id = true end
      end
      nparam = nparam + 1
    elseif wc >= 0 then cur = wc; break
    else found = false; break end
    i = e + 1
  end
  if not found then return 404 end
  local hs = nodes[cur].h
  local rid = hs[method]
  if rid < 0 then
    return (hs[0] >= 0 or hs[1] >= 0 or hs[2] >= 0 or hs[3] >= 0) and 405 or 404
  end
  pid_out = (bad_id or pid < 0) and 0 or pid

  if AUTH[rid] then
    if tok_off < 0 then return 401 end
    local uid = verify_token(b, tok_off, tok_len)
    if uid < 0 then return 401 end
    if (rid == 6 or rid == 7) and uid % 13 == 0 then return 403 end
  end
  if bad_id then return 400 end
  if (rid == 4 or rid == 6 or rid == 9) and not body_valid(rid, b, body_off, body_len) then return 400 end
  if rid == 2 and pid > 90000 then return 404 end
  if rid == 4 then return 201 end
  if rid == 5 and pid % 7 == 0 then return 404 end
  if rid == 7 then return 204 end
  return 200
end

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

local cs, sum_pid, ok2xx = 0, 0, 0
local by_status = {}
for s = 0, 599 do by_status[s] = 0 end
local req = {}
for i = 0, NREQ - 1 do
  local n = build_request(req)
  local st = handle(req, n)
  by_status[st] = by_status[st] + 1
  cs = cs + st * (i % 97 + 1)
  sum_pid = sum_pid + pid_out
  if st < 300 then ok2xx = ok2xx + 1 end
end
local mix = 0
for s = 0, 599 do mix = mix + by_status[s] * (s % 53 + 1) * (floor(s / 100) + 1) end
cs = cs + sum_pid + 5 * ok2xx + 7 * mix
print(string.format("CHECKSUM=%d", cs))
