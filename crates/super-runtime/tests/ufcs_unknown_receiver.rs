//! `recv.method(args)` on a receiver whose static type is unknown must pick the
//! method whose arity counts the receiver, not an earlier same-named method that
//! only matches the bare argument count (stdlib `Duration.add(self, d)` shadowed
//! user `Agg.add(self, qty, price)` this way).

use super_core::{Diagnostics, Interner, SourceFile};
use super_frontend::{Lexer, Parser, SemanticAnalyzer};
use super_runtime::{Compiler, StackPool, Vm};

fn run_main(src: &str) -> Vec<String> {
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
    let chunk = Compiler::new(&mut interner, &typed, false).compile(&module);
    let mut stacks = StackPool::new(1 << 16);
    let mut vm = Vm::new(chunk, &mut stacks, &interner);
    vm.run().expect("run");
    vm.output().to_vec()
}

const TYPES: &str = "Clock: Add = {
    ns: i64 = 0
    add(self, d: Clock) : Clock => .{ns: self.ns + d.ns}
}
Stamp: Add = {
    at: i64 = 0
    add(self, d: Stamp) : Stamp => .{at: self.at + d.at * 1000}
}
Agg = {
    n: i64 = 0
    qty: i64 = 0
    add(self, qty: i64, price: i64) : Agg => .{n: self.n + 1, qty: self.qty + qty * price}
}
bump(xs) {
    xs[1] = xs[1].add(2, 5)
    xs[1] = xs[1].add(1, 4)
    xs
}
";

#[test]
fn method_on_untyped_receiver_ignores_shorter_same_named_methods() {
    let out = run_main(&format!(
        "{TYPES}main() {{\n    xs = bump([Agg.{{}}, Agg.{{}}])\n    print(\"{{xs[1].n}} {{xs[1].qty}} {{xs[0].n}}\")\n}}\n"
    ));
    assert_eq!(out, ["2 14 0"]);
}

#[test]
fn typed_receivers_keep_their_own_same_named_methods() {
    let out = run_main(&format!(
        "{TYPES}main() {{\n    c = Clock.{{ns: 5}}.add(Clock.{{ns: 7}})\n    s = Stamp.{{}}.add(Stamp.{{at: 2}})\n    a = Agg.{{}}.add(3, 3)\n    print(\"{{c.ns}} {{s.at}} {{a.qty}}\")\n}}\n"
    ));
    assert_eq!(out, ["12 2000 9"]);
}
