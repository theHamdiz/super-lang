//! `x.len` is the intrinsic length for arrays / strings, but a struct that declares its own
//! `len` field reads that field.

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

const LOG: &str = r#"
+Log = {
    items: _ = []
    len: i64 = 0
    push(@self, v: i64) {
        self.items[self.len] = v
        self.len += 1
    }
    last(self) : i64 => self.items[self.len - 1]
}
"#;

#[test]
fn struct_len_field_is_a_field_not_the_intrinsic() {
    let src = format!(
        "{LOG}\ntest \"t\" {{\n    log = Log{{items: [0, 0, 0, 0]}}\n    @log.push(5)\n    @log.push(9)\n    assert_eq(log.len, 2)\n    assert_eq(log.last(), 9)\n    assert_eq(log.items.len, 4)\n}}\n"
    );
    assert!(run_tests(&src) > 0);
}

#[test]
fn array_and_string_len_stay_intrinsic() {
    // `str.len` is the UTF-8 byte length.
    let src = "test \"t\" {\n    xs = [1, 2, 3]\n    s = \"héllo\"\n    assert_eq(xs.len, 3)\n    assert_eq(s.len, 6)\n    assert_eq([].len, 0)\n}\n";
    assert!(run_tests(src) > 0);
}

#[test]
fn len_field_survives_passing_the_struct_around() {
    let src = format!(
        "{LOG}\nsize(l: Log) : i64 => l.len\nfill(@l: Log, n: i64) {{\n    for 0..n {{ @l.push(it) }}\n}}\ntest \"t\" {{\n    log = Log{{items: [0, 0, 0, 0, 0, 0]}}\n    fill(@log, 3)\n    assert_eq(size(log), 3)\n    assert_eq(log.last(), 2)\n}}\n"
    );
    assert!(run_tests(&src) > 0);
}

#[test]
fn len_inside_string_interpolation_uses_the_intrinsic_unless_the_struct_declares_it() {
    let src = "Buf = {\n    len: i64 = 0\n}\ntest \"t\" {\n    body = \"abcd\"\n    xs = [1, 2, 3]\n    b = Buf.{len: 9}\n    assert_eq(\"n={body.len} m={xs.len} k={b.len}\", \"n=4 m=3 k=9\")\n}\n";
    assert!(run_tests(src) > 0);
}
