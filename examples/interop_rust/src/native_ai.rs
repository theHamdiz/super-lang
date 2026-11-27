//! Pure Rust helpers Super calls through `host.rarity_score` (not a `super` crate).

/// Toy rarity score used to show Super → Rust library calls.
#[inline]
pub fn rarity_score(energy: f64, id: i64) -> f64 {
    energy * 0.1 + (id as f64) * 0.01
}
