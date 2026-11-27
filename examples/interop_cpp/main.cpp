// C++20 host: embeds Super and exposes a real C++ library to Super via extern "C".
#include "damage_c.h"
#include "super.hpp"

#include <cmath>
#include <cstdio>
#include <span>
#include <vector>

struct Entity {
  int64_t id;
  float hp;
  float dps;
  float x;
  float y;
};

static int g_damage_calls = 0;

int main(int argc, char **argv) {
  const char *script = argc > 1 ? argv[1] : "src/combat.sr";
  try {
    super::Engine engine;

    std::vector<Entity> entities;
    entities.reserve(64);
    for (int i = 0; i < 64; ++i) {
      entities.push_back(Entity{
          .id = i,
          .hp = (i % 5 == 0) ? 10.0f : 100.0f,
          .dps = 12.0f,
          .x = float(i),
          .y = float(i) * 0.5f,
      });
    }

    engine.bind("host", "apply_damage",
                [&](int64_t id, float dmg) {
                  if (id >= 0 && id < (int64_t)entities.size()) {
                    entities[(size_t)id].hp -= dmg;
                    g_damage_calls++;
                  }
                });

    // Super → C++ library: wrap damage_apply_hit (updates hp by pointer semantics via returns).
    engine.bind("host", "damage_apply_hit",
                [](double hp, double armor, double raw) -> double {
                  float h = (float)hp;
                  float taken = damage_apply_hit(&h, (float)armor, (float)raw);
                  (void)taken;
                  return (double)h;
                });

    engine.bind_span("entities", std::span<Entity>(entities),
                     "id:i64,hp:f32,dps:f32,x:f32,y:f32");
    engine.load(script);

    float dps = engine.call<float>("combat", "compute_dps", 10.0f, 1.5f);
    if (std::fabs(dps - 15.0f) > 1e-4f) {
      std::fprintf(stderr, "FAIL compute_dps=%f\n", dps);
      return 1;
    }

    engine.call("combat", "apply_ai", 1.0);
    int64_t alive = engine.call<int64_t>("combat", "alive_count");
    if (alive <= 0 || alive > 64) {
      std::fprintf(stderr, "FAIL alive_count=%lld\n", (long long)alive);
      return 1;
    }
    if (g_damage_calls <= 0) {
      std::fprintf(stderr, "FAIL expected apply_damage calls\n");
      return 1;
    }

    // Super called into C++: 100 HP, 5 armor, 20 raw → remaining 85.
    double remaining = engine.call<double>("combat", "hit_through_cpp", 100.0, 5.0, 20.0);
    if (std::fabs(remaining - 85.0) > 1e-3) {
      std::fprintf(stderr, "FAIL Super→C++ hit remaining=%f\n", remaining);
      return 1;
    }

    std::printf("interop_cpp OK (dps=%.1f alive=%lld damage_calls=%d cpp_hp=%.1f)\n", dps,
                (long long)alive, g_damage_calls, remaining);
    return 0;
  } catch (const std::exception &ex) {
    std::fprintf(stderr, "interop_cpp error: %s\n", ex.what());
    return 1;
  }
}
