-- 16_crypto_pq_argon2_suite — Argon2id-like KDF (m=8KiB t=2 p=1) x32, SHA-256 Merkle of tags,
-- toy lattice negacyclic NTT poly-mul (q=12289, n=256)
local ffi = require("ffi")
local bit = bit
local band, bor, bxor, lshift, rshift = bit.band, bit.bor, bit.bxor, bit.lshift, bit.rshift

local MOD = 1000000007
local NH, BW, NB, PASSES = 32, 64, 32, 2
local Q, N, PAIRS = 12289, 256, 6
local P32 = 4294967296

local rng = 16001
local function next_rand()
  rng = rng * 48271 % 2147483647
  return rng
end

local function u32(x) return x % P32 end
local function xor32(a, b) return u32(bxor(a, b)) end
local function rot32(x, r) return u32(bor(rshift(x, r), lshift(x, 32 - r))) end
local function blamka(x, y) return u32(x + y + 2 * band(x, 0xFFFF) * band(y, 0xFFFF)) end

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

local function g_mix(z, ia, ib, ic, id)
  local a, b, c, d = z[ia], z[ib], z[ic], z[id]
  a = blamka(a, b); d = rot32(xor32(d, a), 16)
  c = blamka(c, d); b = rot32(xor32(b, c), 12)
  a = blamka(a, b); d = rot32(xor32(d, a), 8)
  c = blamka(c, d); b = rot32(xor32(b, c), 7)
  z[ia], z[ib], z[ic], z[id] = a, b, c, d
end

local function p_round(z, base, s)
  g_mix(z, base, base + 2 * s, base + 4 * s, base + 6 * s)
  g_mix(z, base + s, base + 3 * s, base + 5 * s, base + 7 * s)
  g_mix(z, base, base + 3 * s, base + 4 * s, base + 7 * s)
  g_mix(z, base + s, base + 2 * s, base + 5 * s, base + 6 * s)
end

-- blocks are `double[BW]` windows into one flat array at word offsets
local function compress(blocks, xo, yo, out)
  local r = ffi.new("double[?]", BW)
  for i = 0, BW - 1 do
    r[i] = xor32(blocks[xo + i], blocks[yo + i])
    out[i] = r[i]
  end
  for row = 0, 7 do p_round(out, row * 8, 1) end
  for col = 0, 7 do p_round(out, col, 8) end
  for i = 0, BW - 1 do out[i] = xor32(out[i], r[i]) end
end

local function init_block(h0, j, blocks)
  local msg = ffi.new("uint8_t[33]")
  for i = 0, 31 do msg[i] = h0[i] end
  msg[32] = j
  local d = sha256(msg, 33)
  local o = j * BW
  for k = 0, 7 do
    blocks[o + k] = d[4 * k] * 16777216 + d[4 * k + 1] * 65536 + d[4 * k + 2] * 256 + d[4 * k + 3]
  end
  for k = 8, BW - 1 do
    blocks[o + k] = u32(blocks[o + k - 8] + band(blocks[o + k - 3], 0xFFFF) * 40503 + rshift(blocks[o + k - 1], 16) * 69069)
  end
end

local function argon2_like(idx_pw)
  local msg = ffi.new("uint8_t[36]")
  msg[0], msg[1], msg[2], msg[3] = idx_pw, NB * 256 / 1024, PASSES, 1
  for i = 4, 35 do msg[i] = next_rand() % 256 end
  local h0 = sha256(msg, 36)
  local blocks = ffi.new("double[?]", NB * BW)
  init_block(h0, 0, blocks)
  init_block(h0, 1, blocks)
  local nw = ffi.new("double[?]", BW)
  for pass = 0, PASSES - 1 do
    for idx = (pass == 0 and 2 or 0), NB - 1 do
      local po = (idx + NB - 1) % NB * BW
      local r = blocks[po]
      if pass == 0 and idx < NB / 2 then r = ((pass * 131 + idx) * 40503 + 12345) * 69069 % P32 end
      local r16 = math.floor(r / 65536)
      local x = math.floor(r16 * r16 / 65536)
      local ref
      if pass == 0 then
        local area = idx - 1
        ref = area - 1 - math.floor(area * x / 65536)
      else
        local area = NB - 2
        ref = (idx + 1 + (area - 1 - math.floor(area * x / 65536))) % NB
      end
      compress(blocks, po, ref * BW, nw)
      local o = idx * BW
      if pass == 0 then
        for i = 0, BW - 1 do blocks[o + i] = nw[i] end
      else
        for i = 0, BW - 1 do blocks[o + i] = xor32(blocks[o + i], nw[i]) end
      end
    end
  end
  local bytes = ffi.new("uint8_t[256]")
  local lo = (NB - 1) * BW
  for i = 0, BW - 1 do
    local w = blocks[lo + i]
    bytes[4 * i], bytes[4 * i + 1], bytes[4 * i + 2], bytes[4 * i + 3] =
      rshift(w, 24), band(rshift(w, 16), 255), band(rshift(w, 8), 255), band(w, 255)
  end
  return sha256(bytes, 256)
end

local function pow_mod(b, e)
  local r = 1
  b = b % Q
  while e > 0 do
    if e % 2 == 1 then r = r * b % Q end
    b = b * b % Q
    e = math.floor(e / 2)
  end
  return r
end

local function ntt(a, w)
  local j = 0
  for i = 1, N - 1 do
    local b = N / 2
    while band(j, b) ~= 0 do
      j = bxor(j, b)
      b = b / 2
    end
    j = bxor(j, b)
    if i < j then a[i], a[j] = a[j], a[i] end
  end
  local len = 2
  while len <= N do
    local wl, half = pow_mod(w, N / len), len / 2
    for i = 0, N - 1, len do
      local wn = 1
      for k = 0, half - 1 do
        local u = a[i + k]
        local v = a[i + k + half] * wn % Q
        a[i + k] = (u + v) % Q
        a[i + k + half] = (u + Q - v) % Q
        wn = wn * wl % Q
      end
    end
    len = len * 2
  end
end

local function polymul(a0, b0, psi)
  local psi_inv, omega, n_inv = pow_mod(psi, Q - 2), psi * psi % Q, pow_mod(N, Q - 2)
  local a, b, c = ffi.new("double[?]", N), ffi.new("double[?]", N), ffi.new("double[?]", N)
  local p = 1
  for i = 0, N - 1 do
    a[i], b[i] = a0[i] * p % Q, b0[i] * p % Q
    p = p * psi % Q
  end
  ntt(a, omega)
  ntt(b, omega)
  for i = 0, N - 1 do c[i] = a[i] * b[i] % Q end
  ntt(c, pow_mod(omega, Q - 2))
  p = n_inv
  for i = 0, N - 1 do
    c[i] = c[i] * p % Q
    p = p * psi_inv % Q
  end
  return c
end

local function schoolbook(a, b)
  local c = ffi.new("double[?]", N)
  for i = 0, N - 1 do
    for j = 0, N - 1 do
      local pr, k = a[i] * b[j] % Q, i + j
      if k >= N then c[k - N] = (c[k - N] + Q - pr) % Q else c[k] = (c[k] + pr) % Q end
    end
  end
  return c
end

local level = {}
for i = 0, NH - 1 do level[i] = argon2_like(i) end
local n = NH
while n > 1 do
  local m = math.floor((n + 1) / 2)
  local nxt = {}
  for i = 0, m - 1 do
    local a = level[2 * i]
    local b = (2 * i + 1 < n) and level[2 * i + 1] or a
    local cat = ffi.new("uint8_t[64]")
    for k = 0, 31 do cat[k], cat[k + 32] = a[k], b[k] end
    nxt[i] = sha256(cat, 64)
  end
  level, n = nxt, m
end
local cs = 0
for i = 0, 31 do cs = (cs * 131 + level[0][i]) % MOD end

local psi = pow_mod(11, (Q - 1) / (2 * N))
local ok = (pow_mod(psi, N) == Q - 1) and 1 or 0
for pr = 0, PAIRS - 1 do
  local a, b = ffi.new("double[?]", N), ffi.new("double[?]", N)
  for i = 0, N - 1 do a[i] = next_rand() % Q end
  for i = 0, N - 1 do b[i] = next_rand() % Q end
  local c = polymul(a, b, psi)
  if pr < 2 then
    local ref = schoolbook(a, b)
    for i = 0, N - 1 do if ref[i] ~= c[i] then ok = 0 end end
  end
  for i = 0, N - 1 do cs = (cs * 131 + c[i]) % MOD end
end
cs = (cs * 131 + ok) % MOD
print(string.format("CHECKSUM=%d", cs))
