//! An `if` expression used as an arithmetic or comparison operand keeps its value through the
//! peephole pass (no `LoadConst; Add` → `AddImm` fusion across the if/else merge point).

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

fn check(body: &str) {
    assert!(run_tests(&format!("test \"t\" {{\n{body}\n}}\n")) > 0);
}

#[test]
fn add_of_if_expression_both_edges() {
    check(
        "    ok, no = true, false\n    assert_eq(10 + (if ok { 1 } else { 0 }), 11)\n    assert_eq(10 + (if no { 1 } else { 0 }), 10)",
    );
}

#[test]
fn nested_checksum_shape() {
    check(
        "    ok = true\n    cs = 7\n    cs = (cs * 131 + (if ok { 1 } else { 0 })) % 1000\n    assert_eq(cs, 918)",
    );
}

#[test]
fn sub_and_mul_of_if_expression() {
    check(
        "    ok, no = true, false\n    assert_eq(20 - (if ok { 2 } else { 0 }), 18)\n    assert_eq(20 - (if no { 2 } else { 0 }), 20)\n    assert_eq(3 * (if ok { 4 } else { 2 }), 12)\n    assert_eq(3 * (if no { 4 } else { 2 }), 6)",
    );
}

#[test]
fn comparison_of_if_expression() {
    check(
        "    ok, no = true, false\n    #assert(1 < (if ok { 5 } else { 0 }))\n    #assert(!(1 < (if no { 5 } else { 0 })))",
    );
}

#[test]
fn bare_return_does_not_swallow_the_next_line() {
    let src = "Box = {\n    v: i64 = 0\n    touch(@self, early: bool) {\n        self.v = 1\n        if early {\n            return\n        }\n        self.v = 2\n    }\n    stop(@self) {\n        self.v = 7\n        return\n        // trailing comment\n        self.v = 9\n    }\n}\ntest \"t\" {\n    b = Box.{}\n    @b.touch(true)\n    assert_eq(b.v, 1)\n    @b.touch(false)\n    assert_eq(b.v, 2)\n    @b.stop()\n    assert_eq(b.v, 7)\n}\n";
    assert!(run_tests(src) > 0);
}

const GRADE: &str = "grade(n: i64) : i64 {\n    g = 0\n    if n > 90 {\n        g = 4\n    } else if n > 75 {\n        g = 3\n    } else if n > 50 {\n        g = 2\n    } else {\n        g = 1\n    }\n    g\n}\nsign(n: i64) : i64 => if n < 0 { -1 } else if n == 0 { 0 } else { 1 }\nclamp_hi(n: i64) : i64 {\n    if n > 10 {\n        return 10\n    } else if n > 5 {\n        return 5\n    }\n    n\n}\n";

#[test]
fn else_if_chains_take_exactly_one_branch() {
    let src = format!(
        "{GRADE}test \"t\" {{\n    assert_eq(grade(95), 4)\n    assert_eq(grade(80), 3)\n    assert_eq(grade(60), 2)\n    assert_eq(grade(10), 1)\n    assert_eq(clamp_hi(12), 10)\n    assert_eq(clamp_hi(7), 5)\n    assert_eq(clamp_hi(3), 3)\n}}\n"
    );
    assert!(run_tests(&src) > 0);
}

#[test]
fn else_if_works_as_a_value_and_nests_like_else_block_if() {
    let src = format!(
        "{GRADE}test \"t\" {{\n    assert_eq(sign(-4), -1)\n    assert_eq(sign(0), 0)\n    assert_eq(sign(9), 1)\n    a = if false {{ 1 }} else {{ if true {{ 2 }} else {{ 3 }} }}\n    b = if false {{ 1 }} else if true {{ 2 }} else {{ 3 }}\n    assert_eq(a, b)\n}}\n"
    );
    assert!(run_tests(&src) > 0);
}
