use super_core::{Diagnostics, Interner, SourceFile};
use super_frontend::{Lexer, Parser, SemanticAnalyzer};
use super_runtime::{Compiler, StackPool, Vm};

#[test]
fn field_assignment_updates_local() {
    let src = r#"
+Enemy = {
    hp: i64 = 3
    score: i64 = 100
}
+apply(@enemy: Enemy, amount: i64) : i64 {
    enemy.hp = enemy.hp - amount
    return enemy.hp
}
test "t" {
    e = Enemy{hp: 3, score: 100}
    v = @e.apply(1)
    assert_eq(v, 2)
    assert_eq(e.hp, 2)
}
"#;
    let source = SourceFile::new("t.sr", src);
    let mut interner = Interner::new();
    let mut diags = Diagnostics::new();
    let tokens = Lexer::new(&source.text, &mut interner, &mut diags).tokenize();
    let mut parser = Parser::new(&tokens, &source.text, &mut interner, &mut diags);
    let mut module = parser.parse_module();
    super_frontend::macros::expand_module(&mut module, &mut interner, &mut diags);
    assert!(!diags.has_errors(), "{:?}", diags);
    let typed = SemanticAnalyzer::new(&mut interner, &mut diags).analyze(&module);
    let chunk = Compiler::new(&mut interner, &typed, false).compile_tests(&module);
    for (i, op) in chunk.ops.iter().enumerate() {
        println!("{i}: {op:?}");
    }
    let mut stacks = StackPool::new(1 << 16);
    let mut vm = Vm::new(chunk, &mut stacks, &interner);
    let r = vm.run();
    println!("result={r:?} passed={}", vm.tests_passed());
    r.unwrap();
    assert!(vm.tests_passed() >= 1);
}
