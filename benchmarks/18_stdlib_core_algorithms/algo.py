#!/usr/bin/env python3
"""18_stdlib_core_algorithms — one checksum from four std-style kernels:
(A) KMP-DFA "regex" field extraction over 3000 generated log lines,
(B) strict UTF-8 validation/counting over 60 documents (some corrupted),
(C) SQL-ish filter / group-by / order-by-limit / join over 6000 rows x 24 queries,
(D) HM-style type unification (union-find, iterative) over 8504 type nodes."""

MOD = 1_000_000_007
NL, NDOC, PERDOC, NR, NQ = 3000, 60, 200, 6000, 24
NVARS, NCOMP, NCON = 2500, 6000, 5000
NTOT = NVARS + 4 + NCOMP

rng = 18018
cs = 0


def next_rand() -> int:
    global rng
    rng = rng * 48271 % 2147483647
    return rng


def fold(v: int) -> None:
    global cs
    cs = (cs * 131 + v) % MOD


# ---------- (A) log extraction ----------
def build_dfa(needle: bytes) -> list[int]:
    n = len(needle)
    dfa = [0] * (n * 256)
    dfa[needle[0]] = 1
    x = 0
    for j in range(1, n):
        dfa[j * 256:(j + 1) * 256] = dfa[x * 256:(x + 1) * 256]
        dfa[j * 256 + needle[j]] = j + 1
        x = dfa[x * 256 + needle[j]]
    return dfa


def extract(line: bytes, dfa: list[int], n: int) -> int:
    st, i, e = 0, 0, len(line)
    while i < e:
        st = dfa[st * 256 + line[i]]
        i += 1
        if st == n:
            v = nd = 0
            while i < e and 48 <= line[i] <= 57:
                v = v * 10 + line[i] - 48
                nd += 1
                i += 1
            if nd > 0:
                return v
            st = 0
    return -1


def part_a() -> None:
    levels = (b"INFO", b"WARN", b"ERROR", b"DEBUG")
    codes = (200, 201, 204, 301, 400, 401, 404, 500, 503)
    d_status, d_took, d_user = build_dfa(b"status="), build_dfa(b"took="), build_dfa(b"user=")
    cls = [0] * 8
    took_sum = took_max = took_n = user_sum = bad = total = 0
    for i in range(NL):
        mo, dd, hh = 1 + next_rand() % 12, 1 + next_rand() % 28, next_rand() % 24
        mi, ss, lv = next_rand() % 60, next_rand() % 60, next_rand() % 4
        svc, uid, sidx, ms = next_rand() % 100, next_rand() % 100000, next_rand() % 9, next_rand() % 3000
        parts = [b"2026-%02d-%02d %02d:%02d:%02d " % (mo, dd, hh, mi, ss), levels[lv], b" svc-%02d user=%d" % (svc, uid)]
        parts.append(b" status=abc" if i % 13 == 5 else b" status=%d" % codes[sidx])
        if i % 17 != 3:
            parts.append(b" took=%dms" % ms)
        line = b"".join(parts)
        st, tk, us = extract(line, d_status, 7), extract(line, d_took, 5), extract(line, d_user, 5)
        if st < 0:
            bad += 1
        else:
            cls[st // 100] += 1
        if tk >= 0:
            took_sum += tk
            took_n += 1
            took_max = max(took_max, tk)
        user_sum += us
        total += len(line) + 1
    for c in cls:
        fold(c)
    for v in (took_sum, took_max, took_n, user_sum, bad, total):
        fold(v)


# ---------- (B) UTF-8 ----------
def utf8_valid(d: bytearray) -> bool:
    i, n = 0, len(d)
    while i < n:
        b0 = d[i]
        if b0 < 128:
            i += 1
        elif b0 < 194:
            return False
        elif b0 < 224:
            if i + 1 >= n or d[i + 1] & 0xC0 != 0x80:
                return False
            i += 2
        elif b0 < 240:
            if i + 2 >= n or d[i + 1] & 0xC0 != 0x80 or d[i + 2] & 0xC0 != 0x80:
                return False
            cp = (b0 - 224) * 4096 + (d[i + 1] - 128) * 64 + (d[i + 2] - 128)
            if cp < 2048 or 55296 <= cp <= 57343:
                return False
            i += 3
        elif b0 < 245:
            if i + 3 >= n or any(d[i + k] & 0xC0 != 0x80 for k in (1, 2, 3)):
                return False
            cp = (b0 - 240) * 262144 + (d[i + 1] - 128) * 4096 + (d[i + 2] - 128) * 64 + (d[i + 3] - 128)
            if cp < 65536 or cp > 1114111:
                return False
            i += 4
        else:
            return False
    return True


def utf8_count(d: bytearray) -> int:
    return sum(1 for b in d if b & 0xC0 != 0x80)


def encode(cp: int) -> bytes:
    if cp < 128:
        return bytes([cp])
    if cp < 2048:
        return bytes([192 + cp // 64, 128 + cp % 64])
    if cp < 65536:
        return bytes([224 + cp // 4096, 128 + cp // 64 % 64, 128 + cp % 64])
    return bytes([240 + cp // 262144, 128 + cp // 4096 % 64, 128 + cp // 64 % 64, 128 + cp % 64])


def part_b() -> None:
    badb = (0x80, 0xC0, 0xC1, 0xF5, 0xFF, 0xE0)
    valid_docs = total_cp = 0
    for d in range(NDOC):
        doc = bytearray()
        for _ in range(PERDOC):
            cat, r = next_rand() % 4, next_rand()
            cp = (r % 128, 128 + r % 1920, 2048 + r % 53248, 65536 + r % 1048576)[cat]
            doc += encode(cp)
        if d % 5 == 2:
            p, b = next_rand() % len(doc), next_rand() % 6
            doc[p] = badb[b]
        ok = utf8_valid(doc)
        cnt = utf8_count(doc) if ok else 0
        fold(int(ok))
        fold(cnt)
        fold(len(doc))
        valid_docs += ok
        total_cp += cnt
    fold(valid_docs)
    fold(total_cp)


# ---------- (C) SQL-ish ----------
def part_c() -> None:
    dept, age, sal, act = [0] * NR, [0] * NR, [0] * NR, [0] * NR
    for i in range(NR):
        dept[i] = next_rand() % 16
        age[i] = 18 + next_rand() % 50
        sal[i] = 30000 + next_rand() % 90000
        act[i] = 1 if next_rand() % 10 != 0 else 0
    for _ in range(NQ):
        age_lo, sal_max, need = 18 + next_rand() % 30, 40000 + next_rand() % 80000, next_rand() % 2
        cnt, tot, mx, taken = [0] * 16, [0] * 16, [0] * 16, [0] * 16
        for i in range(NR):
            if age[i] >= age_lo and sal[i] < sal_max and act[i] >= need:
                g = dept[i]
                cnt[g] += 1
                tot[g] += sal[i]
                if age[i] > mx[g]:
                    mx[g] = age[i]
        for _ in range(3):
            best = -1
            for g in range(16):
                if cnt[g] >= 5 and not taken[g] and (best < 0 or tot[g] > tot[best]):
                    best = g
            if best >= 0:
                taken[best] = 1
                for v in (best, cnt[best], tot[best], mx[best], tot[best] // ((best + 1) * 1000)):
                    fold(v)
            else:
                fold(999)


# ---------- (D) HM unification ----------
def part_d() -> None:
    kind, na, nb = [0] * NTOT, [-1] * NTOT, [-1] * NTOT
    parent = list(range(NTOT))
    for c in range(4):
        kind[NVARS + c], na[NVARS + c] = 1, c
    for i in range(NVARS + 4, NTOT):
        kind[i], na[i], nb[i] = 2, next_rand() % i, next_rand() % i
    unions = 0

    def find(x: int) -> int:
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x

    def unify(x: int, y: int) -> bool:
        nonlocal unions
        stack = [(x, y)]
        while stack:
            a, b = stack.pop()
            rx, ry = find(a), find(b)
            if rx == ry:
                continue
            kx, ky = kind[rx], kind[ry]
            if kx == 0:
                parent[rx] = ry
                unions += 1
            elif ky == 0:
                parent[ry] = rx
                unions += 1
            elif kx == 1 or ky == 1:
                if kx == 1 and ky == 1 and na[rx] == na[ry]:
                    parent[ry] = rx
                    unions += 1
                else:
                    return False
            else:
                parent[ry] = rx
                unions += 1
                stack.append((na[rx], na[ry]))
                stack.append((nb[rx], nb[ry]))
        return True

    fails = 0
    for _ in range(NCON):
        t = next_rand() % 2
        x = next_rand() % NVARS if t == 0 else next_rand() % NTOT
        y = next_rand() % NTOT
        if not unify(x, y):
            fails += 1
    fold(fails)
    fold(unions)
    classes = 0
    for i in range(NTOT):
        r = find(i)
        if r == i:
            classes += 1
        fold(r * 3 + kind[r])
    fold(classes)


def main() -> None:
    part_a()
    part_b()
    part_c()
    part_d()
    print(f"CHECKSUM={cs}")


if __name__ == "__main__":
    main()
