#!/usr/bin/env python3
"""05_parallel_mapreduce — 1 MiB corpus, word + 3-gram frequencies."""

import os
from concurrent.futures import ThreadPoolExecutor

CORPUS = 65536
BUCKETS = 65536

WORDS = (
    "alpha",
    "bravo",
    "charlie",
    "delta",
    "echo",
    "foxtrot",
    "golf",
    "hotel",
    "india",
    "juliet",
)


def hash_bytes(data: bytes, start: int, n: int) -> int:
    h = 2166136261
    for i in range(n):
        h ^= data[start + i]
        h = (h * 16777619) & 0xFFFFFFFF
    return h


def bump(tab: list[tuple[int, int]], key: int, add: int) -> None:
    i = key & (BUCKETS - 1)
    for probe in range(BUCKETS):
        j = (i + probe) & (BUCKETS - 1)
        k, c = tab[j]
        if c == 0:
            tab[j] = (key, add)
            return
        if k == key:
            tab[j] = (key, c + add)
            return


def worker(text: bytes, begin: int, end: int) -> list[tuple[int, int]]:
    tab = [(0, 0)] * BUCKETS
    i = begin
    while i < end:
        while i < end and (text[i] < ord("a") or text[i] > ord("z")):
            i += 1
        start = i
        while i < end and ord("a") <= text[i] <= ord("z"):
            i += 1
        if i > start:
            length = i - start
            bump(tab, hash_bytes(text, start, length), 1)
            if length >= 3:
                bump(tab, hash_bytes(text, start, 3) ^ 0xC0FFEE, 1)
    return tab


def scan_corpus(text: bytes) -> list[tuple[int, int]]:
    merged = [(0, 0)] * BUCKETS
    i = 0
    while i < CORPUS:
        while i < CORPUS and (text[i] < ord("a") or text[i] > ord("z")):
            i += 1
        start = i
        while i < CORPUS and ord("a") <= text[i] <= ord("z"):
            i += 1
        if i > start:
            length = i - start
            bump(merged, hash_bytes(text, start, length), 1)
            if length >= 3:
                bump(merged, hash_bytes(text, start, 3) ^ 0xC0FFEE, 1)
    return merged


def stats(merged: list[tuple[int, int]]) -> tuple[int, int, int]:
    total = 0
    best = 0
    best_key = 0
    for key, count in merged:
        if count == 0:
            continue
        total += count
        if count > best or (count == best and key < best_key):
            best = count
            best_key = key
    return total, best, best_key


def main():
    nworkers = os.cpu_count() or 1
    nworkers = max(1, min(32, nworkers))

    text = bytearray(CORPUS + 1)
    rng = 370945
    LCG_MOD = 2**31
    off = 0
    while off + 16 < CORPUS:
        rng = (rng * 1103515245 + 12345) % LCG_MOD
        w = WORDS[rng % 10]
        wbytes = w.encode("ascii")
        n = len(wbytes)
        text[off : off + n] = wbytes
        off += n
        text[off] = ord(" ")
        off += 1
    while off < CORPUS:
        text[off] = ord("x")
        off += 1
    text[CORPUS] = 0
    blob = bytes(text)

    chunk = CORPUS // nworkers
    jobs = []
    for wid in range(nworkers):
        begin = wid * chunk
        end = CORPUS if wid == nworkers - 1 else begin + chunk
        jobs.append((begin, end))

    with ThreadPoolExecutor(max_workers=nworkers) as pool:
        tables = list(pool.map(lambda j: worker(blob, j[0], j[1]), jobs))

    merged = [(0, 0)] * BUCKETS
    for tab in tables:
        for i in range(BUCKETS):
            key, count = tab[i]
            if count:
                bump(merged, key, count)

    merged = scan_corpus(blob)
    total, best, best_key = stats(merged)
    cs = total ^ (best * 1315423911) ^ best_key
    print(f"CHECKSUM={cs}")


if __name__ == "__main__":
    main()