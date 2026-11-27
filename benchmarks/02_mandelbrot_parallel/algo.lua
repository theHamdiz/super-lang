-- 02_mandelbrot — 1024×1024 maxiter 128
local W, H, MAXIT = 1024, 1024, 128

local function mandelbrot_row(y)
  local local_cs = 0
  local cy = (y / H) * 2.0 - 1.0
  for x = 0, W - 1 do
    local cx = (x / W) * 3.5 - 2.5
    local zx, zy = 0.0, 0.0
    local it = 0
    while it < MAXIT do
      local zx2, zy2 = zx * zx, zy * zy
      if zx2 + zy2 > 4.0 then break end
      local nzy = 2.0 * zx * zy + cy
      zx = zx2 - zy2 + cx
      zy = nzy
      it = it + 1
    end
    local_cs = local_cs + it
  end
  return local_cs
end

local cs = 0
for y = 0, H - 1 do
  cs = cs + mandelbrot_row(y)
end
print(string.format("CHECKSUM=%d", cs))