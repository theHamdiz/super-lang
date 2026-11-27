/* C17 host: embeds Super AND exposes a real C library (native/mathlib) to Super. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "mathlib.h"
#include "super.h"

#define N 100000

typedef struct {
  float x, y, vx, vy;
} Particle;

static int g_steps = 0;

static void on_step(void *user_data, void *args_buf, void *ret_buf) {
  (void)args_buf;
  (void)ret_buf;
  int *steps = (int *)user_data;
  (*steps)++;
}

/* Trampoline: packed f64,f64,f64 -> f64 calling mathlib_lerp. */
static void tramp_lerp(void *user_data, void *args_buf, void *ret_buf) {
  (void)user_data;
  const double *args = (const double *)args_buf;
  double out = mathlib_lerp(args[0], args[1], args[2]);
  *(double *)ret_buf = out;
}

static void tramp_clamp(void *user_data, void *args_buf, void *ret_buf) {
  (void)user_data;
  const double *args = (const double *)args_buf;
  double out = mathlib_clamp(args[0], args[1], args[2]);
  *(double *)ret_buf = out;
}

static int expect(int cond, const char *msg) {
  if (!cond) {
    fprintf(stderr, "FAIL: %s\n", msg);
    return 0;
  }
  return 1;
}

int main(int argc, char **argv) {
  const char *script = argc > 1 ? argv[1] : "src/physics.sr";
  Particle *particles = calloc(N, sizeof(Particle));
  if (!particles) {
    perror("calloc");
    return 1;
  }
  for (int i = 0; i < N; i++) {
    particles[i].x = (float)i;
    particles[i].y = 0.0f;
    particles[i].vx = 1.0f;
    particles[i].vy = 0.5f;
  }

  SuperEngine *eng = super_engine_new();
  if (!eng) {
    fprintf(stderr, "super_engine_new failed\n");
    return 1;
  }

  if (super_bind_fn(eng, "host", "on_step", on_step, &g_steps, "i64->void") != 0) {
    fprintf(stderr, "bind on_step: %s\n", super_last_error(eng));
    return 1;
  }
  /* Super → C library: bind real mathlib_* symbols. */
  if (super_bind_fn(eng, "host", "mathlib_lerp", tramp_lerp, NULL, "f64,f64,f64->f64") != 0) {
    fprintf(stderr, "bind mathlib_lerp: %s\n", super_last_error(eng));
    return 1;
  }
  if (super_bind_fn(eng, "host", "mathlib_clamp", tramp_clamp, NULL, "f64,f64,f64->f64") != 0) {
    fprintf(stderr, "bind mathlib_clamp: %s\n", super_last_error(eng));
    return 1;
  }
  if (super_bind_slice_ex(eng, "particles", particles, N, sizeof(Particle),
                          "x:f32,y:f32,vx:f32,vy:f32") != 0) {
    fprintf(stderr, "bind_slice: %s\n", super_last_error(eng));
    return 1;
  }
  if (super_load_file(eng, script) != 0) {
    fprintf(stderr, "load: %s\n", super_last_error(eng));
    return 1;
  }

  double dt = 1.0;
  if (super_call_sig(eng, "physics", "step", &dt, NULL, "f64->void") != 0) {
    fprintf(stderr, "call step: %s\n", super_last_error(eng));
    return 1;
  }

  int ok = 1;
  ok &= expect(particles[0].x > 0.9f && particles[0].x < 1.1f, "p0.x advanced");
  ok &= expect(g_steps >= 1, "host callback fired");

  /* Exercise Super → C: blend(0, 10, 0.25) == 2.5 via mathlib_lerp. */
  struct {
    double a, b, t;
  } blend_args = {0.0, 10.0, 0.25};
  double blended = 0.0;
  if (super_call_sig(eng, "physics", "blend", &blend_args, &blended, "f64,f64,f64->f64") != 0) {
    fprintf(stderr, "blend: %s\n", super_last_error(eng));
    return 1;
  }
  ok &= expect(blended > 2.49 && blended < 2.51, "Super called C mathlib_lerp");

  struct {
    double x, lo, hi;
  } clamp_args = {42.0, 0.0, 1.0};
  double clamped = 0.0;
  if (super_call_sig(eng, "physics", "limit", &clamp_args, &clamped, "f64,f64,f64->f64") != 0) {
    fprintf(stderr, "limit: %s\n", super_last_error(eng));
    return 1;
  }
  ok &= expect(clamped > 0.99 && clamped < 1.01, "Super called C mathlib_clamp");

  if (super_reload_module(eng, script) != 0) {
    fprintf(stderr, "reload: %s\n", super_last_error(eng));
    return 1;
  }
  if (super_call_sig(eng, "physics", "step", &dt, NULL, "f64->void") != 0) {
    fprintf(stderr, "call step after reload: %s\n", super_last_error(eng));
    return 1;
  }
  ok &= expect(g_steps >= 2, "callback after reload");

  int64_t count = 0;
  if (super_call_sig(eng, "physics", "particle_count", NULL, &count, "->i64") != 0) {
    fprintf(stderr, "particle_count: %s\n", super_last_error(eng));
    return 1;
  }
  ok &= expect(count == (int64_t)N, "particle_count matches N");

  super_engine_free(eng);
  free(particles);

  if (!ok) {
    fprintf(stderr, "interop_c FAILED\n");
    return 1;
  }
  printf("interop_c OK (N=%d, steps=%d, count=%lld, blend=%.2f, clamp=%.2f)\n", N, g_steps,
         (long long)count, blended, clamped);
  return 0;
}
