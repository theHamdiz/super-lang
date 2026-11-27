#!/usr/bin/env python3
"""03_astar — 64×64 grid, 40 queries, binary min-heap."""

S = 64
CELLS = S * S
QUERIES = 40
INF = 0x3FFFFFFF

LCG_MOD = 2**31
rng = 10846167

DIRS = ((1, 0), (-1, 0), (0, 1), (0, -1))


def next_u():
    global rng
    rng = (rng * 1103515245 + 12345) % LCG_MOD
    return rng


def heur(a: int, b: int) -> int:
    ax, ay = a % S, a // S
    bx, by = b % S, b // S
    dx = ax - bx
    if dx < 0:
        dx = -dx
    dy = ay - by
    if dy < 0:
        dy = -dy
    return dx + dy


class MinHeap:
    def __init__(self):
        self.heap: list[int] = []
        self.hprio: list[int] = []

    def push(self, node: int, prio: int) -> None:
        self.heap.append(node)
        self.hprio.append(prio)
        i = len(self.heap) - 1
        while i > 0:
            p = (i - 1) // 2
            if self.hprio[i] >= self.hprio[p]:
                break
            self.heap[i], self.heap[p] = self.heap[p], self.heap[i]
            self.hprio[i], self.hprio[p] = self.hprio[p], self.hprio[i]
            i = p

    def pop(self) -> int:
        top = self.heap[0]
        self.heap[0] = self.heap[-1]
        self.hprio[0] = self.hprio[-1]
        self.heap.pop()
        self.hprio.pop()
        i = 0
        n = len(self.heap)
        while True:
            l = 2 * i + 1
            r = l + 1
            sm = i
            if l < n and self.hprio[l] < self.hprio[sm]:
                sm = l
            if r < n and self.hprio[r] < self.hprio[sm]:
                sm = r
            if sm == i:
                break
            self.heap[i], self.heap[sm] = self.heap[sm], self.heap[i]
            self.hprio[i], self.hprio[sm] = self.hprio[sm], self.hprio[i]
            i = sm
        return top

    def __len__(self) -> int:
        return len(self.heap)


def astar(walkable: list[int], start: int, goal: int) -> int:
    gscore = [INF] * CELLS
    came = [-1] * CELLS
    openf = [0] * CELLS
    heap = MinHeap()
    gscore[start] = 0
    heap.push(start, heur(start, goal))
    openf[start] = 1
    while len(heap) > 0:
        cur = heap.pop()
        openf[cur] = 0
        if cur == goal:
            length = 0
            c = goal
            while c != start and c >= 0:
                length += 1
                c = came[c]
            return length
        cx, cy = cur % S, cur // S
        for dx, dy in DIRS:
            nx, ny = cx + dx, cy + dy
            if nx < 0 or ny < 0 or nx >= S or ny >= S:
                continue
            ni = ny * S + nx
            if not walkable[ni]:
                continue
            tent = gscore[cur] + 1
            if tent < gscore[ni]:
                came[ni] = cur
                gscore[ni] = tent
                f = tent + heur(ni, goal)
                if not openf[ni]:
                    heap.push(ni, f)
                    openf[ni] = 1
                else:
                    heap.push(ni, f)
    return -1


def main():
    walkable = [1] * CELLS
    for _ in range(CELLS // 5):
        cell = next_u() % CELLS
        if cell != 0 and cell != CELLS - 1:
            walkable[cell] = 0
    cs = 0
    for _ in range(QUERIES):
        a = next_u() % CELLS
        b = next_u() % CELLS
        while not walkable[a]:
            a = next_u() % CELLS
        while not walkable[b]:
            b = next_u() % CELLS
        cs += astar(walkable, a, b)
    print(f"CHECKSUM={cs}")


if __name__ == "__main__":
    main()