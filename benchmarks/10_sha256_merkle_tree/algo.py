#!/usr/bin/env python3
"""10_sha256_merkle — 1024 leaves × 64 bytes, SHA-256 Merkle root."""

import hashlib

LEAVES = 1024
LEAF_SZ = 64


def main():
    level = []
    leaf = bytearray(LEAF_SZ)
    for i in range(LEAVES):
        for b in range(LEAF_SZ):
            leaf[b] = (i * 131 + b * 17) & 255
        level.append(hashlib.sha256(bytes(leaf)).digest())

    n = LEAVES
    while n > 1:
        m = (n + 1) // 2
        nxt = []
        for i in range(m):
            cat = bytearray(64)
            cat[:32] = level[2 * i]
            if 2 * i + 1 < n:
                cat[32:] = level[2 * i + 1]
            else:
                cat[32:] = level[2 * i]
            nxt.append(hashlib.sha256(bytes(cat)).digest())
        level = nxt
        n = m

    cs = 0
    MASK = (1 << 64) - 1
    for b in level[0]:
        cs = ((cs * 131) + b) & MASK
    if cs >= (1 << 63):
        cs -= 1 << 64
    print(f"CHECKSUM={cs}")


if __name__ == "__main__":
    main()