//! `obj.field[i] = v` (and deeper place paths) must write the copy-on-write array back
//! into the struct on the VM, exactly as the C23 backend stores through the field.

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

const TREE: &str = r#"
+Tree = {
    op: _
    c: _
    cnt: i64 = 0
    push(@self, op: i64, c: f64) : i64 {
        i = self.cnt
        self.op[i] = op
        self.c[i] = c
        self.cnt = i + 1
        i
    }
    bump(@self, i: i64) {
        self.c[i] += 1.0
    }
}
+fill(@s: Tree) {
    s.cnt = 0
    @s.push(3, 1.5)
    @s.push(4, 2.5)
}
+Forest = {
    t: Tree
}
"#;

#[test]
fn method_index_store_through_self_field_persists() {
    let src = format!(
        "{TREE}\ntest \"push\" {{\n    t = Tree.{{op: [0, 0, 0, 0], c: [0.0, 0.0, 0.0, 0.0]}}\n    @t.push(7, 0.5)\n    assert_eq(t.cnt, 1)\n    assert_eq(t.op[0], 7)\n    assert_eq(t.c[0], 0.5)\n}}\n"
    );
    assert!(run_tests(&src) > 0);
}

#[test]
fn forwarded_mut_param_index_store_persists() {
    let src = format!(
        "{TREE}\ntest \"fill\" {{\n    s = Tree.{{op: [0, 0, 0, 0], c: [0.0, 0.0, 0.0, 0.0]}}\n    fill(@s)\n    assert_eq(s.cnt, 2)\n    assert_eq(s.op[1], 4)\n    assert_eq(s.c[0], 1.5)\n}}\n"
    );
    assert!(run_tests(&src) > 0);
}

#[test]
fn compound_index_assign_through_field_persists() {
    let src = format!(
        "{TREE}\ntest \"bump\" {{\n    t = Tree.{{op: [0, 0, 0, 0], c: [1.0, 1.0, 1.0, 1.0]}}\n    @t.bump(2)\n    @t.bump(2)\n    assert_eq(t.c[2], 3.0)\n    assert_eq(t.c[1], 1.0)\n}}\n"
    );
    assert!(run_tests(&src) > 0);
}

#[test]
fn nested_field_path_index_store_persists() {
    let src = format!(
        "{TREE}\ntest \"nested\" {{\n    f = Forest.{{t: Tree.{{op: [0, 0, 0, 0], c: [0.0, 0.0, 0.0, 0.0]}}}}\n    f.t.op[3] = 9\n    f.t.c[1] += 2.0\n    assert_eq(f.t.op[3], 9)\n    assert_eq(f.t.c[1], 2.0)\n}}\n"
    );
    assert!(run_tests(&src) > 0);
}

const SCRATCH: &str = r#"
+bump_all(@z, by: i64) : i64 {
    for 0..3 { z[it] += by }
    z[0]
}
+Pad = {
    out: _
    rows: _
    run(@self) {
        bump_all(@self.out, 2)
        bump_all(@self.out, 3)
    }
}
+Grid = {
    pad: Pad
}
"#;

#[test]
fn mut_field_argument_writes_back_into_the_field() {
    let src = format!(
        "{SCRATCH}\ntest \"field arg\" {{\n    p = Pad.{{out: [1, 1, 1], rows: [[0, 0, 0]]}}\n    first = bump_all(@p.out, 4)\n    assert_eq(first, 5)\n    assert_eq(p.out[2], 5)\n}}\n"
    );
    assert!(run_tests(&src) > 0);
}

#[test]
fn method_forwarding_self_field_to_mut_param_persists() {
    let src = format!(
        "{SCRATCH}\ntest \"self field\" {{\n    p = Pad.{{out: [0, 0, 0], rows: [[0, 0, 0]]}}\n    @p.run()\n    assert_eq(p.out[0], 5)\n    assert_eq(p.out[1], 5)\n}}\n"
    );
    assert!(run_tests(&src) > 0);
}

#[test]
fn nested_field_and_index_mut_argument_persists() {
    let src = format!(
        "{SCRATCH}\ntest \"nested\" {{\n    g = Grid.{{pad: Pad.{{out: [0, 0, 0], rows: [[1, 1, 1], [2, 2, 2]]}}}}\n    bump_all(@g.pad.out, 7)\n    bump_all(@g.pad.rows[1], 1)\n    assert_eq(g.pad.out[1], 7)\n    assert_eq(g.pad.rows[1][2], 3)\n    assert_eq(g.pad.rows[0][2], 1)\n}}\n"
    );
    assert!(run_tests(&src) > 0);
}

#[test]
fn plain_local_index_store_still_works() {
    let src = "test \"local\" {\n    a = [0, 0, 0]\n    a[1] = 5\n    a[2] += 4\n    b, a[0] = 1, 8\n    assert_eq(a[0], 8)\n    assert_eq(a[1], 5)\n    assert_eq(a[2], 4)\n    assert_eq(b, 1)\n}\n";
    assert!(run_tests(src) > 0);
}

/// `w.bodies[i].f = v` — struct element field RMW through a field-held array (physics2d).
#[test]
fn struct_elem_field_assign_through_world_persists() {
    let src = r#"
+Body = { y: f64 = 0.0, vy: f64 = 0.0 }
+World = { bodies: _ = [] }
test "elem field" {
    w = World.{bodies: [Body.{y: 10.0, vy: 0.0}]}
    w.bodies[0].vy = -1.0
    w.bodies[0].y = w.bodies[0].y + w.bodies[0].vy * 0.01
    assert_eq(w.bodies[0].vy, -1.0)
    assert_eq(w.bodies[0].y, 9.99)
    w.bodies[0].vy += -0.5
    assert_eq(w.bodies[0].vy, -1.5)
}
"#;
    assert!(run_tests(src) > 0);
}

/// peer `for bodies |*b| { b.vx = … }` — `for @w.bodies { it.f = … }` must persist.
#[test]
fn for_at_struct_elem_field_assign_persists() {
    let src = r#"
+Body = { x: f64 = 0.0, vx: f64 = 0.0, inv_mass: f64 = 1.0 }
+World = { bodies: _ = [] }
test "for @ field" {
    w = World.{bodies: [Body.{x: 1.0, vx: 2.0, inv_mass: 1.0}, Body.{x: 0.0, vx: 0.0, inv_mass: 0.0}]}
    for @w.bodies {
        if it.inv_mass > 0.0 {
            it.vx = it.vx + 1.0
            it.x = it.x + it.vx
        }
    }
    assert_eq(w.bodies[0].vx, 3.0)
    assert_eq(w.bodies[0].x, 4.0)
    assert_eq(w.bodies[1].vx, 0.0)
}
"#;
    assert!(run_tests(src) > 0);
}
