-- 15_physics2d_broadphase_solver - 6k circles/boxes: uniform spatial-hash broadphase,
-- circle/box manifolds, sequential-impulse solver (4 iterations x 10 steps) (LuaJIT)
-- Mirrors std.game.physics2d (grid build, candidate order, narrowphase, resolve).
local bit = require("bit")
local bxor, band = bit.bxor, bit.band
local floor, sqrt, abs, tsort = math.floor, math.sqrt, math.abs, table.sort

local COLS, ROWS = 100, 60
local N_DYN = COLS * ROWS
local FLOOR_TILES, WALL_TILES = 62, 40
local STEPS, ITERS = 10, 4
local MOD = 1000000007

-- (rng * 1103515245 + 12345) mod 2^31, exact in doubles via a 16-bit split of rng.
local rng = 150015
local function draw(m)
  local al = rng % 65536
  local ah = (rng - al) / 65536
  local hi = (ah * 1103515245) % 32768
  rng = (hi * 65536 + al * 1103515245 + 12345) % 2147483648
  return floor(rng / 256) % m
end

-- parallel 0-based arrays; shape 0 = circle, 1 = box; e = 0.2, mu = 0.4 for every body
local X, Y, VX, VY, IM, SH, R, HW, HH = {}, {}, {}, {}, {}, {}, {}, {}, {}
local n = 0

local function add(x, y, vx, vy, mass, shape, r, hw, hh)
  X[n], Y[n], VX[n], VY[n] = x, y, vx, vy
  IM[n] = mass <= 0.0 and 0.0 or 1.0 / mass
  SH[n], R[n], HW[n], HH[n] = shape, r, hw, hh
  n = n + 1
end

local function sign(v) return v < 0.0 and -1.0 or 1.0 end
local function clamp(v, lo, hi) if v < lo then return lo elseif v > hi then return hi end return v end

-- narrowphase: returns hit, nx, ny, depth
local function circle_circle(a, b)
  local dx, dy = X[b] - X[a], Y[b] - Y[a]
  local rr = R[a] + R[b]
  local d2 = dx * dx + dy * dy
  if d2 >= rr * rr then return false end
  if d2 == 0.0 then return true, 1.0, 0.0, rr end
  local d = sqrt(d2)
  return true, dx / d, dy / d, rr - d
end

local function box_box(a, b)
  local dx, dy = X[b] - X[a], Y[b] - Y[a]
  local ox = HW[a] + HW[b] - abs(dx)
  local oy = HH[a] + HH[b] - abs(dy)
  if ox <= 0.0 or oy <= 0.0 then return false end
  if ox < oy then return true, sign(dx), 0.0, ox end
  return true, 0.0, sign(dy), oy
end

local function circle_box(a, b)
  local lx, ly = X[a] - X[b], Y[a] - Y[b]
  local cx = clamp(lx, 0.0 - HW[b], HW[b])
  local cy = clamp(ly, 0.0 - HH[b], HH[b])
  local fx, fy = lx - cx, ly - cy
  local d2 = fx * fx + fy * fy
  if d2 > 0.0 then
    if d2 >= R[a] * R[a] then return false end
    local d = sqrt(d2)
    return true, (0.0 - fx) / d, (0.0 - fy) / d, R[a] - d
  end
  local px, py = HW[b] - abs(lx), HH[b] - abs(ly)
  if px < py then return true, 0.0 - sign(lx), 0.0, px + R[a] end
  return true, 0.0, 0.0 - sign(ly), py + R[a]
end

local function collide(a, b)
  local sa, sb = SH[a], SH[b]
  if sa == 0 and sb == 0 then return circle_circle(a, b) end
  if sa == 1 and sb == 1 then return box_box(a, b) end
  if sa == 0 and sb == 1 then return circle_box(a, b) end
  local hit, nx, ny, depth = circle_box(b, a)
  if not hit then return false end
  return true, 0.0 - nx, 0.0 - ny, depth
end

local function resolve(a, b, nx, ny, depth)
  local ia, ib = IM[a], IM[b]
  local isum = ia + ib
  if isum == 0.0 then return end
  local rvx, rvy = VX[b] - VX[a], VY[b] - VY[a]
  local vn = rvx * nx + rvy * ny
  if vn < 0.0 then
    local e = 0.2
    local j = (0.0 - (1.0 + e)) * vn / isum
    VX[a] = VX[a] - j * ia * nx
    VY[a] = VY[a] - j * ia * ny
    VX[b] = VX[b] + j * ib * nx
    VY[b] = VY[b] + j * ib * ny
    local tx, ty = rvx - vn * nx, rvy - vn * ny
    local tl = sqrt(tx * tx + ty * ty)
    if tl > 0.000001 then
      tx = tx / tl
      ty = ty / tl
      local jt = (0.0 - (rvx * tx + rvy * ty)) / isum
      local mu = sqrt(0.4 * 0.4)
      local lim = mu * j
      jt = clamp(jt, 0.0 - lim, lim)
      VX[a] = VX[a] - jt * ia * tx
      VY[a] = VY[a] - jt * ia * ty
      VX[b] = VX[b] + jt * ib * tx
      VY[b] = VY[b] + jt * ib * ty
    end
  end
  local corr = math.max(depth - 0.01, 0.0) / isum * 0.8
  X[a] = X[a] - corr * ia * nx
  Y[a] = Y[a] - corr * ia * ny
  X[b] = X[b] + corr * ib * nx
  Y[b] = Y[b] + corr * ib * ny
end

local function bucket(cx, cy, mask) return band(bxor(cx * 73856093, cy * 19349663), mask) end

local function solve_grid()
  local s, big = 0.0, 0.0
  for i = 0, n - 1 do
    local e = math.max(HW[i], HH[i])
    s = s + e
    if e > big then big = e end
  end
  local cell = math.max(2.0 * s / n, big / 8.0)
  if cell < 0.000001 then cell = 1.0 end
  local inv = 1.0 / cell
  local pad = cell * 0.1
  local size = 16
  while size < n * 2 do size = size * 2 end
  local mask = size - 1
  local heads = {}
  for k = 0, size - 1 do heads[k] = -1 end
  local eb, ex, ey, en, elen = {}, {}, {}, {}, 0
  for i = 0, n - 1 do
    local cx0, cx1 = floor((X[i] - HW[i] - pad) * inv), floor((X[i] + HW[i] + pad) * inv)
    local cy0, cy1 = floor((Y[i] - HH[i] - pad) * inv), floor((Y[i] + HH[i] + pad) * inv)
    for cy = cy0, cy1 do
      for cx = cx0, cx1 do
        local slot = bucket(cx, cy, mask)
        en[elen] = heads[slot]
        heads[slot] = elen
        eb[elen], ex[elen], ey[elen] = i, cx, cy
        elen = elen + 1
      end
    end
  end
  local mark = {}
  for k = 0, n - 1 do mark[k] = 0 end
  local qpad = 0.1 / inv
  local last = 0
  local cands = {}
  for i = 0, n - 1 do
    local cx0, cx1 = floor((X[i] - HW[i] - qpad) * inv), floor((X[i] + HW[i] + qpad) * inv)
    local cy0, cy1 = floor((Y[i] - HH[i] - qpad) * inv), floor((Y[i] + HH[i] + qpad) * inv)
    local cnt = 0
    for cy = cy0, cy1 do
      for cx = cx0, cx1 do
        local e = heads[bucket(cx, cy, mask)]
        while e >= 0 do
          if ex[e] == cx and ey[e] == cy then
            local j = eb[e]
            if j > i and mark[j] ~= i + 1 then
              mark[j] = i + 1
              cnt = cnt + 1
              cands[cnt] = j
            end
          end
          e = en[e]
        end
      end
    end
    for k = #cands, cnt + 1, -1 do cands[k] = nil end
    tsort(cands)
    for k = 1, cnt do
      local j = cands[k]
      if IM[i] + IM[j] > 0.0 and not (abs(X[j] - X[i]) >= HW[i] + HW[j]) and not (abs(Y[j] - Y[i]) >= HH[i] + HH[j]) then
        local hit, nx, ny, depth = collide(i, j)
        if hit then
          resolve(i, j, nx, ny, depth)
          last = last + 1
        end
      end
    end
  end
  return last
end

local function posmod(v) return ((v % MOD) + MOD) % MOD end

for k = 0, FLOOR_TILES - 1 do add(1.0 + 2.0 * k, -1.0, 0.0, 0.0, 0.0, 1, 0.0, 1.0, 1.0) end
for k = 0, WALL_TILES - 1 do add(-1.0, 1.0 + 2.0 * k, 0.0, 0.0, 0.0, 1, 0.0, 1.0, 1.0) end
for k = 0, WALL_TILES - 1 do add(122.0, 1.0 + 2.0 * k, 0.0, 0.0, 0.0, 1, 0.0, 1.0, 1.0) end
for i = 0, N_DYN - 1 do
  local col, row = i % COLS, floor(i / COLS)
  local kind = draw(3)
  local sw = 0.35 + draw(400001) * 0.000001
  local sh = 0.35 + draw(400001) * 0.000001
  local mass = 1.0 + draw(4)
  local jx = (draw(600001) - 300000) * 0.000001
  local jy = (draw(600001) - 300000) * 0.000001
  local vx = (draw(8000001) - 4000000) * 0.000001
  local vy = (draw(8000001) - 4000000) * 0.000001
  local x = 0.75 + col * 1.2 + jx
  local y = 1.75 + row * 1.2 + jy
  if kind == 0 then add(x, y, vx, vy, mass, 1, 0.0, sw, sh)
  else add(x, y, vx, vy, mass, 0, sw, sw, sw) end
end

local cs = 0
local dt = 1.0 / 60.0
local damp = 1.0 / (1.0 + dt * 0.0)
for s = 0, STEPS - 1 do
  for i = 0, n - 1 do
    if IM[i] > 0.0 then
      VX[i] = (VX[i] + 0.0 * dt) * damp
      VY[i] = (VY[i] + -9.81 * dt) * damp
      X[i] = X[i] + VX[i] * dt
      Y[i] = Y[i] + VY[i] * dt
    end
  end
  local last = 0
  for _ = 1, ITERS do last = solve_grid() end
  cs = cs + last * (s + 1)
end
local h, ke = 0, 0.0
for i = 0, n - 1 do
  h = (h * 131 + posmod(floor(X[i] * 1000.0))) % MOD
  h = (h * 131 + posmod(floor(Y[i] * 1000.0))) % MOD
  h = (h * 131 + posmod(floor(VX[i] * 1000.0))) % MOD
  h = (h * 131 + posmod(floor(VY[i] * 1000.0))) % MOD
  if IM[i] > 0.0 then ke = ke + 0.5 * (VX[i] * VX[i] + VY[i] * VY[i]) / IM[i] end
end
cs = cs + h + floor(ke * 10.0)
print(string.format("CHECKSUM=%d", cs))
