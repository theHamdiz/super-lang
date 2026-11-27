-- 07_crc32 — IEEE CRC32 on expanded LCG-scrambled block (parity with py/js/sr/c)
-- Avoid ULL suffixes so plain Lua can parse; harness prefers luajit + FFI.
local bit = bit or bit32
if not bit then
  local ok, native = pcall(function()
    return {
      band = function(a, b) return a & b end,
      bxor = function(a, b) return a ~ b end,
      rshift = function(a, n) return a >> n end,
    }
  end)
  if ok then bit = native else error("need bit/bit32 library") end
end

local U32 = 4294967296
local function u32(x)
  x = x % U32
  if x < 0 then x = x + U32 end
  return x
end

local crc_table = {}
for i = 0, 255 do
  local c = i
  for _ = 1, 8 do
    if bit.band(c, 1) ~= 0 then
      c = bit.bxor(0xEDB88320, bit.rshift(c, 1))
    else
      c = bit.rshift(c, 1)
    end
    c = u32(c)
  end
  crc_table[i] = c
end

local function crc32(s)
  local crc = 0xffffffff
  for i = 1, #s do
    local b = string.byte(s, i)
    local idx = bit.band(bit.bxor(crc, b), 0xff)
    local shifted = math.floor(u32(crc) / 256)
    crc = u32(bit.bxor(crc_table[idx], shifted))
  end
  return u32(bit.bxor(crc, 0xffffffff))
end

local seed = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.."
local block = seed
for _ = 1, 6 do block = block .. block end

local acc = 0
if type(jit) == "table" then
  local ffi = require("ffi")
  local LCG_MOD = ffi.cast("uint64_t", 2147483648)
  local MUL = ffi.cast("uint64_t", 1103515245)
  local ADD = ffi.cast("uint64_t", 12345)
  local rng_st = ffi.new("uint32_t[1]", 8137778)
  for i = 0, 4095 do
    local prod = ffi.cast("uint64_t", rng_st[0]) * MUL + ADD
    local rng = tonumber(prod % LCG_MOD)
    rng_st[0] = rng
    local ch = string.byte(block, (i % #block) + 1)
    acc = acc + (rng % 255) + ch
  end
else
  local rng = 8137778
  for i = 0, 4095 do
    rng = (rng * 1103515245 + 12345) % 2147483648
    local ch = string.byte(block, (i % #block) + 1)
    acc = acc + (rng % 255) + ch
  end
end

local payload = string.rep(block, 9)
local c1 = crc32(payload)
local c2 = crc32(block)
local cs = c1 + c2 + (acc % 1000)
print(string.format("CHECKSUM=%d", cs))
