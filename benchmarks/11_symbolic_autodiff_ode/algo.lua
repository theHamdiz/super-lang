-- 11_symbolic_autodiff_ode — 20k random expressions: simplify + forward dual AD,
-- then dual-RK4 sensitivities of a stiff-ish ODE
local floor, sin, cos, abs = math.floor, math.sin, math.cos, math.abs
local OP_CONST, OP_VAR, OP_ADD, OP_SUB, OP_MUL, OP_NEG, OP_SIN = 0, 1, 2, 3, 4, 5, 6
local EXPRS, POINTS, TRAJ, STEPS = 20000, 7, 1024, 1000
local LEAF = {[0] = 0.0, 1.0, 2.0, 0.5, -1.0}

-- (rng * 1103515245 + 12345) mod 2^31, exact in doubles via a 16-bit split of rng.
local rng = 20240611
local function lcg()
  local al = rng % 65536
  local ah = (rng - al) / 65536
  local hi = (ah * 1103515245) % 32768
  rng = (hi * 65536 + al * 1103515245 + 12345) % 2147483648
  return rng
end

local function new_tree() return {op = {}, a = {}, b = {}, cv = {}, cnt = 0} end

local function gen(t, depth)
  local k = floor(lcg() / 256) % 10
  if k < 2 or depth == 0 then
    local m = floor(lcg() / 256) % 8
    local n = t.cnt
    t.cnt = n + 1
    if m < 3 then t.op[n], t.a[n], t.b[n], t.cv[n] = OP_VAR, 0, 0, 0.0
    else t.op[n], t.a[n], t.b[n], t.cv[n] = OP_CONST, 0, 0, LEAF[m - 3] end
    return n
  end
  local a = gen(t, depth - 1)
  local kind = OP_SIN
  if k == 8 then kind = OP_NEG end
  if k >= 5 and k < 8 then kind = OP_MUL end
  if k == 4 then kind = OP_SUB end
  if k < 4 then kind = OP_ADD end
  local b = 0
  if kind == OP_ADD or kind == OP_SUB or kind == OP_MUL then b = gen(t, depth - 1) end
  local n = t.cnt
  t.cnt = n + 1
  t.op[n], t.a[n], t.b[n], t.cv[n] = kind, a, b, 0.0
  return n
end

local function is_const(s, i, c) return s.op[i] == OP_CONST and s.cv[i] == c end

local function push(s, kind, a, b, c)
  local n = s.cnt
  s.cnt = n + 1
  s.op[n], s.a[n], s.b[n], s.cv[n] = kind, a, b, c
  return n
end

local function simp_node(s, kind, cv, l, r)
  local sop, scv = s.op, s.cv
  if kind == OP_CONST then return push(s, OP_CONST, 0, 0, cv) end
  if kind == OP_VAR then return push(s, OP_VAR, 0, 0, 0.0) end
  if kind == OP_ADD then
    if is_const(s, l, 0.0) then return r end
    if is_const(s, r, 0.0) then return l end
    if sop[l] == OP_CONST and sop[r] == OP_CONST then return push(s, OP_CONST, 0, 0, scv[l] + scv[r]) end
    return push(s, OP_ADD, l, r, 0.0)
  elseif kind == OP_SUB then
    if is_const(s, r, 0.0) then return l end
    if sop[l] == OP_CONST and sop[r] == OP_CONST then return push(s, OP_CONST, 0, 0, scv[l] - scv[r]) end
    if l == r then return push(s, OP_CONST, 0, 0, 0.0) end
    return push(s, OP_SUB, l, r, 0.0)
  elseif kind == OP_MUL then
    if is_const(s, l, 0.0) or is_const(s, r, 0.0) then return push(s, OP_CONST, 0, 0, 0.0) end
    if is_const(s, l, 1.0) then return r end
    if is_const(s, r, 1.0) then return l end
    if sop[l] == OP_CONST and sop[r] == OP_CONST then return push(s, OP_CONST, 0, 0, scv[l] * scv[r]) end
    return push(s, OP_MUL, l, r, 0.0)
  elseif kind == OP_NEG then
    if sop[l] == OP_CONST then return push(s, OP_CONST, 0, 0, -scv[l]) end
    if sop[l] == OP_NEG then return s.a[l] end
    return push(s, OP_NEG, l, 0, 0.0)
  end
  if is_const(s, l, 0.0) then return push(s, OP_CONST, 0, 0, 0.0) end
  return push(s, OP_SIN, l, 0, 0.0)
end

local function simplify(t, s, rep)
  s.cnt = 0
  local op, a, b, cv = t.op, t.a, t.b, t.cv
  for i = 0, t.cnt - 1 do
    rep[i] = simp_node(s, op[i], cv[i], rep[a[i]], rep[b[i]])
  end
end

local function eval_dual(t, x, ev, ed)
  local op, ta, tb, cv = t.op, t.a, t.b, t.cv
  for i = 0, t.cnt - 1 do
    local k, ia, ib = op[i], ta[i], tb[i]
    local rv, rd
    if k == OP_CONST then rv, rd = cv[i], 0.0
    elseif k == OP_VAR then rv, rd = x, 1.0
    elseif k == OP_ADD then rv, rd = ev[ia] + ev[ib], ed[ia] + ed[ib]
    elseif k == OP_SUB then rv, rd = ev[ia] - ev[ib], ed[ia] - ed[ib]
    elseif k == OP_MUL then rv, rd = ev[ia] * ev[ib], ev[ia] * ed[ib] + ed[ia] * ev[ib]
    elseif k == OP_NEG then rv, rd = -ev[ia], -ed[ia]
    else rv, rd = sin(ev[ia]), cos(ev[ia]) * ed[ia] end
    ev[i], ed[i] = rv, rd
  end
end

local function rk4_final(lam, y0, h, steps)
  local yv, yd = y0, 0.0
  local hh, h6 = 0.5 * h, h / 6.0
  local function rhs(t, v, d)
    local s, c = sin(t), cos(t)
    local dv = v - s
    local pv = lam * dv
    local pd = lam * d + 1.0 * dv
    return -pv + 0.5 * c, -pd
  end
  for k = 0, steps - 1 do
    local t = k * h
    local k1v, k1d = rhs(t, yv, yd)
    local k2v, k2d = rhs(t + hh, yv + k1v * hh, yd + k1d * hh)
    local k3v, k3d = rhs(t + hh, yv + k2v * hh, yd + k2d * hh)
    local k4v, k4d = rhs(t + h, yv + k3v * h, yd + k3d * h)
    yv = yv + (((k1v + k2v * 2.0) + k3v * 2.0) + k4v) * h6
    yd = yd + (((k1d + k2d * 2.0) + k3d * 2.0) + k4d) * h6
  end
  return yv, yd
end

local t, s = new_tree(), new_tree()
local rep, ev, ed, sev, sed = {}, {}, {}, {}, {}
for i = 0, 63 do rep[i], ev[i], ed[i], sev[i], sed[i] = 0, 0.0, 0.0, 0.0, 0.0 end
local orig_nodes, simp_nodes, mismatch, acc_v, acc_d = 0, 0, 0, 0.0, 0.0
for _ = 1, EXPRS do
  t.cnt = 0
  gen(t, 4)
  local n = t.cnt
  simplify(t, s, rep)
  local m = s.cnt
  orig_nodes = orig_nodes + n
  simp_nodes = simp_nodes + m
  for p = 0, POINTS - 1 do
    local x = (p - 3) / 4.0
    eval_dual(t, x, ev, ed)
    eval_dual(s, x, sev, sed)
    local vo, dro, vs, ds = ev[n - 1], ed[n - 1], sev[rep[n - 1]], sed[rep[n - 1]]
    if abs(vo - vs) > 0.000000001 * (1.0 + abs(vo)) then mismatch = mismatch + 1 end
    if abs(dro - ds) > 0.000000001 * (1.0 + abs(dro)) then mismatch = mismatch + 1 end
    acc_v = acc_v + vs
    acc_d = acc_d + ds
  end
end

local sum_y, sum_dy = 0.0, 0.0
for j = 0, TRAJ - 1 do
  local yv, yd = rk4_final(10.0 + j * 0.25, 1.0, 0.002, STEPS)
  sum_y = sum_y + yv
  sum_dy = sum_dy + yd
end

local q = floor(acc_v * 10000.0)
local qd = floor(acc_d * 10000.0)
local y1 = floor(sum_y * 1000000.0)
local y2 = floor(sum_dy * 1000000000.0)
local cs = orig_nodes + simp_nodes * 7 + mismatch * 13 + q * 3 + qd * 5 + y1 * 11 + y2 * 17
print(string.format("CHECKSUM=%d", cs))
