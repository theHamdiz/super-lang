//! `@recv.method()` binds like the other prefix operators: `@r.roll() == 1` is
//! `(@r.roll()) == 1`, so the in-place receiver write-back survives inside larger expressions.

use super_core::{Diagnostics, Interner, SourceFile};
use super_frontend::{Lexer, Parser, SemanticAnalyzer};
use super_runtime::{Compiler, StackPool, Vm};

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

const COUNTER: &str = r#"
+Counter = {
    s: i64 = 0
    roll(@self) : i64 {
        self.s = self.s + 1
        self.s
    }
    draw(@self, m: i64) : i64 => @self.roll() * 10 % m
}
"#;

fn check(body: &str) {
    let src = format!("{COUNTER}\ntest \"t\" {{\n{body}\n}}\n");
    assert!(run_tests(&src) > 0);
}

#[test]
fn comparison_operand_writes_back() {
    check("    r = Counter.{s: 0}\n    _b = @r.roll() == 1\n    assert_eq(r.s, 1)");
}

#[test]
fn if_expression_condition_writes_back() {
    check(
        "    r = Counter.{s: 0}\n    x = if @r.roll() == 1 { 10 } else { 20 }\n    assert_eq(x, 10)\n    assert_eq(r.s, 1)",
    );
}

#[test]
fn if_statement_condition_writes_back() {
    check("    r = Counter.{s: 0}\n    if @r.roll() > 0 { r.s += 100 }\n    assert_eq(r.s, 101)");
}

#[test]
fn arithmetic_operand_writes_back() {
    check(
        "    r = Counter.{s: 5}\n    v = @r.roll() / 2 % 2 + 1\n    assert_eq(v, 2)\n    assert_eq(r.s, 6)",
    );
}

#[test]
fn expression_bodied_forwarding_writes_back() {
    check(
        "    r = Counter.{s: 0}\n    a = @r.draw(7)\n    b = @r.draw(7)\n    assert_eq(a, 3)\n    assert_eq(b, 6)\n    assert_eq(r.s, 2)",
    );
}
