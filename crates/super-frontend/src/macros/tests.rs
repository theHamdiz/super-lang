use super::*;
use crate::ast::{Binding, Block, Expr, FnBody, Item, Stmt, UnOp};

fn expand(src: &str, extra_sources: &[&str]) -> (Module, Diagnostics, Interner) {
    let mut interner = Interner::with_capacity(256);
    let mut diagnostics = Diagnostics::new();
    let tokens = Lexer::new(src, &mut interner, &mut diagnostics).tokenize();
    let mut module = Parser::new(&tokens, src, &mut interner, &mut diagnostics).parse_module();
    let mut extra = prelude_macros(&mut interner);
    for source in extra_sources {
        extra.extend(macros_in_source(source, &mut interner));
    }
    expand_module_with(&mut module, &extra, &mut interner, &mut diagnostics);
    (module, diagnostics, interner)
}

fn http_source() -> &'static str {
    crate::modules::embedded_stdlib_get("net/http.sr").expect("embedded net/http.sr")
}

fn messages(diagnostics: Diagnostics) -> Vec<String> {
    diagnostics
        .into_vec()
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect()
}

fn function_body(module: &Module, index: usize) -> &FnBody {
    let Item::Function(function) = &module.items[index] else {
        panic!("expected function, got {:?}", module.items[index]);
    };
    &function.body
}

fn expr_block(body: &FnBody) -> &Block {
    let FnBody::Expr(Expr::Block(block)) = body else {
        panic!("expected expanded block, got {body:?}");
    };
    block
}

fn block_body(body: &FnBody) -> &Block {
    let FnBody::Block(block) = body else {
        panic!("expected block body, got {body:?}");
    };
    block
}

fn strings_in(stmt: &Stmt, interner: &Interner) -> Vec<String> {
    let text = format!("{stmt:?}");
    let mut out = Vec::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find("String(SymId(") {
        rest = &rest[at + "String(SymId(".len()..];
        let end = rest.find(')').expect("closing paren");
        let id: u32 = rest[..end].parse().expect("symbol id");
        let sym = SymId::from_index(id).expect("symbol index");
        out.push(interner.resolve(sym).to_owned());
    }
    out
}

#[test]
fn routes_expands_to_plain_app_builder_calls() {
    let (module, diagnostics, interner) = expand(
        "+build() => #routes {\n    get \"/health\" { ok(\"up\") }\n    post \"/echo\" |req| { ok(req.body) }\n}\n",
        &[http_source()],
    );
    assert!(!diagnostics.has_errors(), "{:?}", messages(diagnostics));
    let block = expr_block(function_body(&module, 0));
    assert_eq!(block.stmts.len(), 4, "{:#?}", block.stmts);
    let Stmt::Binding(Binding {
        name: app, value, ..
    }) = &block.stmts[0]
    else {
        panic!("expected `app = App.new()`");
    };
    assert!(interner.resolve(*app).starts_with("__app_"));
    assert!(format!("{value:?}").contains("Pipeline"), "{value:?}");
    let mut methods = Vec::new();
    for statement in &block.stmts[1..3] {
        let Stmt::Binding(Binding { name, value, .. }) = statement else {
            panic!("expected rebinding of the builder, got {statement:?}");
        };
        assert_eq!(name, app);
        let Expr::Unary {
            op: UnOp::MutRef,
            expr,
            ..
        } = value
        else {
            panic!("expected `@app.verb(…)`");
        };
        let Expr::Pipeline { method, args, .. } = expr.as_ref() else {
            panic!("expected builder pipeline, got {expr:?}");
        };
        methods.push(interner.resolve(*method));
        assert_eq!(args.len(), 2);
        assert!(matches!(args[0], Expr::String(..)));
        assert!(matches!(args[1], Expr::Lambda { .. }));
    }
    assert_eq!(methods, ["get", "post"]);
    assert!(matches!(&block.stmts[3], Stmt::Expr(Expr::Ident(name, _)) if name == app));
}

#[test]
fn routes_rejects_duplicate_method_and_path() {
    let (_, diagnostics, _) = expand(
        "+build() => #routes {\n    get \"/health\" { ok(\"up\") }\n    get \"/health\" { ok(\"still up\") }\n}\n",
        &[http_source()],
    );
    let messages = messages(diagnostics);
    assert!(
        messages
            .iter()
            .any(|message| message.contains("duplicate route `GET /health`")),
        "{messages:?}"
    );
}

#[test]
fn routes_allows_same_path_under_different_methods() {
    let (_, diagnostics, _) = expand(
        "+build() => #routes {\n    get \"/x\" { ok(1) }\n    post \"/x\" { ok(2) }\n}\n",
        &[http_source()],
    );
    assert!(!diagnostics.has_errors(), "{:?}", messages(diagnostics));
}

#[test]
fn routes_rejects_unknown_verbs() {
    let (_, diagnostics, _) = expand(
        "+build() => #routes {\n    fetch \"/x\" { ok(1) }\n}\n",
        &[http_source()],
    );
    let messages = messages(diagnostics);
    assert!(
        messages
            .iter()
            .any(|message| message.contains("unknown route method `fetch` in `#routes`")),
        "{messages:?}"
    );
}

#[test]
fn routes_rejects_malformed_entries() {
    let (_, diagnostics, _) = expand(
        "+build() => #routes {\n    get { ok(1) }\n}\n",
        &[http_source()],
    );
    let messages = messages(diagnostics);
    assert!(
        messages
            .iter()
            .any(|message| message.contains("`#routes` entries are `verb \"/path\" handler`")),
        "{messages:?}"
    );
}

#[test]
fn routes_rejects_non_closure_handlers() {
    let (_, diagnostics, _) = expand(
        "+build() => #routes {\n    get \"/x\" \"text\"\n}\n",
        &[http_source()],
    );
    let messages = messages(diagnostics);
    assert!(
        messages
            .iter()
            .any(|message| message.contains("the handler is a `{ block }` or a `|req| …` lambda")),
        "{messages:?}"
    );
}

#[test]
fn routes_is_an_ordinary_name_without_the_hash() {
    let (module, diagnostics, interner) = expand(
        "+count() : i64 {\n    routes = [1, 2, 3]\n    len(routes)\n}\n",
        &[http_source()],
    );
    assert!(!diagnostics.has_errors(), "{:?}", messages(diagnostics));
    let block = block_body(function_body(&module, 0));
    let Stmt::Binding(Binding { name, .. }) = &block.stmts[0] else {
        panic!("expected binding");
    };
    assert_eq!(interner.resolve(*name), "routes");
}

#[test]
fn assert_captures_expression_text_and_location() {
    let (module, diagnostics, interner) = expand(
        "+probe(body: str) {\n    #assert(contains(body, \"x\"))\n}\n",
        &[],
    );
    assert!(!diagnostics.has_errors(), "{:?}", messages(diagnostics));
    let expansion = block_body(function_body(&module, 0));
    let strings: Vec<String> = expansion
        .stmts
        .iter()
        .flat_map(|stmt| strings_in(stmt, &interner))
        .collect();
    let joined = strings.join("|");
    assert!(joined.contains("2:5"), "{joined}");
    assert!(joined.contains(r#"contains(body, \"x\")"#), "{joined}");
}

#[test]
fn assert_binary_operands_are_evaluated_once() {
    let (module, diagnostics, interner) =
        expand("+probe(a: i64, b: i64) {\n    #assert(a == b)\n}\n", &[]);
    assert!(!diagnostics.has_errors(), "{:?}", messages(diagnostics));
    let expansion = block_body(function_body(&module, 0));
    let bound: Vec<&str> = expansion
        .stmts
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::Binding(binding) => Some(interner.resolve(binding.name)),
            _ => None,
        })
        .collect();
    assert_eq!(bound.len(), 3, "{bound:?}");
    assert!(bound.iter().all(|name| name.starts_with("__")), "{bound:?}");
    assert!(
        expansion
            .stmts
            .iter()
            .any(|stmt| matches!(stmt, Stmt::Ensure { .. }))
    );
}

#[test]
fn dbg_is_hygienic_per_expansion() {
    let (module, diagnostics, interner) = expand(
        "+probe(v: i64) : i64 {\n    a = #dbg(v)\n    b = #dbg(v + 1)\n    a + b\n}\n",
        &[],
    );
    assert!(!diagnostics.has_errors(), "{:?}", messages(diagnostics));
    let block = block_body(function_body(&module, 0));
    let mut temps = Vec::new();
    for stmt in &block.stmts[..2] {
        let Stmt::Binding(Binding {
            value: Expr::Block(inner),
            ..
        }) = stmt
        else {
            panic!("expected expression-position block, got {stmt:?}");
        };
        let Stmt::Binding(temp) = &inner.stmts[0] else {
            panic!("expected hygienic temp");
        };
        temps.push(interner.resolve(temp.name).to_owned());
    }
    assert_ne!(temps[0], temps[1]);
    assert!(temps.iter().all(|t| t.starts_with("__v_")), "{temps:?}");
}

#[test]
fn user_macros_support_repetition_and_intrinsics() {
    let (module, diagnostics, interner) = expand(
        "#sum3 {\n    ($($x:expr),*) => { 0 $(+ $x)* }\n}\n#name_of {\n    ($x:ident) => { #concat(\"id_\", #upper($x)) }\n}\n+total() : i64 => #sum3(1, 2, 3)\n+label() : str => #name_of(user)\n+text() : str => #stringify(a + b * 2)\n",
        &[],
    );
    assert!(!diagnostics.has_errors(), "{:?}", messages(diagnostics));
    let FnBody::Expr(total) = function_body(&module, 2) else {
        panic!("expected expression body");
    };
    assert!(matches!(total, Expr::Binary { .. }), "{total:?}");
    let FnBody::Expr(Expr::String(label, _)) = function_body(&module, 3) else {
        panic!("expected concatenated string");
    };
    assert_eq!(interner.resolve(*label), "id_USER");
    let FnBody::Expr(Expr::String(text, _)) = function_body(&module, 4) else {
        panic!("expected stringified text");
    };
    assert_eq!(interner.resolve(*text), "a + b * 2");
}

#[test]
fn rules_are_tried_in_order_and_report_no_match() {
    let (_, diagnostics, _) = expand(
        "#pair {\n    ($a:lit, $b:lit) => { $a + $b }\n}\n+f() => #pair(1)\n",
        &[],
    );
    let messages = messages(diagnostics);
    assert!(
        messages
            .iter()
            .any(|message| message.contains("no rule of macro `#pair` matches `1`")),
        "{messages:?}"
    );
}

#[test]
fn unknown_brace_macros_are_errors() {
    let (_, diagnostics, _) = expand("+f() => #nothing { 1 }\n", &[]);
    let messages = messages(diagnostics);
    assert!(
        messages
            .iter()
            .any(|message| message.contains("unknown macro `#nothing`")),
        "{messages:?}"
    );
}

#[test]
fn recursive_expansion_is_bounded() {
    let (_, diagnostics, _) = expand(
        "#forever {\n    ($x:expr) => { #forever($x) }\n}\n+f() => #forever(1)\n",
        &[],
    );
    let messages = messages(diagnostics);
    assert!(
        messages
            .iter()
            .any(|message| message.contains("expands recursively more than")),
        "{messages:?}"
    );
}

#[test]
fn duplicate_macro_definitions_are_rejected() {
    let (_, diagnostics, _) = expand(
        "#twice {\n    ($x:expr) => { $x }\n}\n#twice {\n    ($x:expr) => { $x }\n}\n",
        &[],
    );
    let messages = messages(diagnostics);
    assert!(
        messages
            .iter()
            .any(|message| message.contains("macro `#twice` is defined more than once")),
        "{messages:?}"
    );
}

#[test]
fn operand_fragment_does_not_match_none() {
    let (module, diagnostics, _) = expand("+f(x: i64?) : bool {\n    #assert(x == none)\n}\n", &[]);
    assert!(!diagnostics.has_errors(), "{:?}", messages(diagnostics));
    let expansion = block_body(function_body(&module, 0));
    let bindings = expansion
        .stmts
        .iter()
        .filter(|stmt| matches!(stmt, Stmt::Binding(..)))
        .count();
    assert_eq!(
        bindings, 1,
        "falls through to the generic `$cond:expr` rule"
    );
}

#[test]
fn expansion_tokens_carry_distinct_marks() {
    let (module, diagnostics, _) = expand(
        "+probe(v: i64) : i64 {\n    a = #dbg(v)\n    b = #dbg(v)\n    a + b\n}\n",
        &[],
    );
    assert!(!diagnostics.has_errors(), "{:?}", messages(diagnostics));
    let block = block_body(function_body(&module, 0));
    let marks: Vec<u16> = block.stmts[..2]
        .iter()
        .map(|stmt| match stmt {
            Stmt::Binding(Binding {
                value: Expr::Block(inner),
                ..
            }) => match &inner.stmts[0] {
                Stmt::Binding(temp) => temp.span.expansion_mark(),
                other => panic!("unexpected {other:?}"),
            },
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert!(marks.iter().all(|&m| m != 0));
    assert_ne!(marks[0], marks[1]);
}
