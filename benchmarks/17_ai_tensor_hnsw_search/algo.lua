-- 17_ai_tensor_hnsw_search — fixed-point (Q10) transformer blocks (causal attention, softmax,
-- ReLU FFN, RMS norm) on 12 sequences, then brute-force top-10 cosine search over 4096 unit
-- vectors of dim 32 using the pooled embeddings as queries. Integer-only => bit-exact everywhere.
local ffi = require("ffi")
local floor = math.floor

local MOD = 1000000007
local BIAS = 2 ^ 40
local D, T, FF, LAYERS, SEQS, NV, K = 32, 16, 64, 2, 12, 4096, 10

local rng = 17017
local function next_rand()
  rng = rng * 48271 % 2147483647
  return rng
end

-- doubles hold every value exactly (< 2^53); sar = arithmetic shift right, idiv = non-negative division
local function sar(x, k) return floor(x / 2 ^ k) end
local function idiv(a, b) return floor(a / b) end
local function arr(n) return ffi.new("double[?]", n) end

local function isqrt(n)
  if n == 0 then return 0 end
  local x, y = n, sar(n + 1, 1)
  while y < x do
    x = y
    y = sar(x + idiv(n, x), 1)
  end
  return x
end

local function exp_frac(f)
  local term, sum = 1024, 1024
  for k = 1, 6 do
    term = idiv(sar(term * f, 10), k)
    if k % 2 == 1 then sum = sum - term else sum = sum + term end
  end
  return sum < 0 and 0 or sum
end

local function exp_q(x, e1)
  local a = -x
  local n, f = sar(a, 10), a % 1024
  if n >= 16 then return 0 end
  local r = exp_frac(f)
  for _ = 1, n do r = sar(r * e1, 10) end
  return r
end

local function matmul(x, w, y, din, dout)
  for t = 0, T - 1 do
    for o = 0, dout - 1 do
      local s = 0
      for i = 0, din - 1 do s = s + x[t * din + i] * w[i * dout + o] end
      y[t * dout + o] = sar(s, 10)
    end
  end
end

local function rms_norm(h)
  for t = 0, T - 1 do
    local ss = 0
    for d = 0, D - 1 do ss = ss + h[t * D + d] * h[t * D + d] end
    local rms = isqrt(idiv(ss, D))
    local inv = idiv(1048576, rms + 1)
    for d = 0, D - 1 do h[t * D + d] = sar(h[t * D + d] * inv, 10) end
  end
end

local function fill_w(n)
  local w = arr(n)
  for i = 0, n - 1 do w[i] = next_rand() % 257 - 128 end
  return w
end

local function block(ly, h, e1)
  local q, k, v, at, o = arr(T * D), arr(T * D), arr(T * D), arr(T * D), arr(T * D)
  local f, g = arr(T * FF), arr(T * D)
  matmul(h, ly.wq, q, D, D)
  matmul(h, ly.wk, k, D, D)
  matmul(h, ly.wv, v, D, D)
  local sc, pr = arr(T), arr(T)
  for t = 0, T - 1 do
    local mx, sum = -2 ^ 60, 0
    for j = 0, t do
      local s = 0
      for d = 0, D - 1 do s = s + q[t * D + d] * k[j * D + d] end
      sc[j] = sar(s, 13)
      if sc[j] > mx then mx = sc[j] end
    end
    for j = 0, t do
      pr[j] = exp_q(sc[j] - mx, e1)
      sum = sum + pr[j]
    end
    for j = 0, t do pr[j] = idiv(pr[j] * 1024, sum) end
    for d = 0, D - 1 do
      local s = 0
      for j = 0, t do s = s + pr[j] * v[j * D + d] end
      at[t * D + d] = sar(s, 10)
    end
  end
  matmul(at, ly.wo, o, D, D)
  for i = 0, T * D - 1 do h[i] = h[i] + o[i] end
  rms_norm(h)
  matmul(h, ly.w1, f, D, FF)
  for i = 0, T * FF - 1 do if f[i] < 0 then f[i] = 0 end end
  matmul(f, ly.w2, g, FF, D)
  for i = 0, T * D - 1 do h[i] = h[i] + g[i] end
  rms_norm(h)
end

local function normalize(raw, out)
  local ss = 0
  for d = 0, D - 1 do ss = ss + raw[d] * raw[d] end
  local inv = idiv(1048576, isqrt(ss) + 1)
  for d = 0, D - 1 do out[d] = sar(raw[d] * inv, 10) end
end

local e1 = exp_frac(1024)
local layers = {}
for l = 1, LAYERS do
  layers[l] = { wq = fill_w(D * D), wk = fill_w(D * D), wv = fill_w(D * D),
                wo = fill_w(D * D), w1 = fill_w(D * FF), w2 = fill_w(FF * D) }
end
local db = arr(NV * D)
local raw, vec = arr(D), arr(D)
for i = 0, NV - 1 do
  for d = 0, D - 1 do raw[d] = next_rand() % 2049 - 1024 end
  normalize(raw, vec)
  for d = 0, D - 1 do db[i * D + d] = vec[d] end
end
local cs = 0
local function fold(v) cs = (cs * 131 + v + BIAS) % MOD end
for _ = 1, SEQS do
  local h = arr(T * D)
  for i = 0, T * D - 1 do h[i] = next_rand() % 2049 - 1024 end
  for l = 1, LAYERS do block(layers[l], h, e1) end
  local pooled, query = arr(D), arr(D)
  for d = 0, D - 1 do
    local sum = 0
    for t = 0, T - 1 do sum = sum + h[t * D + d] end
    pooled[d] = sar(sum, 4)
    fold(pooled[d])
  end
  normalize(pooled, query)
  local top_s, top_i = arr(K), arr(K)
  local cnt = 0
  for i = 0, NV - 1 do
    local sc, o = 0, i * D
    for d = 0, D - 1 do sc = sc + query[d] * db[o + d] end
    if cnt < K or sc > top_s[K - 1] then
      local p = cnt < K and cnt or K - 1
      while p > 0 and top_s[p - 1] < sc do
        top_s[p], top_i[p] = top_s[p - 1], top_i[p - 1]
        p = p - 1
      end
      top_s[p], top_i[p] = sc, i
      if cnt < K then cnt = cnt + 1 end
    end
  end
  for j = 0, K - 1 do
    fold(top_i[j])
    fold(top_s[j])
  end
end
print(string.format("CHECKSUM=%d", cs))
