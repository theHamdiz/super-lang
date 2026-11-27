//! Build-time template diagnostic codes T001–T004 (one test per code).

use super_core::SourceFile;
use super_core::err;
use super_frontend::analyze;

fn analyze_src(src: &str) -> super_frontend::AnalyzeResult {
    let file = SourceFile::new("t.sr", src.to_string());
    analyze(&file)
}

#[test]
fn build_t001_syntax() {
    let r = analyze_src(r#"t = compile_html("{{name")"#);
    assert!(
        r.diagnostics.iter().any(|d| d.code == Some(err::T001)),
        "expected T001, got {:?}",
        r.diagnostics
            .iter()
            .map(|d| (d.code, d.message.clone()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn build_t002_unknown_name() {
    let r = analyze_src(r#"t = compile_text("{{name}}", ["title"])"#);
    assert!(
        r.diagnostics.iter().any(|d| d.code == Some(err::T002)),
        "expected T002, got {:?}",
        r.diagnostics
            .iter()
            .map(|d| (d.code, d.message.clone()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn build_t002_unknown_partial() {
    let r = analyze_src(r#"t = compile_text("{{> card}}", ["x"], ["other"])"#);
    assert!(
        r.diagnostics.iter().any(|d| d.code == Some(err::T002)),
        "expected T002 for unknown partial, got {:?}",
        r.diagnostics
            .iter()
            .map(|d| (d.code, d.message.clone()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn build_t003_not_renderable() {
    let r = analyze_src(r#"t = compile_text("{{blob}}", ["!blob"])"#);
    assert!(
        r.diagnostics.iter().any(|d| d.code == Some(err::T003)),
        "expected T003, got {:?}",
        r.diagnostics
            .iter()
            .map(|d| (d.code, d.message.clone()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn build_t004_unsafe_context() {
    let r = analyze_src(r#"t = compile_html("<script>{{x}}</script>")"#);
    assert!(
        r.diagnostics.iter().any(|d| d.code == Some(err::T004)),
        "expected T004, got {:?}",
        r.diagnostics
            .iter()
            .map(|d| (d.code, d.message.clone()))
            .collect::<Vec<_>>()
    );
}
