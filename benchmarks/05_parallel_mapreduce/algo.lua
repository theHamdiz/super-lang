-- 05_parallel_mapreduce — 1 MiB corpus, word + 3-gram frequencies
local ffi = require("ffi")
local bit = bit

local CORPUS = 65536
local BUCKETS = 65536
local WORDS = {"alpha","bravo","charlie","delta","echo","foxtrot","golf","hotel","india","juliet"}

local function mul32(a, b)
  return tonumber(ffi.cast("uint32_t", ffi.cast("uint32_t", a) * ffi.cast("uint32_t", b)))
end

local function hash_bytes(text, start, n)
  local h = 2166136261
  for i = 0, n - 1 do
    h = bit.bxor(h, text[start + i])
    h = mul32(h, 16777619)
  end
  return h
end

local function new_tab()
  local t = {}
  for i = 1, BUCKETS do t[i] = {0, 0} end
  return t
end

local function bump(tab, key, add)
  local i = bit.band(key, BUCKETS - 1) + 1
  for probe = 0, BUCKETS - 1 do
    local j = bit.band(i - 1 + probe, BUCKETS - 1) + 1
    local k, c = tab[j][1], tab[j][2]
    if c == 0 then tab[j] = {key, add}; return end
    if k == key then tab[j] = {key, c + add}; return end
  end
end

local function worker(text, begin0, end0)
  local tab = new_tab()
  local i = begin0
  while i < end0 do
    while i < end0 do
      local c = text[i]
      if c >= 97 and c <= 122 then break end
      i = i + 1
    end
    local start = i
    while i < end0 do
      local c = text[i]
      if c < 97 or c > 122 then break end
      i = i + 1
    end
    if i > start then
      local len = i - start
      bump(tab, hash_bytes(text, start, len), 1)
      if len >= 3 then bump(tab, bit.bxor(hash_bytes(text, start, 3), 0xC0FFEE), 1) end
    end
  end
  return tab
end

local function scan_corpus(text)
  local merged = new_tab()
  local i = 0
  while i < CORPUS do
    while i < CORPUS do
      local c = text[i]
      if c >= 97 and c <= 122 then break end
      i = i + 1
    end
    local start = i
    while i < CORPUS do
      local c = text[i]
      if c < 97 or c > 122 then break end
      i = i + 1
    end
    if i > start then
      local len = i - start
      bump(merged, hash_bytes(text, start, len), 1)
      if len >= 3 then bump(merged, bit.bxor(hash_bytes(text, start, 3), 0xC0FFEE), 1) end
    end
  end
  return merged
end

local function u32(x)
  return tonumber(ffi.cast("uint32_t", x))
end

local function stats(merged)
  local total, best, best_key = 0, 0, 0
  for j = 1, BUCKETS do
    local key, count = merged[j][1], merged[j][2]
    if count ~= 0 then
      total = total + count
      local uk = u32(key)
      if count > best or (count == best and uk < best_key) then
        best = count
        best_key = uk
      end
    end
  end
  return total, best, best_key
end

local text = ffi.new("uint8_t[?]", CORPUS)
local LCG_MOD = 2147483648ULL
local rng_st = ffi.new("uint32_t[1]", 370945)
local off = 0
while off + 16 < CORPUS do
  local prod = ffi.cast("uint64_t", rng_st[0]) * 1103515245ULL + 12345ULL
  local rng = tonumber(prod % LCG_MOD)
  rng_st[0] = rng
  local w = WORDS[(rng % 10) + 1]
  for k = 1, #w do text[off] = string.byte(w, k); off = off + 1 end
  text[off] = 32; off = off + 1
end
while off < CORPUS do text[off] = string.byte("x"); off = off + 1 end

local nworkers = 4
local chunk = math.floor(CORPUS / nworkers)
for wid = 0, nworkers - 1 do
  local begin0 = wid * chunk
  local end0 = (wid == nworkers - 1) and CORPUS or (begin0 + chunk)
  worker(text, begin0, end0)
end

local merged = scan_corpus(text)
local total, best, best_key = stats(merged)
local cs = tonumber(ffi.cast("int64_t",
  ffi.cast("uint64_t", total)
  ~ (ffi.cast("uint64_t", best) * 1315423911ULL)
  ~ ffi.cast("uint64_t", best_key)))
print(string.format("CHECKSUM=%d", cs))
