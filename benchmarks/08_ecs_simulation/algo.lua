-- 08_ecs — 2000 entities × 20 ticks, spatial grid neighbor queries
local ffi = require("ffi")
local bit = bit

local N, TICKS, CELL, GRID = 2000, 20, 10, 64
local GRID_CELLS = GRID * GRID
local LCG_MOD = 2147483648ULL
local rng_st = ffi.new("uint32_t[1]", 15482888)

local function next_u()
  local prod = ffi.cast("uint64_t", rng_st[0]) * 1103515245ULL + 12345ULL
  local r = tonumber(prod % LCG_MOD)
  rng_st[0] = r
  return r
end

local function next_f()
  return tonumber(next_u()) / 2147483648.0
end

local function f32(x)
  return x
end

local px, py, vx, vy, hp = {}, {}, {}, {}, {}
for i = 1, N do
  px[i] = f32(next_f() * 600.0)
  py[i] = f32(next_f() * 600.0)
  vx[i] = f32((next_f() - 0.5) * 2.0)
  vy[i] = f32((next_f() - 0.5) * 2.0)
  hp[i] = 10 + tonumber(next_u() % 20ULL)
end

local heads, nxt = {}, {}
for _ = 1, TICKS do
  for i = 1, N do
    px[i] = f32(px[i] + vx[i])
    py[i] = f32(py[i] + vy[i])
    if px[i] < 0 or px[i] > 600 then vx[i] = f32(-vx[i]) end
    if py[i] < 0 or py[i] > 600 then vy[i] = f32(-vy[i]) end
  end
  for c = 1, GRID_CELLS do heads[c] = -1 end
  for i = 1, N do
    local cx = math.floor(px[i] / CELL)
    local cy = math.floor(py[i] / CELL)
    if cx < 0 then cx = 0 end
    if cy < 0 then cy = 0 end
    if cx >= GRID then cx = GRID - 1 end
    if cy >= GRID then cy = GRID - 1 end
    local cell = cy * GRID + cx + 1
    nxt[i] = heads[cell]
    heads[cell] = i
  end
  for i = 1, N do
    local cx = math.floor(px[i] / CELL)
    local cy = math.floor(py[i] / CELL)
    if cx < 0 then cx = 0 end
    if cy < 0 then cy = 0 end
    if cx >= GRID then cx = GRID - 1 end
    if cy >= GRID then cy = GRID - 1 end
    local hits = 0
    for oy = -1, 1 do
      for ox = -1, 1 do
        local nx, ny = cx + ox, cy + oy
        if nx >= 0 and ny >= 0 and nx < GRID and ny < GRID then
          local id = heads[ny * GRID + nx + 1]
          while id >= 1 do
            if id ~= i then
              local dx = f32(px[i] - px[id])
              local dy = f32(py[i] - py[id])
              if f32(dx * dx + dy * dy) < 4.0 then hits = hits + 1 end
            end
            id = nxt[id]
          end
        end
      end
    end
    if hits > 3 then hp[i] = hp[i] - 1 end
  end
end

local cs = ffi.new("int64_t", 0)
for i = 1, N do
  cs = cs + hp[i]
  cs = cs + math.floor(f32(px[i] * 10.0))
  cs = cs + math.floor(f32(py[i] * 10.0))
end
print(string.format("CHECKSUM=%d", cs))