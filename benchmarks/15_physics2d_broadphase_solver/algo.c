/* 15_physics2d_broadphase_solver - 6k circles/boxes: uniform spatial-hash broadphase,
   circle/box manifolds, sequential-impulse solver (4 iterations x 10 steps).
   Mirrors std.game.physics2d (grid build, candidate order, narrowphase, resolve). */
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define COLS 100
#define ROWS 60
#define N_DYN (COLS * ROWS)
#define FLOOR_TILES 62
#define WALL_TILES 40
#define N_STATIC (FLOOR_TILES + 2 * WALL_TILES)
#define N_BODIES (N_STATIC + N_DYN)
#define STEPS 10
#define ITERS 4
#define MOD 1000000007LL

typedef struct {
  double x, y, vx, vy, inv_mass;
  int64_t shape;
  double r, hw, hh, e, mu;
} Body;

typedef struct {
  int hit;
  double nx, ny, depth;
} Contact;

static Body bodies[N_BODIES];
static int nb = 0;

static int64_t rng = 150015;
static int64_t draw(int64_t m) {
  rng = (rng * 1103515245 + 12345) % 2147483648;
  return rng / 256 % m;
}

static double p2_abs(double v) { return v < 0.0 ? 0.0 - v : v; }
static double p2_min(double a, double b) { return a < b ? a : b; }
static double p2_max(double a, double b) { return a > b ? a : b; }
static double p2_clamp(double v, double lo, double hi) { return v < lo ? lo : v > hi ? hi : v; }
static double p2_sign(double v) { return v < 0.0 ? 0.0 - 1.0 : 1.0; }
static double p2_inv(double m) { return m <= 0.0 ? 0.0 : 1.0 / m; }

static void add_circle(double x, double y, double r, double mass, double vx, double vy) {
  Body b = {x, y, vx, vy, p2_inv(mass), 0, r, r, r, 0.2, 0.4};
  bodies[nb++] = b;
}
static void add_box(double x, double y, double hw, double hh, double mass, double vx, double vy) {
  Body b = {x, y, vx, vy, p2_inv(mass), 1, 0.0, hw, hh, 0.2, 0.4};
  bodies[nb++] = b;
}

/* ---- narrowphase ---- */
static Contact no_contact(void) { Contact c = {0, 0.0, 0.0, 0.0}; return c; }

static Contact circle_circle(const Body *a, const Body *b) {
  double dx = b->x - a->x, dy = b->y - a->y;
  double rr = a->r + b->r;
  double d2 = dx * dx + dy * dy;
  if (d2 >= rr * rr) return no_contact();
  if (d2 == 0.0) { Contact c = {1, 1.0, 0.0, rr}; return c; }
  double d = sqrt(d2);
  Contact c = {1, dx / d, dy / d, rr - d};
  return c;
}

static Contact box_box(const Body *a, const Body *b) {
  double dx = b->x - a->x, dy = b->y - a->y;
  double ox = a->hw + b->hw - p2_abs(dx);
  double oy = a->hh + b->hh - p2_abs(dy);
  if (ox <= 0.0) return no_contact();
  if (oy <= 0.0) return no_contact();
  if (ox < oy) { Contact c = {1, p2_sign(dx), 0.0, ox}; return c; }
  Contact c = {1, 0.0, p2_sign(dy), oy};
  return c;
}

static Contact circle_box(const Body *a, const Body *b) {
  double lx = a->x - b->x, ly = a->y - b->y;
  double cx = p2_clamp(lx, 0.0 - b->hw, b->hw);
  double cy = p2_clamp(ly, 0.0 - b->hh, b->hh);
  double fx = lx - cx, fy = ly - cy;
  double d2 = fx * fx + fy * fy;
  if (d2 > 0.0) {
    if (d2 >= a->r * a->r) return no_contact();
    double d = sqrt(d2);
    Contact c = {1, (0.0 - fx) / d, (0.0 - fy) / d, a->r - d};
    return c;
  }
  double px = b->hw - p2_abs(lx), py = b->hh - p2_abs(ly);
  if (px < py) { Contact c = {1, 0.0 - p2_sign(lx), 0.0, px + a->r}; return c; }
  Contact c = {1, 0.0, 0.0 - p2_sign(ly), py + a->r};
  return c;
}

static Contact collide(const Body *a, const Body *b) {
  if (a->shape == 0 && b->shape == 0) return circle_circle(a, b);
  if (a->shape == 1 && b->shape == 1) return box_box(a, b);
  if (a->shape == 0 && b->shape == 1) return circle_box(a, b);
  Contact c = circle_box(b, a);
  c.nx = 0.0 - c.nx;
  c.ny = 0.0 - c.ny;
  return c;
}

static void resolve(Body *a, Body *b, Contact c) {
  double ia = a->inv_mass, ib = b->inv_mass, isum = ia + ib;
  if (!c.hit) return;
  if (isum == 0.0) return;
  double rvx = b->vx - a->vx, rvy = b->vy - a->vy;
  double vn = rvx * c.nx + rvy * c.ny;
  if (vn < 0.0) {
    double e = p2_min(a->e, b->e);
    double j = (0.0 - (1.0 + e)) * vn / isum;
    a->vx = a->vx - j * ia * c.nx;
    a->vy = a->vy - j * ia * c.ny;
    b->vx = b->vx + j * ib * c.nx;
    b->vy = b->vy + j * ib * c.ny;
    double tx = rvx - vn * c.nx, ty = rvy - vn * c.ny;
    double tl = sqrt(tx * tx + ty * ty);
    if (tl > 0.000001) {
      tx = tx / tl;
      ty = ty / tl;
      double jt = (0.0 - (rvx * tx + rvy * ty)) / isum;
      double mu = sqrt(a->mu * b->mu);
      double lim = mu * j;
      jt = p2_clamp(jt, 0.0 - lim, lim);
      a->vx = a->vx - jt * ia * tx;
      a->vy = a->vy - jt * ia * ty;
      b->vx = b->vx + jt * ib * tx;
      b->vy = b->vy + jt * ib * ty;
    }
  }
  double corr = p2_max(c.depth - 0.01, 0.0) / isum * 0.8;
  a->x = a->x - corr * ia * c.nx;
  a->y = a->y - corr * ia * c.ny;
  b->x = b->x + corr * ib * c.nx;
  b->y = b->y + corr * ib * c.ny;
}

static int bounds_overlap(const Body *a, const Body *b) {
  if (p2_abs(b->x - a->x) >= a->hw + b->hw) return 0;
  if (p2_abs(b->y - a->y) >= a->hh + b->hh) return 0;
  return 1;
}

/* ---- broadphase: uniform spatial hash ---- */
typedef struct {
  double inv;
  int64_t mask;
  int64_t *heads;
  int64_t *eb, *ex, *ey, *en;
  int64_t len, cap;
} Grid;

static int64_t cell_of(double v, double inv) { return (int64_t)floor(v * inv); }
static int64_t bucket(int64_t cx, int64_t cy, int64_t mask) { return ((cx * 73856093) ^ (cy * 19349663)) & mask; }

static double cell_size(int n) {
  double sum = 0.0, big = 0.0;
  for (int i = 0; i < n; i++) {
    double e = p2_max(bodies[i].hw, bodies[i].hh);
    sum += e;
    if (e > big) big = e;
  }
  double cell = p2_max(2.0 * sum / (double)n, big / 8.0);
  if (cell < 0.000001) return 1.0;
  return cell;
}

static void grid_push(Grid *g, int64_t head, int64_t i, int64_t cx, int64_t cy) {
  if (g->len == g->cap) {
    g->cap = g->cap ? g->cap * 2 : 1024;
    g->eb = realloc(g->eb, g->cap * sizeof(int64_t));
    g->ex = realloc(g->ex, g->cap * sizeof(int64_t));
    g->ey = realloc(g->ey, g->cap * sizeof(int64_t));
    g->en = realloc(g->en, g->cap * sizeof(int64_t));
  }
  g->en[g->len] = head;
  g->eb[g->len] = i;
  g->ex[g->len] = cx;
  g->ey[g->len] = cy;
  g->len++;
}

static void grid_build(Grid *g, int n) {
  double cell = cell_size(n);
  g->inv = 1.0 / cell;
  double pad = cell * 0.1;
  int64_t size = 16;
  while (size < (int64_t)n * 2) size *= 2;
  g->mask = size - 1;
  free(g->heads);
  g->heads = malloc(size * sizeof(int64_t));
  for (int64_t k = 0; k < size; k++) g->heads[k] = -1;
  g->len = 0;
  for (int i = 0; i < n; i++) {
    const Body *b = &bodies[i];
    int64_t cx0 = cell_of(b->x - b->hw - pad, g->inv), cx1 = cell_of(b->x + b->hw + pad, g->inv);
    int64_t cy0 = cell_of(b->y - b->hh - pad, g->inv), cy1 = cell_of(b->y + b->hh + pad, g->inv);
    for (int64_t cy = cy0; cy < cy1 + 1; cy++)
      for (int64_t cx = cx0; cx < cx1 + 1; cx++) {
        int64_t slot = bucket(cx, cy, g->mask);
        grid_push(g, g->heads[slot], i, cx, cy);
        g->heads[slot] = g->len - 1;
      }
  }
}

static int cmp_i64(const void *a, const void *b) {
  int64_t x = *(const int64_t *)a, y = *(const int64_t *)b;
  return (x > y) - (x < y);
}

/* bodies j > i whose registered cells overlap body i's padded AABB: ascending, unique */
static int grid_candidates(const Grid *g, const Body *b, int64_t i, int64_t *mark, int64_t *out) {
  double pad = 0.1 / g->inv;
  int64_t cx0 = cell_of(b->x - b->hw - pad, g->inv), cx1 = cell_of(b->x + b->hw + pad, g->inv);
  int64_t cy0 = cell_of(b->y - b->hh - pad, g->inv), cy1 = cell_of(b->y + b->hh + pad, g->inv);
  int cnt = 0;
  for (int64_t cy = cy0; cy < cy1 + 1; cy++)
    for (int64_t cx = cx0; cx < cx1 + 1; cx++)
      for (int64_t e = g->heads[bucket(cx, cy, g->mask)]; e >= 0; e = g->en[e])
        if (g->ex[e] == cx && g->ey[e] == cy) {
          int64_t j = g->eb[e];
          if (j > i && mark[j] != i + 1) {
            mark[j] = i + 1;
            out[cnt++] = j;
          }
        }
  qsort(out, cnt, sizeof(int64_t), cmp_i64);
  return cnt;
}

static int solve_pair(int i, int j) {
  Body *a = &bodies[i], *b = &bodies[j];
  if (a->inv_mass + b->inv_mass > 0.0 && bounds_overlap(a, b)) {
    Contact c = collide(a, b);
    if (c.hit) {
      resolve(a, b, c);
      return 1;
    }
  }
  return 0;
}

static int64_t mark[N_BODIES];
static int64_t cands[N_BODIES];

static int solve_grid(Grid *g) {
  int n = nb;
  grid_build(g, n);
  memset(mark, 0, sizeof(mark));
  int last = 0;
  for (int i = 0; i < n; i++) {
    int cnt = grid_candidates(g, &bodies[i], i, mark, cands);
    for (int k = 0; k < cnt; k++) last += solve_pair(i, (int)cands[k]);
  }
  return last;
}

static void integrate(double dt, double gx, double gy) {
  double damp = 1.0 / (1.0 + dt * 0.0);
  for (int i = 0; i < nb; i++) {
    Body *b = &bodies[i];
    if (b->inv_mass > 0.0) {
      b->vx = (b->vx + gx * dt) * damp;
      b->vy = (b->vy + gy * dt) * damp;
      b->x = b->x + b->vx * dt;
      b->y = b->y + b->vy * dt;
    }
  }
}

static int64_t fl(double v) { return (int64_t)floor(v); }
static int64_t posmod(int64_t v) { return ((v % MOD) + MOD) % MOD; }

int main(void) {
  for (int k = 0; k < FLOOR_TILES; k++) add_box(1.0 + 2.0 * k, -1.0, 1.0, 1.0, 0.0, 0.0, 0.0);
  for (int k = 0; k < WALL_TILES; k++) add_box(-1.0, 1.0 + 2.0 * k, 1.0, 1.0, 0.0, 0.0, 0.0);
  for (int k = 0; k < WALL_TILES; k++) add_box(122.0, 1.0 + 2.0 * k, 1.0, 1.0, 0.0, 0.0, 0.0);
  for (int i = 0; i < N_DYN; i++) {
    int64_t col = i % COLS, row = i / COLS;
    int64_t kind = draw(3);
    double sw = 0.35 + (double)draw(400001) * 0.000001;
    double sh = 0.35 + (double)draw(400001) * 0.000001;
    double mass = 1.0 + (double)draw(4);
    double jx = (double)(draw(600001) - 300000) * 0.000001, jy = (double)(draw(600001) - 300000) * 0.000001;
    double vx = (double)(draw(8000001) - 4000000) * 0.000001, vy = (double)(draw(8000001) - 4000000) * 0.000001;
    double x = 0.75 + (double)col * 1.2 + jx, y = 1.75 + (double)row * 1.2 + jy;
    if (kind == 0) add_box(x, y, sw, sh, mass, vx, vy);
    else add_circle(x, y, sw, mass, vx, vy);
  }

  Grid g = {0};
  int64_t cs = 0;
  double dt = 1.0 / 60.0;
  for (int s = 0; s < STEPS; s++) {
    integrate(dt, 0.0, -9.81);
    int last = 0;
    for (int it = 0; it < ITERS; it++) last = solve_grid(&g);
    cs += (int64_t)last * (s + 1);
  }
  double ke = 0.0;
  int64_t h = 0;
  for (int i = 0; i < nb; i++) {
    const Body *b = &bodies[i];
    h = (h * 131 + posmod(fl(b->x * 1000.0))) % MOD;
    h = (h * 131 + posmod(fl(b->y * 1000.0))) % MOD;
    h = (h * 131 + posmod(fl(b->vx * 1000.0))) % MOD;
    h = (h * 131 + posmod(fl(b->vy * 1000.0))) % MOD;
    if (b->inv_mass > 0.0) ke += 0.5 * (b->vx * b->vx + b->vy * b->vy) / b->inv_mass;
  }
  cs += h + fl(ke * 10.0);
  printf("CHECKSUM=%lld\n", (long long)cs);
  return 0;
}
