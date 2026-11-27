//! Implicit last-expression return for functions and methods.

use super_core::{Diagnostics, Interner, SourceFile};
use super_frontend::{Lexer, Parser, SemanticAnalyzer};
use super_runtime::{Compiler, StackPool, Vm};

fn run_tests(src: &str) -> usize {
    let source = SourceFile::new("implicit_return.sr", src);
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
    vm.run().expect("vm");
    vm.tests_passed()
}

#[test]
fn last_expr_is_implicit_return() {
    let passed = run_tests(
        r#"
+add(a: i64, b: i64) : i64 {
    a + b
}

+Enemy = {
    hp: i64 = 3
    remaining(self) : i64 {
        self.hp
    }
}

test "fn last expr" {
    assert_eq(add(2, 3), 5)
}

test "method last expr" {
    e = Enemy{hp: 7}
    assert_eq(e.remaining(), 7)
}
"#,
    );
    assert!(passed >= 2, "passed={passed}");
}

#[test]
fn explicit_return_still_works() {
    let passed = run_tests(
        r#"
+pick(flag: bool) : i64 {
    ensure flag else { return 0 }
    42
}

test "mixed ensure + implicit" {
    assert_eq(pick(true), 42)
    assert_eq(pick(false), 0)
}
"#,
    );
    assert!(passed >= 1, "passed={passed}");
}

#[test]
fn spawn_block_last_expr() {
    let passed = run_tests(
        r#"
+run() : i64 {
    scope {
        t = spawn {
            n = 1 + 2
            n
        }
        await t
    }
}

test "spawn implicit return" {
    assert_eq(run(), 3)
}
"#,
    );
    assert!(passed >= 1, "passed={passed}");
}
