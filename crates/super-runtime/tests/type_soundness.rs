//! Negative type-safety tests: invalid programs must fail analysis with E0202.
//!
//! Lives in `super-runtime` (requested as the runtime verification crate) and
//! exercises the frontend `SemanticAnalyzer` end-to-end.

use super_core::{Diagnostics, Interner, SourceFile};
use super_frontend::lexer::Lexer;
use super_frontend::parser::Parser;
use super_frontend::sema::SemanticAnalyzer;

fn analyze(src: &str) -> Diagnostics {
    let mut interner = Interner::new();
    let mut diags = Diagnostics::new();
    let _file = SourceFile::new("neg.sr", src);
    let tokens = Lexer::new(src, &mut interner, &mut diags).tokenize();
    let mut parser = Parser::new(&tokens, src, &mut interner, &mut diags);
    let mut module = parser.parse_module();
    super_frontend::macros::expand_module(&mut module, &mut interner, &mut diags);
    let sema = SemanticAnalyzer::new(&mut interner, &mut diags);
    let _ = sema.analyze(&module);
    diags
}

fn msgs(d: &Diagnostics) -> Vec<String> {
    d.iter().map(|e| format!("{e}")).collect()
}

#[test]
fn rejects_str_to_i64_argument() {
    let d = analyze(
        r#"
take(n: i64) : i64 => n
main() { take("nope") }
"#,
    );
    assert!(d.has_errors());
    assert!(
        msgs(&d).iter().any(|m| m.contains("E0202")),
        "got {:?}",
        msgs(&d)
    );
}

#[test]
fn rejects_optional_without_or_unwrap() {
    let d = analyze(
        r#"
find() : i64? => none
main() {
    x: i64 = find()
}
"#,
    );
    assert!(d.has_errors());
    assert!(
        msgs(&d)
            .iter()
            .any(|m| m.contains("E0202") && m.contains("optional")),
        "got {:?}",
        msgs(&d)
    );
}

#[test]
fn rejects_fallible_without_or_unwrap() {
    let d = analyze(
        r#"
read() : i64! => 1
main() {
    x: i64 = read()
}
"#,
    );
    assert!(d.has_errors());
    assert!(
        msgs(&d).iter().any(|m| m.contains("E0202")),
        "got {:?}",
        msgs(&d)
    );
}

#[test]
fn rejects_wrong_struct_field_literal_type() {
    let d = analyze(
        r#"
+Pt = { x: i64 = 0, y: i64 = 0 }
main() {
    p: Pt = Pt.{x: "bad", y: 1}
}
"#,
    );
    assert!(d.has_errors());
    assert!(
        msgs(&d).iter().any(|m| m.contains("E0202")),
        "got {:?}",
        msgs(&d)
    );
}

#[test]
fn accepts_matching_i64_call() {
    let d = analyze(
        r#"
take(n: i64) : i64 => n
main() { take(42) }
"#,
    );
    assert!(!d.has_errors(), "got {:?}", msgs(&d));
}
