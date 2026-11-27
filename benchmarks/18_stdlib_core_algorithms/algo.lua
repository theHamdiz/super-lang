-- 18_stdlib_core_algorithms — one checksum from four std-style kernels:
-- (A) KMP-DFA "regex" field extraction over 3000 generated log lines,
-- (B) strict UTF-8 validation/counting over 60 documents (some corrupted),
-- (C) SQL-ish filter / group-by / order-by-limit / join over 6000 rows x 24 queries,
-- (D) HM-style type unification (union-find, iterative) over 8504 type nodes.
local ffi = require("ffi")
local floor, band = math.floor, bit.band

local MOD = 1000000007
local NL, NDOC, PERDOC, NR, NQ = 3000, 60, 200, 6000, 24
local NVARS, NCOMP, NCON = 2500, 6000, 5000
local NTOT = NVARS + 4 + NCOMP

local rng = 18018
local function next_rand()
  rng = rng * 48271 % 2147483647
  return rng
end
local cs = 0
local function fold(v) cs = (cs * 131 + v) % MOD end
local function ints(n, init)
  local a = ffi.new("int32_t[?]", n)
  if init then for i = 0, n - 1 do a[i] = init end end
  return a
end

-- ---------- (A) log extraction ----------
local function build_dfa(needle)
  local n = #needle
  local dfa = ints(n * 256)
  dfa[needle:byte(1)] = 1
  local x = 0
  for j = 1, n - 1 do
    for c = 0, 255 do dfa[j * 256 + c] = dfa[x * 256 + c] end
    local nj = needle:byte(j + 1)
    dfa[j * 256 + nj] = j + 1
    x = dfa[x * 256 + nj]
  end
  return dfa
end

local function extract(line, e, dfa, n)
  local st, i = 0, 0
  while i < e do
    st = dfa[st * 256 + line[i]]
    i = i + 1
    if st == n then
      local v, nd = 0, 0
      while i < e and line[i] >= 48 and line[i] <= 57 do
        v = v * 10 + line[i] - 48
        nd = nd + 1
        i = i + 1
      end
      if nd > 0 then return v end
      st = 0
    end
  end
  return -1
end

local function part_a()
  local levels = { [0] = "INFO", "WARN", "ERROR", "DEBUG" }
  local codes = { [0] = 200, 201, 204, 301, 400, 401, 404, 500, 503 }
  local d_status, d_took, d_user = build_dfa("status="), build_dfa("took="), build_dfa("user=")
  local cls = {}
  for c = 0, 7 do cls[c] = 0 end
  local took_sum, took_max, took_n, user_sum, bad, total = 0, 0, 0, 0, 0, 0
  local line = ffi.new("uint8_t[160]")
  for i = 0, NL - 1 do
    local mo, dd, hh = 1 + next_rand() % 12, 1 + next_rand() % 28, next_rand() % 24
    local mi, ss, lv = next_rand() % 60, next_rand() % 60, next_rand() % 4
    local svc, uid, sidx, ms = next_rand() % 100, next_rand() % 100000, next_rand() % 9, next_rand() % 3000
    local s = string.format("2026-%02d-%02d %02d:%02d:%02d %s svc-%02d user=%d", mo, dd, hh, mi, ss, levels[lv], svc, uid)
    if i % 13 == 5 then s = s .. " status=abc" else s = s .. string.format(" status=%d", codes[sidx]) end
    if i % 17 ~= 3 then s = s .. string.format(" took=%dms", ms) end
    local e = #s
    for k = 1, e do line[k - 1] = s:byte(k) end
    local st, tk, us = extract(line, e, d_status, 7), extract(line, e, d_took, 5), extract(line, e, d_user, 5)
    if st < 0 then bad = bad + 1 else cls[floor(st / 100)] = cls[floor(st / 100)] + 1 end
    if tk >= 0 then
      took_sum, took_n = took_sum + tk, took_n + 1
      if tk > took_max then took_max = tk end
    end
    user_sum = user_sum + us
    total = total + e + 1
  end
  for c = 0, 7 do fold(cls[c]) end
  fold(took_sum); fold(took_max); fold(took_n); fold(user_sum); fold(bad); fold(total)
end

-- ---------- (B) UTF-8 ----------
local function cont(b) return band(b, 0xC0) == 0x80 end

local function utf8_valid(d, n)
  local i = 0
  while i < n do
    local b0 = d[i]
    if b0 < 128 then
      i = i + 1
    elseif b0 < 194 then
      return false
    elseif b0 < 224 then
      if i + 1 >= n or not cont(d[i + 1]) then return false end
      i = i + 2
    elseif b0 < 240 then
      if i + 2 >= n or not cont(d[i + 1]) or not cont(d[i + 2]) then return false end
      local cp = (b0 - 224) * 4096 + (d[i + 1] - 128) * 64 + (d[i + 2] - 128)
      if cp < 2048 or (cp >= 55296 and cp <= 57343) then return false end
      i = i + 3
    elseif b0 < 245 then
      if i + 3 >= n or not cont(d[i + 1]) or not cont(d[i + 2]) or not cont(d[i + 3]) then return false end
      local cp = (b0 - 240) * 262144 + (d[i + 1] - 128) * 4096 + (d[i + 2] - 128) * 64 + (d[i + 3] - 128)
      if cp < 65536 or cp > 1114111 then return false end
      i = i + 4
    else
      return false
    end
  end
  return true
end

local function utf8_count(d, n)
  local c = 0
  for i = 0, n - 1 do if not cont(d[i]) then c = c + 1 end end
  return c
end

local function encode(cp, o, at)
  if cp < 128 then o[at] = cp; return 1 end
  if cp < 2048 then o[at], o[at + 1] = 192 + floor(cp / 64), 128 + cp % 64; return 2 end
  if cp < 65536 then
    o[at], o[at + 1], o[at + 2] = 224 + floor(cp / 4096), 128 + floor(cp / 64) % 64, 128 + cp % 64
    return 3
  end
  o[at], o[at + 1] = 240 + floor(cp / 262144), 128 + floor(cp / 4096) % 64
  o[at + 2], o[at + 3] = 128 + floor(cp / 64) % 64, 128 + cp % 64
  return 4
end

local function part_b()
  local badb = { [0] = 0x80, 0xC0, 0xC1, 0xF5, 0xFF, 0xE0 }
  local doc = ints(PERDOC * 4)
  local valid_docs, total_cp = 0, 0
  for d = 0, NDOC - 1 do
    local n = 0
    for _ = 1, PERDOC do
      local cat, r = next_rand() % 4, next_rand()
      local cp
      if cat == 0 then cp = r % 128
      elseif cat == 1 then cp = 128 + r % 1920
      elseif cat == 2 then cp = 2048 + r % 53248
      else cp = 65536 + r % 1048576 end
      n = n + encode(cp, doc, n)
    end
    if d % 5 == 2 then
      local p, b = next_rand() % n, next_rand() % 6
      doc[p] = badb[b]
    end
    local ok = utf8_valid(doc, n) and 1 or 0
    local cnt = ok == 1 and utf8_count(doc, n) or 0
    fold(ok); fold(cnt); fold(n)
    valid_docs, total_cp = valid_docs + ok, total_cp + cnt
  end
  fold(valid_docs); fold(total_cp)
end

-- ---------- (C) SQL-ish ----------
local function part_c()
  local dept, age, sal, act = ints(NR), ints(NR), ints(NR), ints(NR)
  for i = 0, NR - 1 do
    dept[i] = next_rand() % 16
    age[i] = 18 + next_rand() % 50
    sal[i] = 30000 + next_rand() % 90000
    act[i] = (next_rand() % 10 ~= 0) and 1 or 0
  end
  for _ = 1, NQ do
    local age_lo, sal_max, need = 18 + next_rand() % 30, 40000 + next_rand() % 80000, next_rand() % 2
    local cnt, sum, mx, taken = {}, {}, {}, {}
    for g = 0, 15 do cnt[g], sum[g], mx[g], taken[g] = 0, 0, 0, false end
    for i = 0, NR - 1 do
      if age[i] >= age_lo and sal[i] < sal_max and act[i] >= need then
        local g = dept[i]
        cnt[g] = cnt[g] + 1
        sum[g] = sum[g] + sal[i]
        if age[i] > mx[g] then mx[g] = age[i] end
      end
    end
    for _ = 1, 3 do
      local best = -1
      for g = 0, 15 do
        if cnt[g] >= 5 and not taken[g] and (best < 0 or sum[g] > sum[best]) then best = g end
      end
      if best >= 0 then
        taken[best] = true
        fold(best); fold(cnt[best]); fold(sum[best]); fold(mx[best])
        fold(floor(sum[best] / ((best + 1) * 1000)))
      else
        fold(999)
      end
    end
  end
end

-- ---------- (D) HM unification ----------
local function part_d()
  local kind, na, nb, parent = ints(NTOT), ints(NTOT, -1), ints(NTOT, -1), ints(NTOT)
  for i = 0, NTOT - 1 do parent[i] = i end
  for c = 0, 3 do kind[NVARS + c], na[NVARS + c] = 1, c end
  for i = NVARS + 4, NTOT - 1 do
    kind[i], na[i], nb[i] = 2, next_rand() % i, next_rand() % i
  end
  local unions = 0
  local function find(x)
    while parent[x] ~= x do
      parent[x] = parent[parent[x]]
      x = parent[x]
    end
    return x
  end
  local stk_a, stk_b = ints(2 * NTOT + 16), ints(2 * NTOT + 16)
  local function unify(x, y)
    local sp = 0
    stk_a[sp], stk_b[sp] = x, y
    sp = sp + 1
    while sp > 0 do
      sp = sp - 1
      local rx, ry = find(stk_a[sp]), find(stk_b[sp])
      if rx ~= ry then
        local kx, ky = kind[rx], kind[ry]
        if kx == 0 then
          parent[rx] = ry
          unions = unions + 1
        elseif ky == 0 then
          parent[ry] = rx
          unions = unions + 1
        elseif kx == 1 or ky == 1 then
          if kx == 1 and ky == 1 and na[rx] == na[ry] then
            parent[ry] = rx
            unions = unions + 1
          else
            return false
          end
        else
          parent[ry] = rx
          unions = unions + 1
          stk_a[sp], stk_b[sp] = na[rx], na[ry]
          stk_a[sp + 1], stk_b[sp + 1] = nb[rx], nb[ry]
          sp = sp + 2
        end
      end
    end
    return true
  end
  local fails = 0
  for _ = 1, NCON do
    local t = next_rand() % 2
    local x
    if t == 0 then x = next_rand() % NVARS else x = next_rand() % NTOT end
    local y = next_rand() % NTOT
    if not unify(x, y) then fails = fails + 1 end
  end
  fold(fails); fold(unions)
  local classes = 0
  for i = 0, NTOT - 1 do
    local r = find(i)
    if r == i then classes = classes + 1 end
    fold(r * 3 + kind[r])
  end
  fold(classes)
end

part_a()
part_b()
part_c()
part_d()
print(string.format("CHECKSUM=%d", cs))
