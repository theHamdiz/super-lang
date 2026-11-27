//! Data-oriented ECS world preserved across hot reloads.
//!
//! Float scalar components (`Value::Float`) are mirrored in dense SoA columns for
//! SIMD / JIT integration kernels while the sparse `HashMap` API stays compatible.

use std::collections::HashMap;
use super_runtime::Value;

#[derive(Clone, Debug, Default)]
struct F64Column {
    values: Vec<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct World {
    next_id: u64,
    /// component_name -> (entity_id -> value)
    components: HashMap<String, HashMap<u64, Value>>,
    /// Dense `f64` columns keyed by component name (entity id == index).
    f64_soa: HashMap<String, F64Column>,
}

/// `pos[i] += vel[i] * dt` over equal-length slices (systems peer SIMD fallback).
#[inline]
pub fn integrate_soa(pos: &mut [f64], vel: &[f64], dt: f64) {
    assert_eq!(pos.len(), vel.len());
    let n = pos.len();
    if n == 0 {
        return;
    }

    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("sse2") {
            unsafe {
                integrate_soa_sse2(pos, vel, dt);
                return;
            }
        }
    }

    integrate_soa_scalar(pos, vel, dt);
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn integrate_soa_sse2(pos: &mut [f64], vel: &[f64], dt: f64) {
    use std::arch::x86_64::*;

    let n = pos.len();
    let dt_v = _mm_set1_pd(dt);
    let mut i = 0;
    while i + 4 <= n {
        unsafe {
            let p = pos.as_mut_ptr().add(i);
            let v = vel.as_ptr().add(i);
            let pv = _mm_loadu_pd(p);
            let pv1 = _mm_loadu_pd(p.add(2));
            let vv = _mm_loadu_pd(v);
            let vv1 = _mm_loadu_pd(v.add(2));
            _mm_storeu_pd(p, _mm_add_pd(pv, _mm_mul_pd(vv, dt_v)));
            _mm_storeu_pd(p.add(2), _mm_add_pd(pv1, _mm_mul_pd(vv1, dt_v)));
        }
        i += 4;
    }
    if i < n {
        let tail = &mut pos[i..];
        let vt = &vel[i..];
        integrate_soa_scalar(tail, vt, dt);
    }
}

#[inline(always)]
fn integrate_soa_scalar(pos: &mut [f64], vel: &[f64], dt: f64) {
    for (p, v) in pos.iter_mut().zip(vel.iter()) {
        *p += *v * dt;
    }
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn spawn(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn insert(&mut self, entity: u64, component: &str, value: Value) {
        self.components
            .entry(component.to_owned())
            .or_default()
            .insert(entity, value.clone());

        if let Value::Float(f) = value {
            self.write_f64_soa(component, entity, f);
        } else if let Value::Struct { fields, .. } = &value
            && is_xy_float_struct(fields)
            && let (Some(Value::Float(x)), Some(Value::Float(y))) =
                (fields.get("x"), fields.get("y"))
        {
            self.write_f64_soa(&format!("{component}.x"), entity, *x);
            self.write_f64_soa(&format!("{component}.y"), entity, *y);
        }
    }

    fn write_f64_soa(&mut self, component: &str, entity: u64, value: f64) {
        let col = self.f64_soa.entry(component.to_owned()).or_default();
        let idx = entity as usize;
        if col.values.len() <= idx {
            col.values.resize(idx + 1, 0.0);
        }
        col.values[idx] = value;
    }

    pub fn get(&self, entity: u64, component: &str) -> Option<&Value> {
        self.components.get(component)?.get(&entity)
    }

    pub fn get_mut(&mut self, entity: u64, component: &str) -> Option<&mut Value> {
        self.components.get_mut(component)?.get_mut(&entity)
    }

    /// Mutable / immutable `f64` column pair for bulk kernels (e.g. Pos + Vel).
    pub fn soa_pair_mut(&mut self, a: &str, b: &str) -> Option<(&mut [f64], &[f64])> {
        if a == b {
            return None;
        }
        let len = self
            .f64_soa
            .get(a)?
            .values
            .len()
            .min(self.f64_soa.get(b)?.values.len());
        if len == 0 {
            return None;
        }
        let pos_ptr = self.f64_soa.get_mut(a)?.values.as_mut_ptr();
        let vel_ptr = self.f64_soa.get(b)?.values.as_ptr();
        Some(unsafe {
            (
                std::slice::from_raw_parts_mut(pos_ptr, len),
                std::slice::from_raw_parts(vel_ptr, len),
            )
        })
    }

    pub fn query2(&self, a: &str, b: &str) -> Vec<(u64, &Value, &Value)> {
        let Some(ca) = self.components.get(a) else {
            return vec![];
        };
        let Some(cb) = self.components.get(b) else {
            return vec![];
        };
        ca.iter()
            .filter_map(|(id, va)| cb.get(id).map(|vb| (*id, va, vb)))
            .collect()
    }

    pub fn to_value(&self) -> Value {
        let mut fields = HashMap::new();
        fields.insert("entities".into(), Value::Int(self.next_id as i64));
        Value::Struct {
            name: None,
            label: None,
            fields,
        }
    }

    /// Spatial grid helper for game collision.
    pub fn spatial_collide(ax: f64, ay: f64, ar: f64, bx: f64, by: f64, br: f64) -> bool {
        let dx = ax - bx;
        let dy = ay - by;
        let r = ar + br;
        dx * dx + dy * dy <= r * r
    }
}

fn is_xy_float_struct(fields: &HashMap<String, Value>) -> bool {
    matches!(fields.get("x"), Some(Value::Float(_)))
        && matches!(fields.get("y"), Some(Value::Float(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_and_collision() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(
            e,
            "Pos",
            Value::Struct {
                name: None,
                label: None,
                fields: {
                    let mut m = HashMap::new();
                    m.insert("x".into(), Value::Float(0.0));
                    m.insert("y".into(), Value::Float(0.0));
                    m
                },
            },
        );
        assert!(World::spatial_collide(0.0, 0.0, 1.0, 0.5, 0.0, 1.0));
        assert!(!World::spatial_collide(0.0, 0.0, 1.0, 5.0, 0.0, 1.0));
    }

    #[test]
    fn soa_float_integrate() {
        let mut w = World::new();
        for i in 0..4 {
            let e = w.spawn();
            w.insert(e, "Pos", Value::Float(i as f64));
            w.insert(e, "Vel", Value::Float(1.0));
        }
        let (pos, vel) = w.soa_pair_mut("Pos", "Vel").expect("soa");
        integrate_soa(pos, vel, 0.5);
        assert!((pos[0] - 0.5).abs() < 1e-9);
        assert!((pos[3] - 3.5).abs() < 1e-9);
    }

    #[test]
    fn get_after_float_insert() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, "Vel", Value::Float(2.0));
        assert_eq!(w.get(e, "Vel").unwrap().as_float(), 2.0);
    }
}
