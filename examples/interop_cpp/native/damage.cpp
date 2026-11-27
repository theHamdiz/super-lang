#include "damage.hpp"

namespace game {
float apply_hit(Stats &s, float raw_damage) {
  float taken = raw_damage - s.armor;
  if (taken < 0.0f) taken = 0.0f;
  s.hp -= taken;
  if (s.hp < 0.0f) s.hp = 0.0f;
  return taken;
}
}

extern "C" float damage_apply_hit(float *hp, float armor, float raw) {
  game::Stats s{*hp, armor};
  float taken = game::apply_hit(s, raw);
  *hp = s.hp;
  return taken;
}
