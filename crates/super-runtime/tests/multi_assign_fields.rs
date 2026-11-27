use super_core::{Diagnostics, Interner, SourceFile};
use super_frontend::{Lexer, Parser, SemanticAnalyzer};
use super_runtime::{Compiler, StackPool, Vm};

/// Runs every `test` block; returns the number of passing assertions.
fn run_tests(src: &str) -> usize {
    let source = SourceFile::new("t.sr", src);
    let mut interner = Interner::new();
    let mut diags = Diagnostics::new();
    let tokens = Lexer::new(&source.text, &mut interner, &mut diags).tokenize();
    let mut parser = Parser::new(&tokens, &source.text, &mut interner, &mut diags);
    let mut module = parser.parse_module();
    super_frontend::macros::expand_module(&mut module, &mut interner, &mut diags);
    assert!(!diags.has_errors(), "{diags:?}");
    let typed = SemanticAnalyzer::new(&mut interner, &mut diags).analyze(&module);
    let chunk = Compiler::new(&mut interner, &typed, false).compile_tests(&module);
    let mut stacks = StackPool::new(1 << 16);
    let mut vm = Vm::new(chunk, &mut stacks, &interner);
    vm.run().unwrap();
    vm.tests_passed()
}

const PREAMBLE: &str = r#"
+P = {
    x: f64 = 0.0
    y: f64 = 0.0
}
+Box2 = {
    p: P
    n: i64 = 0
}
+bump(@b: Box2, v: f64) {
    b.p.x, b.n = v, b.n + 1
}
"#;

#[test]
fn multi_assign_writes_struct_fields() {
    let src = format!(
        "{PREAMBLE}{}",
        r#"
test "fields" {
    p = P.{x: 0.0, y: 0.0}
    p.x, p.y = 1.0, 2.0
    assert_eq(p.x, 1.0)
    assert_eq(p.y, 2.0)
}
test "field swap evaluates rhs first" {
    p = P.{x: 3.0, y: 4.0}
    p.x, p.y = p.y, p.x
    assert_eq(p.x, 4.0)
    assert_eq(p.y, 3.0)
}
"#
    );
    assert_eq!(run_tests(&src), 4, "every assert_eq must pass");
}

#[test]
fn multi_assign_writes_nested_and_mixed_targets() {
    let src = format!(
        "{PREAMBLE}{}",
        r#"
test "nested through @param" {
    b = Box2.{p: P.{x: 0.0, y: 0.0}, n: 0}
    @b.bump(5.0)
    assert_eq(b.p.x, 5.0)
    assert_eq(b.n, 1)
}
test "mixed ident, index and field" {
    p = P.{x: 0.0, y: 0.0}
    a = [0, 0]
    k, a[1], p.y = 7, 8, 9.0
    assert_eq(k, 7)
    assert_eq(a[1], 8)
    assert_eq(p.y, 9.0)
    assert_eq(p.x, 0.0)
}
"#
    );
    assert_eq!(run_tests(&src), 6, "every assert_eq must pass");
}
