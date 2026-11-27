-- 04_json_serde — LCG field checksum (parity with py/js/c/sr)
-- LuaJIT: FFI uint64 keeps LCG exact past 2^53. Plain Lua: double is exact below 2^31.
-- Avoid ULL suffixes so plain Lua can parse the file (harness prefers luajit).
local N = 10000
local lcg
if type(jit) == "table" then
  local ffi = require("ffi")
  local LCG_MOD = ffi.cast("uint64_t", 2147483648)
  local MUL = ffi.cast("uint64_t", 1103515245)
  local ADD = ffi.cast("uint64_t", 12345)
  local rng_st = ffi.new("uint64_t[1]", 4861952)
  lcg = function()
    local x = (rng_st[0] * MUL + ADD) % LCG_MOD
    rng_st[0] = x
    return tonumber(ffi.cast("uint32_t", x))
  end
else
  local rng = 4861952
  lcg = function()
    rng = (rng * 1103515245 + 12345) % 2147483648
    return rng
  end
end
local cs = 0
for _ = 1, N do
  local id = lcg() % 1000000
  local total = lcg() % 100000
  local items = lcg() % 8 + 1
  cs = cs + id + total + items * 17
  cs = cs + id + total + items * 17
end
print(string.format("CHECKSUM=%d", cs))
