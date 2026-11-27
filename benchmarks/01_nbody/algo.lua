-- 01_nbody — 64 bodies × 500 steps, soft gravity, SoA
local ffi = require("ffi")
local bit = bit

local N, STEPS, DT, SOFT = 64, 500, 0.001, 0.01
local LCG_MOD = 2147483648ULL
local rng_st = ffi.new("uint32_t[1]", 12648430)

local function next_u()
  local prod = ffi.cast("uint64_t", rng_st[0]) * 1103515245ULL + 12345ULL
  local r = tonumber(prod % LCG_MOD)
  rng_st[0] = r
  return r
end

local function next_f()
  return tonumber(next_u()) / 2147483648.0
end

local x, y, vx, vy, m = {}, {}, {}, {}, {}
for i = 1, N do
  x[i] = next_f() * 2.0 - 1.0
  y[i] = next_f() * 2.0 - 1.0
  vx[i] = (next_f() - 0.5) * 0.01
  vy[i] = (next_f() - 0.5) * 0.01
  m[i] = 0.5 + next_f()
end

for _ = 1, STEPS do
  for i = 1, N do
    local ax, ay = 0.0, 0.0
    for j = 1, N do
      if i ~= j then
        local dx, dy = x[j] - x[i], y[j] - y[i]
        local d2 = dx * dx + dy * dy + SOFT
        local inv = 1.0 / math.sqrt(d2)
        local inv3 = inv * inv * inv
        ax = ax + m[j] * dx * inv3
        ay = ay + m[j] * dy * inv3
      end
    end
    vx[i] = vx[i] + ax * DT
    vy[i] = vy[i] + ay * DT
  end
  for i = 1, N do
    x[i] = x[i] + vx[i] * DT
    y[i] = y[i] + vy[i] * DT
  end
end

local cs = ffi.new("int64_t", 0)
for i = 1, N do
  cs = cs + math.floor(x[i] * 1000.0)
  cs = cs + math.floor(y[i] * 1000.0)
  cs = cs + math.floor(vx[i] * 1e6)
end
print(string.format("CHECKSUM=%d", cs))