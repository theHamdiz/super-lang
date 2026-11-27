//! Embed `stdlib/**/*.sr` into the frontend so `use std.*` resolves with zero
//! external file paths at runtime.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let stdlib = manifest_dir.join("../../stdlib");
    let stdlib = stdlib.canonicalize().unwrap_or(stdlib);
    println!("cargo:rerun-if-changed={}", stdlib.display());

    let mut files: Vec<(String, String)> = Vec::new();
    if stdlib.is_dir() {
        visit(&stdlib, &stdlib, &mut files);
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("embedded_stdlib.rs");
    let mut body = String::from(
        "/// Build-time embedded standard library sources (`path` → UTF-8 text).\n\
         pub static EMBEDDED_STDLIB: &[(&str, &str)] = &[\n",
    );
    for (rel, text) in &files {
        body.push_str(&format!("    ({rel:?}, {text:?}),\n"));
        println!("cargo:rerun-if-changed={}", stdlib.join(rel).display());
    }
    body.push_str("];\n\n");
    body.push_str(
        "pub fn embedded_stdlib_get(rel: &str) -> Option<&'static str> {\n\
         \x20   EMBEDDED_STDLIB.iter().find(|(k, _)| *k == rel).map(|(_, v)| *v)\n\
         }\n",
    );
    fs::write(&out, body).expect("write embedded_stdlib.rs");
}

fn visit(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for ent in rd.flatten() {
        let path = ent.path();
        if path.is_dir() {
            visit(root, &path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("sr") {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if let Ok(text) = fs::read_to_string(&path) {
                out.push((rel, text));
            }
        }
    }
}
