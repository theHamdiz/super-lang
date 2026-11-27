-- 20_end_to_end_lakehouse_service — in-process pipeline:
-- ingest JSONL customers + CSV order batches -> filter/join/group -> fixed-point GD linear fit on the
-- per-product aggregates -> serve HTTP-like request bytes (bearer-token check via SHA-256, Merkle root of
-- the group table in every response) -> checksum of response bytes.
local ffi = require("ffi")
local bit = bit
local band, bor, bxor, lshift, rshift = bit.band, bit.bor, bit.bxor, bit.lshift, bit.rshift
local floor, byte, sfmt = math.floor, string.byte, string.format

local MOD = 1000000007
local NCUST, NPROD, NREG, NTIER = 500, 50, 8, 4
local NGRP = NREG * NTIER
local BATCHES, ROWS_PB, NREQ, NCLIENT, ITERS = 6, 4000, 3000, 8, 400
local P32 = 4294967296
local function u32(x) return x % P32 end

local rng = 20020
local function next_rand()
  rng = rng * 48271 % 2147483647
  return rng
end

local function sdiv(a, d)
  if a >= 0 then return floor(a / d) end
  return -floor(-a / d)
end

-- SHA-256 over a 0-based uint8 buffer; returns uint8_t[32]
local K = ffi.new("uint32_t[64]", {
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
  0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
  0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
  0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
  0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
  0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
  0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
  0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
  0xc67178f2,
})

local function rotr(x, n) return bor(rshift(x, n), lshift(x, 32 - n)) end

local function sha256(msg, len)
  local h = ffi.new("uint32_t[8]", 0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                    0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19)
  local total = len + 1
  while total % 64 ~= 56 do total = total + 1 end
  total = total + 8
  local buf = ffi.new("uint8_t[?]", total)
  for i = 0, len - 1 do buf[i] = msg[i] end
  buf[len] = 0x80
  local bits = len * 8
  for i = 0, 7 do buf[total - 1 - i] = math.floor(bits / 2 ^ (8 * i)) % 256 end
  local w = ffi.new("uint32_t[64]")
  for off = 0, total - 1, 64 do
    for i = 0, 15 do
      local b = off + 4 * i
      w[i] = buf[b] * 16777216 + buf[b + 1] * 65536 + buf[b + 2] * 256 + buf[b + 3]
    end
    for i = 16, 63 do
      local w15, w2 = w[i - 15], w[i - 2]
      local s0 = bxor(rotr(w15, 7), rotr(w15, 18), rshift(w15, 3))
      local s1 = bxor(rotr(w2, 17), rotr(w2, 19), rshift(w2, 10))
      w[i] = w[i - 16] + s0 + w[i - 7] + s1
    end
    local a, b, c, d, e, f, g, hh = h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]
    for i = 0, 63 do
      local t1 = hh + bxor(rotr(e, 6), rotr(e, 11), rotr(e, 25)) + bxor(band(e, f), band(bit.bnot(e), g)) + K[i] + w[i]
      local t2 = bxor(rotr(a, 2), rotr(a, 13), rotr(a, 22)) + bxor(band(a, b), band(a, c), band(b, c))
      hh, g, f, e, d, c, b, a = g, f, e, u32(d + t1), c, b, a, u32(t1 + t2)
    end
    h[0] = h[0] + a; h[1] = h[1] + b; h[2] = h[2] + c; h[3] = h[3] + d
    h[4] = h[4] + e; h[5] = h[5] + f; h[6] = h[6] + g; h[7] = h[7] + hh
  end
  local out = ffi.new("uint8_t[32]")
  for i = 0, 7 do
    local v = h[i]
    out[4 * i], out[4 * i + 1], out[4 * i + 2], out[4 * i + 3] =
      rshift(v, 24), band(rshift(v, 16), 255), band(rshift(v, 8), 255), band(v, 255)
  end
  return out
end

local HEXD = "0123456789abcdef"
local function sha_str(s) -- sha256 of a Lua string -> uint8_t[32]
  local buf = ffi.new("uint8_t[?]", #s + 1)
  ffi.copy(buf, s, #s)
  return sha256(buf, #s)
end
local function hex_of(d, n)
  local t = {}
  for i = 0, n - 1 do
    local v = d[i]
    t[#t + 1] = HEXD:sub(rshift(v, 4) + 1, rshift(v, 4) + 1) .. HEXD:sub(band(v, 15) + 1, band(v, 15) + 1)
  end
  return table.concat(t)
end

local CRC = ffi.new("uint32_t[256]")
for i = 0, 255 do
  local c = i
  for _ = 1, 8 do
    if band(c, 1) == 1 then c = bxor(rshift(c, 1), 0xEDB88320) else c = rshift(c, 1) end
  end
  CRC[i] = c
end
local function crc32(s)
  local c = 0xFFFFFFFF
  for i = 1, #s do c = bxor(CRC[band(bxor(c, byte(s, i)), 255)], rshift(c, 8)) end
  return bxor(c, 0xFFFFFFFF) % P32
end

-- Parse the decimal run at 1-based p; returns value (or -1) and the new position.
local function scan_int(s, p)
  local v, have = 0, false
  local d = byte(s, p)
  while d and d >= 48 and d <= 57 do
    v = v * 10 + d - 48
    p = p + 1
    have = true
    d = byte(s, p)
  end
  return have and v or -1, p
end
local function is_digit(c) return c ~= nil and c >= 48 and c <= 57 end

local cs = 0
local function fold(v) cs = (cs * 131 + v) % MOD end

-- ---- ingest customers (JSONL) ----
local lines = {}
for i = 0, NCUST - 1 do
  local id, r = i * 7919 % NCUST, next_rand() % NREG
  local t = next_rand() % NTIER
  lines[#lines + 1] = sfmt('{"id":%d,"region":%d,"tier":%d}\n', id, r, t)
end
local cj = table.concat(lines)
local clen = #cj
local region_of, tier_of = {}, {}
for i = 0, NCUST - 1 do region_of[i], tier_of[i] = -1, -1 end
local ncust = 0
local p = 1
while p <= clen do
  while p <= clen and not is_digit(byte(cj, p)) do p = p + 1 end
  local id, r, t
  id, p = scan_int(cj, p)
  while not is_digit(byte(cj, p)) do p = p + 1 end
  r, p = scan_int(cj, p)
  while not is_digit(byte(cj, p)) do p = p + 1 end
  t, p = scan_int(cj, p)
  region_of[id], tier_of[id] = r, t
  ncust = ncust + 1
  while p <= clen and byte(cj, p) ~= 10 do p = p + 1 end
  p = p + 1
end

-- ---- ingest + filter/join/group order batches (CSV) ----
local function zeros(n) local t = {} for i = 0, n - 1 do t[i] = 0 end return t end
local grp_cnt, grp_qty, grp_rev = zeros(NGRP), zeros(NGRP), zeros(NGRP)
local prod_cnt, prod_qty, prod_rev, prod_price = zeros(NPROD), zeros(NPROD), zeros(NPROD), zeros(NPROD)
local reg_prod_rev = zeros(NREG * NPROD)
local oid, rows, null_cust, bad_qty, valid = 0, 0, 0, 0, 0
local f = { [0] = 0, 0, 0, 0, 0 }
for _ = 1, BATCHES do
  local ls = { "order_id,cust_id,product,qty,price_cents\n" }
  for _ = 1, ROWS_PB do
    local nf, c, pr = next_rand() % 100, next_rand() % NCUST, next_rand() % NPROD
    local q = next_rand() % 9
    local price = 100 + pr * 37 + next_rand() % 200
    if nf ~= 0 then
      ls[#ls + 1] = sfmt("%d,%d,%d,%d,%d\n", oid, c, pr, q, price)
    else
      ls[#ls + 1] = sfmt("%d,,%d,%d,%d\n", oid, pr, q, price)
    end
    oid = oid + 1
  end
  local ob = table.concat(ls)
  local olen = #ob
  local pos = 1
  while byte(ob, pos) ~= 10 do pos = pos + 1 end
  pos = pos + 1
  while pos <= olen do
    for k = 0, 4 do
      f[k], pos = scan_int(ob, pos)
      pos = pos + 1
    end
    rows = rows + 1
    if f[1] < 0 then
      null_cust = null_cust + 1
    elseif f[3] <= 0 then
      bad_qty = bad_qty + 1
    else
      valid = valid + 1
      local rev = f[3] * f[4]
      local g = region_of[f[1]] * NTIER + tier_of[f[1]]
      grp_cnt[g] = grp_cnt[g] + 1; grp_qty[g] = grp_qty[g] + f[3]; grp_rev[g] = grp_rev[g] + rev
      local pd = f[2]
      prod_cnt[pd] = prod_cnt[pd] + 1; prod_qty[pd] = prod_qty[pd] + f[3]
      prod_rev[pd] = prod_rev[pd] + rev; prod_price[pd] = prod_price[pd] + f[4]
      local ri = region_of[f[1]] * NPROD + pd
      reg_prod_rev[ri] = reg_prod_rev[ri] + rev
    end
  end
end

-- ---- fit y = w*x + b (w in Q16) by gradient descent on per-product aggregates ----
local xs, ys, n = {}, {}, 0
for pd = 0, NPROD - 1 do
  if prod_cnt[pd] > 0 then
    xs[n] = floor(floor(prod_price[pd] / prod_cnt[pd]) / 16)
    ys[n] = floor(floor(prod_rev[pd] / prod_cnt[pd]) / 16)
    n = n + 1
  end
end
local w, bias = 0, 0
for _ = 1, ITERS do
  local gw, gb = 0, 0
  for i = 0, n - 1 do
    local err = sdiv(w * xs[i], 65536) + bias - ys[i]
    gw = gw + err * xs[i]
    gb = gb + err
  end
  w = w - sdiv(gw * 65536, n * 8000)
  bias = bias - sdiv(gb, n * 4)
end
local loss = 0
for i = 0, n - 1 do
  local err = sdiv(w * xs[i], 65536) + bias - ys[i]
  loss = loss + err * err
end
local pred_cents = {}
for pd = 0, NPROD - 1 do
  local x = prod_cnt[pd] > 0 and floor(floor(prod_price[pd] / prod_cnt[pd]) / 16) or 0
  local y = sdiv(w * x, 65536) + bias
  pred_cents[pd] = (y > 0 and y or 0) * 16
end

-- ---- merkle root over the region x tier group table ----
local lvl = ffi.new("uint8_t[?]", NGRP * 32)
for g = 0, NGRP - 1 do
  local leaf = ffi.new("uint8_t[32]")
  local vals = { [0] = g, grp_cnt[g], grp_qty[g], grp_rev[g] }
  for k = 0, 3 do
    local v = vals[k]
    for j = 7, 0, -1 do
      leaf[k * 8 + j] = v % 256
      v = floor(v / 256)
    end
  end
  local d = sha256(leaf, 32)
  for j = 0, 31 do lvl[g * 32 + j] = d[j] end
end
local m = NGRP
while m > 1 do
  for i = 0, m / 2 - 1 do
    local cat = ffi.new("uint8_t[64]")
    for j = 0, 31 do
      cat[j] = lvl[2 * i * 32 + j]
      cat[32 + j] = lvl[(2 * i + 1) * 32 + j]
    end
    local d = sha256(cat, 64)
    for j = 0, 31 do lvl[i * 32 + j] = d[j] end
  end
  m = m / 2
end
local root_hex = hex_of(lvl, 32)

-- ---- client tokens ----
local tokens = {}
for k = 0, NCLIENT - 1 do tokens[k] = hex_of(sha_str("key-" .. k), 8) end

-- ---- serve ----
local ok200, un401, nf404, total_bytes, resp_cs = 0, 0, 0, 0, 0
for _ = 1, NREQ do
  local k, pq, rg = next_rand() % NCLIENT, next_rand() % 60, next_rand() % NREG
  local bad = next_rand() % 100
  local at = next_rand() % 16
  local tok = tokens[k]
  if bad < 8 then tok = tok:sub(1, at) .. "z" .. tok:sub(at + 2) end
  local req = sfmt("GET /v1/product/%d?region=%d HTTP/1.1\r\nHost: lake\r\nX-Client: %d\r\nAuthorization: Bearer %s\r\n\r\n",
    pq, rg, k, tok)
  local rl = #req
  -- server side: parse request bytes
  local status, pp, rr = 400, -1, -1
  if rl > 16 and req:sub(1, 16) == "GET /v1/product/" then
    local q = 17
    pp, q = scan_int(req, q)
    q = q + 8
    rr, q = scan_int(req, q)
    local xc = req:find("X-Client: ", q, true)
    local bt = req:find("Bearer ", q, true)
    if xc and bt then
      local kk = scan_int(req, xc + 10)
      if kk >= 0 and kk < NCLIENT and bt + 6 + 16 <= rl and req:sub(bt + 7, bt + 22) == tokens[kk] then
        status = (pp >= 0 and pp < NPROD and rr >= 0 and rr < NREG and prod_cnt[pp] > 0) and 200 or 404
      else
        status = 401
      end
    end
  end
  local body, head
  if status == 200 then
    body = sfmt('{"product":%d,"region":%d,"orders":%d,"revenue":%d,"region_revenue":%d,"forecast":%d,"root":"%s"}',
      pp, rr, prod_cnt[pp], prod_rev[pp], reg_prod_rev[rr * NPROD + pp], pred_cents[pp], root_hex)
    ok200 = ok200 + 1
    head = "HTTP/1.1 200 OK"
  elseif status == 404 then
    body = '{"error":"not found"}'; nf404 = nf404 + 1; head = "HTTP/1.1 404 Not Found"
  elseif status == 401 then
    body = '{"error":"unauthorized"}'; un401 = un401 + 1; head = "HTTP/1.1 401 Unauthorized"
  else
    body = '{"error":"bad request"}'; head = "HTTP/1.1 400 Bad Request"
  end
  local resp = head .. "\r\nContent-Length: " .. #body .. "\r\n\r\n" .. body
  total_bytes = total_bytes + #resp
  resp_cs = (resp_cs * 131 + crc32(resp)) % MOD
end

-- ---- checksum ----
for _, v in ipairs({ ncust, rows, null_cust, bad_qty, valid }) do fold(v) end
for g = 0, NGRP - 1 do fold(grp_cnt[g]); fold(grp_qty[g]); fold(grp_rev[g]) end
for pd = 0, NPROD - 1 do fold(prod_cnt[pd]); fold(prod_rev[pd]) end
for _, v in ipairs({ w, bias, loss }) do fold(v) end
for j = 0, 31 do fold(lvl[j]) end
for _, v in ipairs({ ok200, un401, nf404, total_bytes, resp_cs }) do fold(v) end
print(sfmt("CHECKSUM=%d", cs))
