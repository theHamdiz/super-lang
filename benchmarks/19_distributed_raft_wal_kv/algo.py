#!/usr/bin/env python3
"""19_distributed_raft_wal_kv — single-process Raft log replication (leader + 2 followers):
CRC32-protected WAL records, lossy/corrupting virtual links, majority commit, KV state machine."""

MOD = 1_000_000_007
TICKS, DRAIN, REC, BATCH, KEYS = 6000, 200, 20, 4, 512
DROP_PCT, ACK_DROP_PCT, CORRUPT_PCT = 35, 15, 3
CAP = TICKS * REC


def _crc_table():
    t = []
    for i in range(256):
        c = i
        for _ in range(8):
            c = (c >> 1) ^ 0xEDB88320 if c & 1 else c >> 1
        t.append(c)
    return t


CRC = _crc_table()


def crc32(d, off, n):
    c = 0xFFFFFFFF
    for i in range(off, off + n):
        c = CRC[(c ^ d[i]) & 255] ^ (c >> 8)
    return c ^ 0xFFFFFFFF


def put32(p, o, v):
    p[o] = (v >> 24) & 255
    p[o + 1] = (v >> 16) & 255
    p[o + 2] = (v >> 8) & 255
    p[o + 3] = v & 255


def get32(p, o):
    return (p[o] << 24) | (p[o + 1] << 16) | (p[o + 2] << 8) | p[o + 3]


def main() -> None:
    rng = 19019

    def next_rand():
        nonlocal rng
        rng = rng * 48271 % 2147483647
        return rng

    wal = [bytearray(CAP) for _ in range(3)]
    kv = [[0] * KEYS for _ in range(3)]
    lens, applied, match = [0] * 3, [0] * 3, [0] * 3
    nxt = [1, 1, 1]
    commit = sent = dropped = crc_rejects = acks_lost = 0

    def apply_upto(n, upto):
        w, m = wal[n], kv[n]
        while applied[n] < upto:
            o = applied[n] * REC
            if crc32(w, o, 16) == get32(w, o + 16):
                m[get32(w, o + 8) % KEYS] = get32(w, o + 12)
            applied[n] += 1

    for t in range(TICKS + DRAIN):
        if t < TICKS:
            w = wal[0]
            o = lens[0] * REC
            put32(w, o, lens[0] + 1)
            put32(w, o + 4, 1)
            put32(w, o + 8, next_rand() % KEYS)
            put32(w, o + 12, next_rand() % 1000000)
            put32(w, o + 16, crc32(w, o, 16))
            lens[0] += 1
        leader_commit = commit
        for f in (1, 2):
            n0 = nxt[f]
            if n0 > lens[0]:
                continue
            cnt = min(lens[0] - n0 + 1, BATCH)
            r1 = next_rand() % 100
            r2 = next_rand() % 100
            r3 = next_rand() % 100
            sent += 1
            if r1 < DROP_PCT:
                dropped += 1
                continue
            msg = bytearray(wal[0][(n0 - 1) * REC:(n0 - 1) * REC + cnt * REC])
            if r2 < CORRUPT_PCT:
                msg[next_rand() % (cnt * REC)] ^= 0x5A
            for j in range(cnt):
                o = j * REC
                if crc32(msg, o, 16) != get32(msg, o + 16):
                    crc_rejects += 1
                    break
                idx = get32(msg, o)
                if idx == lens[f] + 1:
                    d = lens[f] * REC
                    wal[f][d:d + REC] = msg[o:o + REC]
                    lens[f] += 1
                elif idx > lens[f] + 1:
                    break
            apply_upto(f, min(leader_commit, lens[f]))
            if r3 < ACK_DROP_PCT:
                acks_lost += 1
                continue
            if lens[f] > match[f]:
                match[f] = lens[f]
            nxt[f] = lens[f] + 1
        med = sorted((lens[0], match[1], match[2]))[1]
        if med > commit:
            commit = med
        apply_upto(0, commit)

    cs = 0
    for v in (*lens, commit, *applied, sent, dropped, crc_rejects, acks_lost):
        cs = (cs * 131 + v) % MOD
    for n in range(3):
        d = 0
        for k in range(KEYS):
            d = (d * 131 + kv[n][k]) % MOD
        cs = (cs * 131 + d) % MOD
    for n in range(2):
        cs = (cs * 131 + crc32(wal[n], 0, lens[n] * REC)) % MOD
    print(f"CHECKSUM={cs}")


if __name__ == "__main__":
    main()
