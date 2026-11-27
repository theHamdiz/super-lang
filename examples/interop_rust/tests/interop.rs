use std::path::PathBuf;
use super_ffi::{Engine, Value};

#[repr(C)]
struct Agent {
    id: i64,
    energy: f64,
    x: f64,
    y: f64,
}

#[test]
fn rust_host_zero_copy_and_callback() {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/ai.sr");
    let mut agents = vec![
        Agent {
            id: 0,
            energy: 0.2,
            x: 0.0,
            y: 0.0,
        },
        Agent {
            id: 1,
            energy: 8.0,
            x: 1.0,
            y: 0.0,
        },
    ];
    let mut engine = Engine::new();
    let ptr = agents.as_mut_ptr() as usize;
    let len = agents.len();
    engine.bind_fn_rust("host", "refuel", move |id: Value, amount: Value| {
        let i = id.as_int() as usize;
        if i < len {
            let slice = unsafe { std::slice::from_raw_parts_mut(ptr as *mut Agent, len) };
            slice[i].energy += amount.as_float();
        }
        Value::Null
    });
    engine.bind_slice_mut("agents", &mut agents, "id:i64,energy:f64,x:f64,y:f64");
    engine.load_file(&script).unwrap();
    let n = engine
        .call_values("", "tick", vec![Value::Float(1.0)])
        .unwrap();
    assert!(n.as_int() >= 1);
    assert!(agents[0].x > 0.5);
    assert!(agents[0].energy > 1.0);
}
