/* 11_symbolic_autodiff_ode — 20k random expressions: simplify + forward dual AD,
   then dual-RK4 sensitivities of a stiff-ish ODE */
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

enum { OP_CONST, OP_VAR, OP_ADD, OP_SUB, OP_MUL, OP_NEG, OP_SIN };
#define EXPRS 20000
#define POINTS 7
#define TRAJ 1024
#define STEPS 1000
#define MAXN 64

typedef struct { double v, d; } Dual;
static inline Dual dadd(Dual a, Dual b) { return (Dual){a.v + b.v, a.d + b.d}; }
static inline Dual dsub(Dual a, Dual b) { return (Dual){a.v - b.v, a.d - b.d}; }
static inline Dual dmul(Dual a, Dual b) { return (Dual){a.v * b.v, a.v * b.d + a.d * b.v}; }
static inline Dual dneg(Dual a) { return (Dual){-a.v, -a.d}; }
static inline Dual dscale(Dual a, double k) { return (Dual){a.v * k, a.d * k}; }
static inline Dual dshift(Dual a, double k) { return (Dual){a.v + k, a.d}; }
static inline Dual dsin(Dual a) { return (Dual){sin(a.v), cos(a.v) * a.d}; }

typedef struct {
  int64_t op[MAXN], a[MAXN], b[MAXN];
  double cv[MAXN];
  int64_t cnt;
} Tree;

static int64_t lcg(int64_t *s) {
  *s = (*s * 1103515245 + 12345) % 2147483648;
  return *s;
}

static double leaf_const(int64_t m) {
  switch (m) {
    case 0: return 0.0;
    case 1: return 1.0;
    case 2: return 2.0;
    case 3: return 0.5;
    default: return -1.0;
  }
}

static int64_t gen(Tree *t, int64_t *rng, int depth) {
  int64_t k = lcg(rng) / 256 % 10;
  if (k < 2 || depth == 0) {
    int64_t m = lcg(rng) / 256 % 8;
    int64_t n = t->cnt++;
    if (m < 3) { t->op[n] = OP_VAR; t->a[n] = 0; t->b[n] = 0; t->cv[n] = 0.0; }
    else { t->op[n] = OP_CONST; t->a[n] = 0; t->b[n] = 0; t->cv[n] = leaf_const(m - 3); }
    return n;
  }
  int64_t a = gen(t, rng, depth - 1);
  int64_t kind = OP_SIN;
  if (k == 8) kind = OP_NEG;
  if (k >= 5 && k < 8) kind = OP_MUL;
  if (k == 4) kind = OP_SUB;
  if (k < 4) kind = OP_ADD;
  int64_t b = 0;
  if (kind == OP_ADD || kind == OP_SUB || kind == OP_MUL) b = gen(t, rng, depth - 1);
  int64_t n = t->cnt++;
  t->op[n] = kind; t->a[n] = a; t->b[n] = b; t->cv[n] = 0.0;
  return n;
}

static int is_const(const Tree *s, int64_t i, double c) { return s->op[i] == OP_CONST && s->cv[i] == c; }

static int64_t push(Tree *s, int64_t kind, int64_t a, int64_t b, double c) {
  int64_t n = s->cnt++;
  s->op[n] = kind; s->a[n] = a; s->b[n] = b; s->cv[n] = c;
  return n;
}

static int64_t simp_node(Tree *s, int64_t kind, double cv, int64_t l, int64_t r) {
  switch (kind) {
    case OP_CONST: return push(s, OP_CONST, 0, 0, cv);
    case OP_VAR: return push(s, OP_VAR, 0, 0, 0.0);
    case OP_ADD:
      if (is_const(s, l, 0.0)) return r;
      if (is_const(s, r, 0.0)) return l;
      if (s->op[l] == OP_CONST && s->op[r] == OP_CONST) return push(s, OP_CONST, 0, 0, s->cv[l] + s->cv[r]);
      return push(s, OP_ADD, l, r, 0.0);
    case OP_SUB:
      if (is_const(s, r, 0.0)) return l;
      if (s->op[l] == OP_CONST && s->op[r] == OP_CONST) return push(s, OP_CONST, 0, 0, s->cv[l] - s->cv[r]);
      if (l == r) return push(s, OP_CONST, 0, 0, 0.0);
      return push(s, OP_SUB, l, r, 0.0);
    case OP_MUL:
      if (is_const(s, l, 0.0) || is_const(s, r, 0.0)) return push(s, OP_CONST, 0, 0, 0.0);
      if (is_const(s, l, 1.0)) return r;
      if (is_const(s, r, 1.0)) return l;
      if (s->op[l] == OP_CONST && s->op[r] == OP_CONST) return push(s, OP_CONST, 0, 0, s->cv[l] * s->cv[r]);
      return push(s, OP_MUL, l, r, 0.0);
    case OP_NEG:
      if (s->op[l] == OP_CONST) return push(s, OP_CONST, 0, 0, -s->cv[l]);
      if (s->op[l] == OP_NEG) return s->a[l];
      return push(s, OP_NEG, l, 0, 0.0);
    default:
      if (is_const(s, l, 0.0)) return push(s, OP_CONST, 0, 0, 0.0);
      return push(s, OP_SIN, l, 0, 0.0);
  }
}

static void simplify(const Tree *t, Tree *s, int64_t *rep) {
  s->cnt = 0;
  for (int64_t i = 0; i < t->cnt; i++)
    rep[i] = simp_node(s, t->op[i], t->cv[i], rep[t->a[i]], rep[t->b[i]]);
}

static void eval_dual(const Tree *t, double x, double *ev, double *ed) {
  for (int64_t i = 0; i < t->cnt; i++) {
    Dual a = {ev[t->a[i]], ed[t->a[i]]}, b = {ev[t->b[i]], ed[t->b[i]]}, r;
    switch (t->op[i]) {
      case OP_CONST: r = (Dual){t->cv[i], 0.0}; break;
      case OP_VAR: r = (Dual){x, 1.0}; break;
      case OP_ADD: r = dadd(a, b); break;
      case OP_SUB: r = dsub(a, b); break;
      case OP_MUL: r = dmul(a, b); break;
      case OP_NEG: r = dneg(a); break;
      default: r = dsin(a); break;
    }
    ev[i] = r.v; ed[i] = r.d;
  }
}

static inline Dual rhs(double t, Dual y, Dual lam) {
  double s = sin(t), c = cos(t);
  return dshift(dneg(dmul(lam, dsub(y, (Dual){s, 0.0}))), 0.5 * c);
}

static Dual rk4_final(double lam, double y0, double h, int steps) {
  Dual ld = {lam, 1.0}, y = {y0, 0.0};
  double hh = 0.5 * h, h6 = h / 6.0;
  for (int k = 0; k < steps; k++) {
    double t = (double)k * h;
    Dual k1 = rhs(t, y, ld);
    Dual k2 = rhs(t + hh, dadd(y, dscale(k1, hh)), ld);
    Dual k3 = rhs(t + hh, dadd(y, dscale(k2, hh)), ld);
    Dual k4 = rhs(t + h, dadd(y, dscale(k3, h)), ld);
    y = dadd(y, dscale(dadd(dadd(dadd(k1, dscale(k2, 2.0)), dscale(k3, 2.0)), k4), h6));
  }
  return y;
}

int main(void) {
  int64_t rng = 20240611;
  Tree t, s;
  int64_t rep[MAXN];
  double ev[MAXN], ed[MAXN], sev[MAXN], sed[MAXN];
  for (int i = 0; i < MAXN; i++) { ev[i] = ed[i] = sev[i] = sed[i] = 0.0; rep[i] = 0; t.op[i] = t.a[i] = t.b[i] = 0; t.cv[i] = 0.0; s = t; }
  int64_t orig_nodes = 0, simp_nodes = 0, mismatch = 0;
  double acc_v = 0.0, acc_d = 0.0;
  for (int e = 0; e < EXPRS; e++) {
    t.cnt = 0;
    gen(&t, &rng, 4);
    int64_t n = t.cnt;
    simplify(&t, &s, rep);
    int64_t m = s.cnt;
    orig_nodes += n;
    simp_nodes += m;
    for (int p = 0; p < POINTS; p++) {
      double x = (p - 3) / 4.0;
      eval_dual(&t, x, ev, ed);
      eval_dual(&s, x, sev, sed);
      double vo = ev[n - 1], dro = ed[n - 1], vs = sev[rep[n - 1]], ds = sed[rep[n - 1]];
      if (fabs(vo - vs) > 0.000000001 * (1.0 + fabs(vo))) mismatch++;
      if (fabs(dro - ds) > 0.000000001 * (1.0 + fabs(dro))) mismatch++;
      acc_v += vs;
      acc_d += ds;
    }
  }

  double sum_y = 0.0, sum_dy = 0.0;
  for (int j = 0; j < TRAJ; j++) {
    Dual r = rk4_final(10.0 + j * 0.25, 1.0, 0.002, STEPS);
    sum_y += r.v;
    sum_dy += r.d;
  }

  int64_t q = (int64_t)floor(acc_v * 10000.0);
  int64_t qd = (int64_t)floor(acc_d * 10000.0);
  int64_t y1 = (int64_t)floor(sum_y * 1000000.0);
  int64_t y2 = (int64_t)floor(sum_dy * 1000000000.0);
  int64_t cs = orig_nodes + simp_nodes * 7 + mismatch * 13 + q * 3 + qd * 5 + y1 * 11 + y2 * 17;
  printf("CHECKSUM=%lld\n", (long long)cs);
  return 0;
}
