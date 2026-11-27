-- 13_polars_data_pipeline — 64k orders: filter -> inner join -> group_by aggregates -> rolling-mean window
local floor = math.floor
local ROWS, CUSTOMERS, KNOWN, REGIONS = 64000, 2000, 1800, 16

-- (rng * 1103515245 + 12345) mod 2^31, exact in doubles via a 16-bit split of rng.
local rng = 130013
local function lcg()
  local al = rng % 65536
  local ah = (rng - al) / 65536
  local hi = (ah * 1103515245) % 32768
  rng = (hi * 65536 + al * 1103515245 + 12345) % 2147483648
  return rng
end

local cust, qty, amtq, amt = {}, {}, {}, {}
for i = 1, ROWS do
  local c = floor(lcg() / 256) % CUSTOMERS
  local q = 1 + floor(lcg() / 256) % 9
  local p = floor(lcg() / 256) % 400
  cust[i], qty[i], amtq[i], amt[i] = c, q, q * p, q * p * 0.25
end
-- dimension table: direct-address hash on customer id
local region_of = {}
for c = 0, CUSTOMERS - 1 do region_of[c] = -1 end
for c = 0, KNOWN - 1 do region_of[c] = floor(lcg() / 256) % 16 end

local big, joined, roll_sum, roll_cnt = 0, 0, 0, 0
local tot, n, mx, sum_amt, seen, ring = {}, {}, {}, {}, {}, {}
for r = 0, REGIONS - 1 do
  tot[r], n[r], mx[r], sum_amt[r], seen[r] = 0, 0, 0, 0.0, 0
  ring[r] = {[0] = 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0}
end
for i = 1, ROWS do
  if amt[i] >= 20.0 and qty[i] > 1 then
    big = big + 1
    local r = region_of[cust[i]]
    if r >= 0 then
      joined = joined + 1
      tot[r] = tot[r] + amtq[i]
      n[r] = n[r] + 1
      sum_amt[r] = sum_amt[r] + amt[i]
      if n[r] == 1 or amtq[i] > mx[r] then mx[r] = amtq[i] end
      ring[r][seen[r] % 8] = amt[i]
      seen[r] = seen[r] + 1
      if seen[r] >= 8 then
        local s = 0.0
        for k = 0, 7 do s = s + ring[r][(seen[r] - 8 + k) % 8] end
        roll_sum = roll_sum + floor(s / 8.0 * 1024.0)
        roll_cnt = roll_cnt + 1
      end
    end
  end
end
local cs = big + joined * 3
for r = 0, REGIONS - 1 do
  if n[r] ~= 0 then
    local avg = sum_amt[r] / n[r]
    cs = cs + (r + 1) * (tot[r] + 7 * n[r] + floor(avg * 1024.0) + 13 * mx[r])
  end
end
cs = cs + roll_sum + 5 * roll_cnt
print(string.format("CHECKSUM=%d", cs))
