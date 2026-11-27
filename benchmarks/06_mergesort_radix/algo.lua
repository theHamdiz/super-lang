-- 06_mergesort_radix — sort 50000 u64, verify + checksum
local ffi = require("ffi")

local N = 50000
local LCG_MOD = 2147483648ULL
local rng_st = ffi.new("uint32_t[1]", 395799)

local function next_u()
  local prod = ffi.cast("uint64_t", rng_st[0]) * 1103515245ULL + 12345ULL
  local r = tonumber(prod % LCG_MOD)
  rng_st[0] = r
  return r
end

local a = ffi.new("uint64_t[?]", N)
local tmp = ffi.new("uint64_t[?]", N)
for i = 0, N - 1 do a[i] = next_u() end

local function merge(lo, mid, hi)
  local i, j, k = lo, mid, lo
  while i < mid and j < hi do
    if a[i] <= a[j] then tmp[k] = a[i]; i = i + 1
    else tmp[k] = a[j]; j = j + 1 end
    k = k + 1
  end
  while i < mid do tmp[k] = a[i]; i = i + 1; k = k + 1 end
  while j < hi do tmp[k] = a[j]; j = j + 1; k = k + 1 end
  for i = lo, hi - 1 do a[i] = tmp[i] end
end

local function mergesort(lo, hi)
  if hi - lo <= 1 then return end
  local mid = lo + math.floor((hi - lo) / 2)
  mergesort(lo, mid)
  mergesort(mid, hi)
  merge(lo, mid, hi)
end

mergesort(0, N)

local cs = ffi.new("int64_t", 0)
local ok = 1
for i = 0, N - 1 do
  local v = a[i]
  cs = cs ~ tonumber(v % 0x100000000ULL)
  cs = cs * 131 + tonumber(v >> 32)
  if i > 0 and a[i] < a[i - 1] then ok = 0 end
end
local out = cs ~ (ok == 1 and 1 or 0)
print(string.format("CHECKSUM=%d", out))