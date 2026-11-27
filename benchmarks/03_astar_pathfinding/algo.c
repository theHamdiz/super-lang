/* 03_astar — 128×128 grid, 40 queries, binary min-heap */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define S 64
#define CELLS (S * S)
#define QUERIES 40
#define INF 0x3fffffff

#define LCG_MOD 2147483648ULL
static uint64_t rng = 10846167ULL;
static uint32_t next_u(void) {
  rng = (rng * 1103515245ULL + 12345ULL) % LCG_MOD;
  return (uint32_t)rng;
}

static int walkable[CELLS];
static int gscore[CELLS];
static int came[CELLS];
static int openf[CELLS];
static int heap[CELLS];
static int hprio[CELLS];
static int hn;

static void heap_push(int node, int prio) {
  int i = hn++;
  heap[i] = node;
  hprio[i] = prio;
  while (i > 0) {
    int p = (i - 1) / 2;
    if (hprio[i] >= hprio[p]) break;
    int tn = heap[i];
    heap[i] = heap[p];
    heap[p] = tn;
    int tp = hprio[i];
    hprio[i] = hprio[p];
    hprio[p] = tp;
    i = p;
  }
}

static int heap_pop(void) {
  int top = heap[0];
  hn--;
  heap[0] = heap[hn];
  hprio[0] = hprio[hn];
  int i = 0;
  for (;;) {
    int l = 2 * i + 1, r = l + 1, sm = i;
    if (l < hn && hprio[l] < hprio[sm]) sm = l;
    if (r < hn && hprio[r] < hprio[sm]) sm = r;
    if (sm == i) break;
    int tn = heap[i];
    heap[i] = heap[sm];
    heap[sm] = tn;
    int tp = hprio[i];
    hprio[i] = hprio[sm];
    hprio[sm] = tp;
    i = sm;
  }
  return top;
}

static int heur(int a, int b) {
  int ax = a % S, ay = a / S, bx = b % S, by = b / S;
  int dx = ax - bx;
  if (dx < 0) dx = -dx;
  int dy = ay - by;
  if (dy < 0) dy = -dy;
  return dx + dy;
}

static const int DIRS[4][2] = {{1, 0}, {-1, 0}, {0, 1}, {0, -1}};

static int astar(int start, int goal) {
  for (int i = 0; i < CELLS; i++) {
    gscore[i] = INF;
    openf[i] = 0;
    came[i] = -1;
  }
  hn = 0;
  gscore[start] = 0;
  heap_push(start, heur(start, goal));
  openf[start] = 1;
  while (hn > 0) {
    int cur = heap_pop();
    openf[cur] = 0;
    if (cur == goal) {
      int len = 0, c = goal;
      while (c != start && c >= 0) {
        len++;
        c = came[c];
      }
      return len;
    }
    int cx = cur % S, cy = cur / S;
    for (int d = 0; d < 4; d++) {
      int nx = cx + DIRS[d][0], ny = cy + DIRS[d][1];
      if (nx < 0 || ny < 0 || nx >= S || ny >= S) continue;
      int ni = ny * S + nx;
      if (!walkable[ni]) continue;
      int tent = gscore[cur] + 1;
      if (tent < gscore[ni]) {
        came[ni] = cur;
        gscore[ni] = tent;
        int f = tent + heur(ni, goal);
        if (!openf[ni]) {
          heap_push(ni, f);
          openf[ni] = 1;
        } else {
          heap_push(ni, f);
        }
      }
    }
  }
  return -1;
}

int main(void) {
  for (int i = 0; i < CELLS; i++) walkable[i] = 1;
  for (int i = 0; i < CELLS / 5; i++) {
    int cell = (int)(next_u() % (uint64_t)CELLS);
    if (cell != 0 && cell != CELLS - 1) walkable[cell] = 0;
  }
  long long cs = 0;
  for (int q = 0; q < QUERIES; q++) {
    int a = (int)(next_u() % (uint64_t)CELLS);
    int b = (int)(next_u() % (uint64_t)CELLS);
    while (!walkable[a]) a = (int)(next_u() % (uint64_t)CELLS);
    while (!walkable[b]) b = (int)(next_u() % (uint64_t)CELLS);
    int path = astar(a, b);
    cs += path;
  }
  printf("CHECKSUM=%lld\n", cs);
  return 0;
}
