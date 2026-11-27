-- 19_distributed_raft_wal_kv — single-process Raft log replication (leader + 2 followers):
-- CRC32-protected WAL records, lossy/corrupting virtual links, majority commit, KV state machine.
local ffi = require("ffi")
local bxor, band, rshift = bit.bxor, bit.band, bit.rshift

local MOD = 1000000007
local TICKS, DRAIN, REC, BATCH, KEYS = 6000, 200, 20, 4, 512
local DROP_PCT, ACK_DROP_PCT, CORRUPT_PCT = 35, 15, 3
local CAP = TICKS * REC

local CRC = ffi.new("uint32_t[256]")
for i = 0, 255 do
  local c = i
  for _ = 1, 8 do
    if band(c, 1) == 1 then c = bxor(rshift(c, 1), 0xEDB88320) else c = rshift(c, 1) end
  end
  CRC[i] = c
end
local function crc32(d, off, n)
  local c = 0xFFFFFFFF
  for i = off, off + n - 1 do
    c = bxor(CRC[band(bxor(c, d[i]), 255)], rshift(c, 8))
  end
  return bxor(c, 0xFFFFFFFF) % 4294967296
end
local function put32(p, o, v)
  p[o] = math.floor(v / 16777216) % 256
  p[o + 1] = math.floor(v / 65536) % 256
  p[o + 2] = math.floor(v / 256) % 256
  p[o + 3] = v % 256
end
local function get32(p, o) return p[o] * 16777216 + p[o + 1] * 65536 + p[o + 2] * 256 + p[o + 3] end

local rng = 19019
local function next_rand()
  rng = rng * 48271 % 2147483647
  return rng
end

local wal = { [0] = ffi.new("uint8_t[?]", CAP), ffi.new("uint8_t[?]", CAP), ffi.new("uint8_t[?]", CAP) }
local kv = { [0] = ffi.new("int32_t[?]", KEYS), ffi.new("int32_t[?]", KEYS), ffi.new("int32_t[?]", KEYS) }
local lens, applied, match, nxt = { [0] = 0, 0, 0 }, { [0] = 0, 0, 0 }, { [0] = 0, 0, 0 }, { [0] = 1, 1, 1 }
local commit, sent, dropped, crc_rejects, acks_lost = 0, 0, 0, 0, 0

local function apply_upto(n, upto)
  local w, m = wal[n], kv[n]
  while applied[n] < upto do
    local o = applied[n] * REC
    if crc32(w, o, 16) == get32(w, o + 16) then m[get32(w, o + 8) % KEYS] = get32(w, o + 12) end
    applied[n] = applied[n] + 1
  end
end

local msg = ffi.new("uint8_t[?]", BATCH * REC)
for t = 0, TICKS + DRAIN - 1 do
  if t < TICKS then
    local w, o = wal[0], lens[0] * REC
    put32(w, o, lens[0] + 1)
    put32(w, o + 4, 1)
    put32(w, o + 8, next_rand() % KEYS)
    put32(w, o + 12, next_rand() % 1000000)
    put32(w, o + 16, crc32(w, o, 16))
    lens[0] = lens[0] + 1
  end
  local leader_commit = commit
  for f = 1, 2 do
    local n0 = nxt[f]
    if n0 <= lens[0] then
      local cnt = math.min(lens[0] - n0 + 1, BATCH)
      local r1 = next_rand() % 100
      local r2 = next_rand() % 100
      local r3 = next_rand() % 100
      sent = sent + 1
      if r1 < DROP_PCT then
        dropped = dropped + 1
      else
        ffi.copy(msg, wal[0] + (n0 - 1) * REC, cnt * REC)
        if r2 < CORRUPT_PCT then
          local pos = next_rand() % (cnt * REC)
          msg[pos] = bxor(msg[pos], 0x5A)
        end
        for j = 0, cnt - 1 do
          local o = j * REC
          if crc32(msg, o, 16) ~= get32(msg, o + 16) then
            crc_rejects = crc_rejects + 1
            break
          end
          local idx = get32(msg, o)
          if idx == lens[f] + 1 then
            ffi.copy(wal[f] + lens[f] * REC, msg + o, REC)
            lens[f] = lens[f] + 1
          elseif idx > lens[f] + 1 then
            break
          end
        end
        apply_upto(f, math.min(leader_commit, lens[f]))
        if r3 < ACK_DROP_PCT then
          acks_lost = acks_lost + 1
        else
          if lens[f] > match[f] then match[f] = lens[f] end
          nxt[f] = lens[f] + 1
        end
      end
    end
  end
  local a, b, c = lens[0], match[1], match[2]
  if a > b then a, b = b, a end
  if b > c then b, c = c, b end
  if a > b then a, b = b, a end
  if b > commit then commit = b end
  apply_upto(0, commit)
end

local cs = 0
local function fold(v) cs = (cs * 131 + v) % MOD end
for _, v in ipairs({ lens[0], lens[1], lens[2], commit, applied[0], applied[1], applied[2], sent, dropped, crc_rejects, acks_lost }) do
  fold(v)
end
for n = 0, 2 do
  local d = 0
  for k = 0, KEYS - 1 do d = (d * 131 + kv[n][k]) % MOD end
  fold(d)
end
for n = 0, 1 do fold(crc32(wal[n], 0, lens[n] * REC)) end
print(string.format("CHECKSUM=%d", cs))
