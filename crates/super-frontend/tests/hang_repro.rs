//! Regression: comma-separated struct fields must not hang the parser.

use super_core::{Diagnostics, Interner, SourceFile};
use super_frontend::analyze;
use super_frontend::lexer::Lexer;
use super_frontend::parser::Parser;
use super_frontend::sema::SemanticAnalyzer;

#[test]
fn comma_separated_struct_fields_parse() {
    let src = "+Foo = { a: f64 = 0.0, b: f64 = 0.0 }\n";
    let mut interner = Interner::new();
    let mut diags = Diagnostics::new();
    let tokens = Lexer::new(src, &mut interner, &mut diags).tokenize();
    let mut parser = Parser::new(&tokens, src, &mut interner, &mut diags);
    let module = parser.parse_module();
    assert_eq!(module.items.len(), 1, "expected one struct item");
    assert!(
        !diags.has_errors(),
        "parse errors: {:?}",
        diags.iter().map(|d| d.message.clone()).collect::<Vec<_>>()
    );
    let typed = SemanticAnalyzer::new(&mut interner, &mut diags).analyze(&module);
    assert!(!typed.structs.is_empty());
}

#[test]
fn analyze_helper_comma_fields() {
    let file = SourceFile::new("t.sr", "+Foo = { a: f64 = 0.0, b: f64 = 0.0 }\n");
    let r = analyze(&file);
    assert!(
        !r.diagnostics.has_errors(),
        "{:?}",
        r.diagnostics
            .iter()
            .map(|d| d.message.clone())
            .collect::<Vec<_>>()
    );
}
