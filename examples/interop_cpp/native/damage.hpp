#pragma once
#include <cstdint>

namespace game {
struct Stats {
  float hp;
  float armor;
};
float apply_hit(Stats &s, float raw_damage);
}
