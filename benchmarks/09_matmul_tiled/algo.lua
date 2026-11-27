-- 09_matmul_tiled — 256×256 f64 tiled multiply
local N, TILE = 256, 32

local A, B, C = {}, {}, {}
for i = 0, N * N - 1 do
  A[i + 1] = ((i * 17) % 100) / 100.0
  B[i + 1] = ((i * 31) % 100) / 100.0
  C[i + 1] = 0.0
end

for ii = 0, N - 1, TILE do
  for jj = 0, N - 1, TILE do
    for kk = 0, N - 1, TILE do
      local iimax = math.min(ii + TILE, N)
      local jjmax = math.min(jj + TILE, N)
      local kkmax = math.min(kk + TILE, N)
      for i = ii, iimax - 1 do
        for k = kk, kkmax - 1 do
          local aik = A[i * N + k + 1]
          for j = jj, jjmax - 1 do
            C[i * N + j + 1] = C[i * N + j + 1] + aik * B[k * N + j + 1]
          end
        end
      end
    end
  end
end

local fro = 0.0
for i = 1, N * N do
  fro = fro + C[i] * C[i]
end
local cs = math.floor(math.sqrt(fro) * 1000.0)
print(string.format("CHECKSUM=%d", cs))