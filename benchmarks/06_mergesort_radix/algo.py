#!/usr/bin/env python3
"""06_mergesort_radix — sort 50000 u64, verify + checksum."""

N = 50_000

LCG_MOD = 2**31
rng = 395799


def next_u():
    global rng
    rng = (rng * 1103515245 + 12345) % LCG_MOD
    return rng


def merge(a: list[int], tmp: list[int], lo: int, mid: int, hi: int) -> None:
    i, j, k = lo, mid, lo
    while i < mid and j < hi:
        if a[i] <= a[j]:
            tmp[k] = a[i]
            i += 1
        else:
            tmp[k] = a[j]
            j += 1
        k += 1
    while i < mid:
        tmp[k] = a[i]
        i += 1
        k += 1
    while j < hi:
        tmp[k] = a[j]
        j += 1
        k += 1
    for i in range(lo, hi):
        a[i] = tmp[i]


def mergesort(a: list[int], tmp: list[int], lo: int, hi: int) -> None:
    if hi - lo <= 1:
        return
    mid = lo + (hi - lo) // 2
    mergesort(a, tmp, lo, mid)
    mergesort(a, tmp, mid, hi)
    merge(a, tmp, lo, mid, hi)


def i64(x: int) -> int:
    x &= 0xFFFFFFFFFFFFFFFF
    if x >= 1 << 63:
        x -= 1 << 64
    return x


def main():
    a = [next_u() for _ in range(N)]
    tmp = [0] * N
    mergesort(a, tmp, 0, N)
    cs = 0
    ok = 1
    for i in range(N):
        cs = i64(cs ^ (a[i] & 0xFFFFFFFF))
        cs = i64(cs * 131 + (a[i] >> 32))
        if i > 0 and a[i] < a[i - 1]:
            ok = 0
    print(f"CHECKSUM={i64(cs ^ (1 if ok else 0))}")


if __name__ == "__main__":
    main()