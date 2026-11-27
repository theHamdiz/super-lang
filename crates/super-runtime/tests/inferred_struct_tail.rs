//! A line that starts with `.{…}` is a fresh inferred struct literal (typically the tail
//! value of a function), never a postfix continuation of the previous line's expression.

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
    assert!(!diags.has_errors(), "{diags:?}");
    let chunk = Compiler::new(&mut interner, &typed, false).compile_tests(&module);
    let mut stacks = StackPool::new(1 << 16);
    let mut vm = Vm::new(chunk, &mut stacks, &interner);
    vm.run().unwrap();
    vm.tests_passed()
}

const P: &str = r#"
+P = {
    a: i64
    b: i64
    c: i64
}
"#;

#[test]
fn tail_literal_after_binding_line_puns_later_fields() {
    let src = format!(
        "{P}\nmk() : P {{\n    a = 1\n    c = 3\n    .{{a, b: a + 1, c}}\n}}\ntest \"t\" {{\n    p = mk()\n    assert_eq(p.a, 1)\n    assert_eq(p.b, 2)\n    assert_eq(p.c, 3)\n}}\n"
    );
    assert!(run_tests(&src) > 0);
}

#[test]
fn tail_literal_after_call_line_is_not_a_continuation() {
    let src = format!(
        "{P}\nbump(x: i64) : i64 => x + 1\nmk() : P {{\n    a = bump(0)\n    c = bump(a)\n    .{{a, b: bump(c) - 1, c}}\n}}\ntest \"t\" {{\n    p = mk()\n    assert_eq(p.a, 1)\n    assert_eq(p.b, 2)\n    assert_eq(p.c, 2)\n}}\n"
    );
    assert!(run_tests(&src) > 0);
}

const D: &str = r#"
+D = {
    xs: _ = []
    hits: i64 = 7
    tag: str = "fresh"
    live: bool = true
    bump(@self) { self.hits += 1 }
    label(self) : str => "{self.tag}:{self.hits}"
}
"#;

#[test]
fn omitted_fields_of_inferred_literals_take_struct_defaults() {
    let src = format!(
        "{D}\ntail() : D {{\n    xs = [1, 2]\n    .{{xs}}\n}}\nbody() : D => .{{hits: 1}}\nret(n: i64) : D {{\n    return .{{hits: n}}\n}}\nmaybe(ok: bool) : D? {{\n    ensure ok else {{ return none }}\n    .{{tag: \"opt\"}}\n}}\ntest \"t\" {{\n    a = tail()\n    assert_eq(a.hits, 7)\n    assert_eq(a.tag, \"fresh\")\n    #assert(a.live)\n    assert_eq(body().tag, \"fresh\")\n    assert_eq(ret(3).hits, 3)\n    #assert(ret(3).live)\n    b: D = .{{tag: \"typed\"}}\n    assert_eq(b.hits, 7)\n    m = maybe(true) or tail()\n    assert_eq(m.tag, \"opt\")\n    assert_eq(m.hits, 7)\n}}\n"
    );
    assert!(run_tests(&src) > 0);
}

#[test]
fn inferred_literals_carry_their_struct_for_methods() {
    let src = format!(
        "{D}\nmk() : D => .{{xs: [0]}}\ntest \"t\" {{\n    d = mk()\n    @d.bump()\n    @d.bump()\n    assert_eq(d.hits, 9)\n    assert_eq(d.label(), \"fresh:9\")\n}}\n"
    );
    assert!(run_tests(&src) > 0);
}

#[test]
fn same_line_and_return_literals_still_work() {
    let src = format!(
        "{P}\nr() : P {{\n    a, c = 4, 6\n    return .{{a, b: 5, c}}\n}}\ne(a: i64) : P => .{{a, b: a, c: a}}\ntest \"t\" {{\n    assert_eq(r().b, 5)\n    assert_eq(r().c, 6)\n    assert_eq(e(7).c, 7)\n}}\n"
    );
    assert!(run_tests(&src) > 0);
}
