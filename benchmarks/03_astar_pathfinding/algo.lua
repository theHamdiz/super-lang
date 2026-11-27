-- 03_astar — 64×64 grid, 40 queries, binary min-heap (0-based cells)
local ffi = require("ffi")

local S, CELLS, QUERIES, INF = 64, 64 * 64, 40, 0x3fffffff
local DIRS = {{1, 0}, {-1, 0}, {0, 1}, {0, -1}}
local LCG_MOD = 2147483648ULL
local rng_st = ffi.new("uint32_t[1]", 10846167)

local function next_u()
  local prod = ffi.cast("uint64_t", rng_st[0]) * 1103515245ULL + 12345ULL
  local r = tonumber(prod % LCG_MOD)
  rng_st[0] = r
  return r
end

local walkable = ffi.new("int[?]", CELLS)
local gscore = ffi.new("int[?]", CELLS)
local came = ffi.new("int[?]", CELLS)
local openf = ffi.new("int[?]", CELLS)
local heap = ffi.new("int[?]", CELLS)
local hprio = ffi.new("int[?]", CELLS)
local hn = 0

local function heap_push(node, prio)
  heap[hn] = node
  hprio[hn] = prio
  local i = hn
  hn = hn + 1
  while i > 0 do
    local p = math.floor((i - 1) / 2)
    if hprio[i] >= hprio[p] then break end
    heap[i], heap[p] = heap[p], heap[i]
    hprio[i], hprio[p] = hprio[p], hprio[i]
    i = p
  end
end

local function heap_pop()
  local top = heap[0]
  hn = hn - 1
  heap[0] = heap[hn]
  hprio[0] = hprio[hn]
  local i = 0
  while true do
    local l = 2 * i + 1
    local r = l + 1
    local sm = i
    if l < hn and hprio[l] < hprio[sm] then sm = l end
    if r < hn and hprio[r] < hprio[sm] then sm = r end
    if sm == i then break end
    heap[i], heap[sm] = heap[sm], heap[i]
    hprio[i], hprio[sm] = hprio[sm], hprio[i]
    i = sm
  end
  return top
end

local function heur(a, b)
  local ax, ay = a % S, math.floor(a / S)
  local bx, by = b % S, math.floor(b / S)
  local dx = ax - bx
  if dx < 0 then dx = -dx end
  local dy = ay - by
  if dy < 0 then dy = -dy end
  return dx + dy
end

local function astar(start, goal)
  for i = 0, CELLS - 1 do
    gscore[i] = INF
    openf[i] = 0
    came[i] = -1
  end
  hn = 0
  gscore[start] = 0
  heap_push(start, heur(start, goal))
  openf[start] = 1
  while hn > 0 do
    local cur = heap_pop()
    openf[cur] = 0
    if cur == goal then
      local len = 0
      local c = goal
      while c ~= start and c >= 0 do
        len = len + 1
        c = came[c]
      end
      return len
    end
    local cx, cy = cur % S, math.floor(cur / S)
    for d = 1, 4 do
      local nx = cx + DIRS[d][1]
      local ny = cy + DIRS[d][2]
      if nx >= 0 and ny >= 0 and nx < S and ny < S then
        local ni = ny * S + nx
        if walkable[ni] ~= 0 then
          local tent = gscore[cur] + 1
          if tent < gscore[ni] then
            came[ni] = cur
            gscore[ni] = tent
            heap_push(ni, tent + heur(ni, goal))
            openf[ni] = 1
          end
        end
      end
    end
  end
  return -1
end

for i = 0, CELLS - 1 do walkable[i] = 1 end
for _ = 1, math.floor(CELLS / 5) do
  local cell = tonumber(next_u() % CELLS)
  if cell ~= 0 and cell ~= CELLS - 1 then walkable[cell] = 0 end
end

local cs = 0
for _ = 1, QUERIES do
  local a = tonumber(next_u() % CELLS)
  local b = tonumber(next_u() % CELLS)
  while walkable[a] == 0 do a = tonumber(next_u() % CELLS) end
  while walkable[b] == 0 do b = tonumber(next_u() % CELLS) end
  cs = cs + astar(a, b)
end
print(string.format("CHECKSUM=%d", cs))