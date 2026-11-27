-- 12_sparse_fem_fft — 128x128 reaction-Poisson CSR, Jacobi-PCG x20, 2D FFT low-pass bank
local floor, sqrt, cos, sin, pi = math.floor, math.sqrt, math.cos, math.sin, math.pi
local G, N, ITERS = 128, 16384, 20

-- 0-based CSR arrays stored in 1-based Lua tables (index + 1)
local indptr, indices, values, diag = {}, {}, {}, {}
local function build_system()
  local nz = 0
  indptr[1] = 0
  for i = 0, N - 1 do
    local row, col = floor(i / G), i % G
    local d = 4.0 + ((i * 37) % 11) * 0.25
    diag[i + 1] = d
    if row > 0 then nz = nz + 1; indices[nz] = i - G; values[nz] = -1.0 end
    if col > 0 then nz = nz + 1; indices[nz] = i - 1; values[nz] = -1.0 end
    nz = nz + 1; indices[nz] = i; values[nz] = d
    if col < G - 1 then nz = nz + 1; indices[nz] = i + 1; values[nz] = -1.0 end
    if row < G - 1 then nz = nz + 1; indices[nz] = i + G; values[nz] = -1.0 end
    indptr[i + 2] = nz
  end
end

local function matvec(x, out)
  for r = 0, N - 1 do
    local s = 0.0
    for k = indptr[r + 1] + 1, indptr[r + 2] do s = s + values[k] * x[indices[k] + 1] end
    out[r + 1] = s
  end
end

local function dot(a, b)
  local s = 0.0
  for i = 1, N do s = s + a[i] * b[i] end
  return s
end

local function pcg(b, x)
  local r, z, p, ap = {}, {}, {}, {}
  for i = 1, N do x[i] = 0.0; r[i] = b[i]; z[i] = r[i] / diag[i]; p[i] = z[i]; ap[i] = 0.0 end
  local rz = dot(r, z)
  for _ = 1, ITERS do
    matvec(p, ap)
    local alpha = rz / dot(p, ap)
    for i = 1, N do
      x[i] = x[i] + alpha * p[i]
      r[i] = r[i] - alpha * ap[i]
      z[i] = r[i] / diag[i]
    end
    local rz_new = dot(r, z)
    local beta = rz_new / rz
    for i = 1, N do p[i] = z[i] + beta * p[i] end
    rz = rz_new
  end
  return dot(r, r)
end

local BITREV = {}
for i = 0, G - 1 do
  local r, v = 0, i
  for _ = 1, 7 do r = r * 2 + (v % 2); v = floor(v / 2) end
  BITREV[i + 1] = r + 1
end

-- twiddles per stage, exactly cos/sin(ang * k) as std.math.fft
local TWR, TWI = {}, {}
do
  local span, lvl = 2, 1
  while span <= G do
    local ang = 0.0 - 2.0 * pi / span
    TWR[lvl], TWI[lvl] = {}, {}
    for k = 0, span / 2 - 1 do
      TWR[lvl][k + 1] = cos(ang * k)
      TWI[lvl][k + 1] = sin(ang * k)
    end
    span = span * 2
    lvl = lvl + 1
  end
end

local tre, tim = {}, {}
local function fft1(re, im, inv)
  if inv then for i = 1, G do im[i] = 0.0 - im[i] end end
  for i = 1, G do local s = BITREV[i]; tre[i] = re[s]; tim[i] = im[s] end
  local span, lvl = 2, 1
  while span <= G do
    local half = span / 2
    local wr_t, wi_t = TWR[lvl], TWI[lvl]
    for start = 0, G - 1, span do
      for k = 0, half - 1 do
        local wr, wi = wr_t[k + 1], wi_t[k + 1]
        local a = start + k + 1
        local b = a + half
        local tr = tre[b] * wr - tim[b] * wi
        local ti = tre[b] * wi + tim[b] * wr
        tre[b] = tre[a] - tr; tim[b] = tim[a] - ti
        tre[a] = tre[a] + tr; tim[a] = tim[a] + ti
      end
    end
    span = span * 2
    lvl = lvl + 1
  end
  if inv then
    local s = 1.0 / G
    for i = 1, G do re[i] = tre[i] * s; im[i] = (0.0 - tim[i]) * s end
  else
    for i = 1, G do re[i] = tre[i]; im[i] = tim[i] end
  end
end

local function fft2(re, im, inv)
  local lr, li = {}, {}
  for r = 0, G - 1 do
    for c = 1, G do lr[c] = re[r * G + c]; li[c] = im[r * G + c] end
    fft1(lr, li, inv)
    for c = 1, G do re[r * G + c] = lr[c]; im[r * G + c] = li[c] end
  end
  for c = 1, G do
    for r = 0, G - 1 do lr[r + 1] = re[r * G + c]; li[r + 1] = im[r * G + c] end
    fft1(lr, li, inv)
    for r = 0, G - 1 do re[r * G + c] = lr[r + 1]; im[r * G + c] = li[r + 1] end
  end
end

local b, x = {}, {}
for i = 0, N - 1 do b[i + 1] = (((i * 7 + floor(i / G) * 13) % 17) - 8) / 8.0 end
build_system()
local res = sqrt(pcg(b, x))

local sre, sim = {}, {}
for i = 1, N do sre[i] = x[i]; sim[i] = 0.0 end
fft2(sre, sim, false)

local cuts = {64, 256, 1024}
local fre, fim = {}, {}
local energy, corner = 0.0, 0.0
for q = 1, 3 do
  for i = 1, N do fre[i] = sre[i]; fim[i] = sim[i] end
  for k = 0, N - 1 do
    local ky, kx = floor(k / G), k % G
    local fy = ky <= G / 2 and ky or G - ky
    local fx = kx <= G / 2 and kx or G - kx
    if fx * fx + fy * fy > cuts[q] then fre[k + 1] = 0.0; fim[k + 1] = 0.0 end
  end
  fft2(fre, fim, true)
  for k = 1, N do energy = energy + fre[k] * fre[k] end
  corner = corner + fre[1] + fre[N / 2 + G / 2 + 1] + fre[N]
end

local xs = dot(x, x)
local cs = floor(sqrt(xs) * 1000000.0) + floor(energy * 1000.0) +
    floor(corner * 1000000.0) + floor(res * 1000000000.0)
print(string.format("CHECKSUM=%d", cs))
