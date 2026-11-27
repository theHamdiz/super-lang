//! Rust host: `bind_fn`, `bind_slice_mut`, bidirectional calls with `#[repr(C)]`.
//!
//! Import the embed crate as `super_ffi` — never name a package `super` (reserved keyword).

mod native_ai;

use std::path::PathBuf;
use super_ffi::{Engine, Value};

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Agent {
    id: i64,
    energy: f64,
    x: f64,
    y: f64,
}

fn main() {
    let script = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/ai.sr"));

    let mut agents: Vec<Agent> = (0..32)
        .map(|i| Agent {
            id: i,
            energy: if i % 4 == 0 { 0.5 } else { 10.0 },
            x: i as f64,
            y: 0.0,
        })
        .collect();

    let mut engine = Engine::new();
    let agents_ptr = agents.as_mut_ptr() as usize;
    let agents_len = agents.len();
    engine.bind_fn_rust("host", "refuel", move |id: Value, amount: Value| {
        let i = id.as_int() as usize;
        let amt = amount.as_float();
        if i < agents_len {
            // SAFETY: agents buffer lives for the process; host owns the memory.
            let slice = unsafe { std::slice::from_raw_parts_mut(agents_ptr as *mut Agent, agents_len) };
            slice[i].energy += amt;
        }
        Value::Null
    });
    // Super → Rust library module.
    engine.bind_fn_rust("host", "rarity_score", |energy: Value, id: Value| {
        Value::Float(native_ai::rarity_score(energy.as_float(), id.as_int()))
    });
    engine.bind_slice_mut("agents", &mut agents, "id:i64,energy:f64,x:f64,y:f64");
    engine.load_file(&script).expect("load");

    let refueled = engine
        .call_values("", "tick", vec![Value::Float(1.0)])
        .expect("tick");
    assert!(refueled.as_int() >= 1, "expected at least one refuel");

    let energy = engine
        .call_values("", "sum_energy", vec![])
        .expect("sum_energy");
    assert!(energy.as_float() > 0.0);

    let score = engine
        .call_values(
            "",
            "score_agent",
            vec![Value::Float(10.0), Value::Int(3)],
        )
        .expect("score_agent");
    let expect = native_ai::rarity_score(10.0, 3);
    assert!((score.as_float() - expect).abs() < 1e-9, "Super→Rust rarity_score");

    assert!(agents[0].x > 0.9 && agents[0].x < 1.1);
    assert!(agents[0].energy > 0.5);

    println!(
        "interop_rust OK (refueled={}, energy={:.1}, agent0.x={:.1}, rarity={:.3})",
        refueled.as_int(),
        energy.as_float(),
        agents[0].x,
        score.as_float()
    );
}
