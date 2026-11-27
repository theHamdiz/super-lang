use super::*;

fn lint(src: &str) -> Vec<Finding> {
    lint_with(src, &LintContext::for_path("app/orders.sr"))
}

fn lint_with(src: &str, ctx: &LintContext) -> Vec<Finding> {
    lint_source(src, ctx).expect("parses")
}

fn rules(findings: &[Finding]) -> Vec<(&'static str, Option<&str>)> {
    findings
        .iter()
        .map(|f| (f.rule.id(), f.item.as_deref()))
        .collect()
}

#[test]
fn rule_ids_and_names_are_stable_and_unique() {
    let ids: Vec<_> = Rule::ALL.iter().map(|r| r.id()).collect();
    assert_eq!(
        ids,
        [
            "S001", "S002", "S003", "S004", "S005", "S006", "S007", "S008"
        ]
    );
    let mut names: Vec<_> = Rule::ALL.iter().map(|r| r.name()).collect();
    names.dedup();
    assert_eq!(names.len(), 8);
    assert_eq!(Rule::parse("S003"), Some(Rule::Unused));
    assert_eq!(Rule::parse("s003"), Some(Rule::Unused));
    assert_eq!(Rule::parse("needless-mut"), Some(Rule::NeedlessMut));
    assert_eq!(Rule::parse("nope"), None);
}

#[test]
fn clean_source_has_no_findings() {
    let src = "+User = {\n    id: i64\n    -token: str = \"t\"\n    masked(self) : str => \"{self.id}:{self.token}\"\n}\n\n+TaxRate = 0.14\n+total(amount: f64) : f64 => amount * (1.0 + TaxRate)\n";
    assert_eq!(lint(src), Vec::new());
}

#[test]
fn naming_flags_wrong_case_per_item_kind() {
    let src =
        "+bad_type = {\n    BadField: i64\n}\n+Shape = {\n    circle\n}\n+DoThing(X: i64) => X\n";
    assert_eq!(
        rules(&lint(src)),
        [
            ("S001", Some("bad_type")),
            ("S001", Some("BadField")),
            ("S001", Some("circle")),
            ("S001", Some("DoThing")),
            ("S001", Some("X")),
        ]
    );
}

#[test]
fn naming_accepts_constant_styles_for_bindings() {
    assert!(lint("+MAX_RETRIES = 3\n+TaxRate = 0.1\n+port = 80\n").is_empty());
}

#[test]
fn stutter_flags_exported_names_that_repeat_the_module() {
    let src = "+orders_total(x: i64) => x\n+OrdersCache = {\n    n: i64\n}\n+total(x: i64) => x\n";
    assert_eq!(
        rules(&lint(src)),
        [
            ("S002", Some("orders_total")),
            ("S002", Some("OrdersCache"))
        ]
    );
}

#[test]
fn stutter_ignores_private_items_and_entry_files() {
    assert!(
        lint("orders_helper(x: i64) => x\ntest \"t\" { assert_eq(orders_helper(1), 1) }\n")
            .is_empty()
    );
    let main = LintContext::for_path("src/main.sr");
    assert!(lint_with("+main_loop() => 1\n", &main).is_empty());
}

#[test]
fn unused_flags_private_items_never_referenced() {
    let src = "helper() => 1\nLIMIT = 3\n+User = {\n    -secret: str = \"s\"\n    -recompute(self) => 1\n}\n+run() => 2\n";
    assert_eq!(
        rules(&lint(src)),
        [
            ("S003", Some("helper")),
            ("S003", Some("LIMIT")),
            ("S003", Some("secret")),
            ("S003", Some("recompute")),
        ]
    );
}

#[test]
fn unused_counts_tests_interpolation_and_companions() {
    let src = "helper() => 1\nNAME = \"x\"\nlawful(a: i64) => a\n+greet() => \"hi {NAME}\"\ntest \"t\" { assert_eq(helper(), 1) }\n";
    let mut ctx = LintContext::for_path("app/orders.sr");
    ctx.companions
        .push("#law(\"id\")\nid_law(a: i64) {\n    #assert(lawful(a) == a)\n}\n".to_owned());
    assert_eq!(lint_with(src, &ctx), Vec::new());
}

#[test]
fn unused_skips_main_underscore_and_attributed_functions() {
    let src =
        "main() {\n    print(\"x\")\n}\n_scratch() => 1\n#get(\"/x\")\nroute_x(req: i64) => req\n";
    assert_eq!(lint(src), Vec::new());
}

#[test]
fn shadowing_flags_fresh_bindings_that_hide_locals() {
    let src = "+f(xs: i64, n: i64) : i64 {\n    total = 0\n    for n in 0..xs {\n        total += n\n    }\n    g = |total| => total + 1\n    g(total)\n}\n";
    assert_eq!(
        rules(&lint(src)),
        [("S004", Some("n")), ("S004", Some("total"))]
    );
}

#[test]
fn shadowing_ignores_reassignment_and_implicit_names() {
    let src = "+f(n: i64) : i64 {\n    x = 1\n    x = x + n\n    for 0..n {\n        x += it + index\n    }\n    x\n}\n";
    assert!(lint(src).is_empty());
}

#[test]
fn shadowing_flags_variant_payload_binders() {
    let src = "S = {\n    A(amount: i64)\n    B\n}\n+f(s: S, amount: i64) : i64 {\n    when s {\n        S.A { amount } => amount\n        else => 0\n    }\n}\n";
    assert_eq!(rules(&lint(src)), [("S004", Some("amount"))]);
}

#[test]
fn needless_mut_flags_at_params_that_are_never_mutated() {
    let src = "+P = {\n    x: i64\n    bump(@self) {\n        self.x += 1\n    }\n    peek(@self) : i64 => self.x\n}\n+push_one(@xs: i64, @ys: i64) {\n    @xs.push(1)\n    print(ys)\n}\n";
    assert_eq!(
        rules(&lint(src)),
        [("S006", Some("peek.self")), ("S006", Some("push_one.ys"))]
    );
}

#[test]
fn needless_mut_sees_assign_mut_ref_loops_and_builtin_mutators() {
    let src = "+a(@xs: i64) {\n    xs[0] = 1\n}\n+b(@xs: i64) {\n    for @xs {\n        it += 1\n    }\n}\n+c(@p: i64, q: i64) {\n    sink(@p, q)\n}\n+d(@xs: i64) {\n    xs.push(1)\n}\n+e(@m: i64) {\n    #assert(m.touch())\n}\n+h(@b: i64) {\n    host.sys_sb_clear(b.h)\n}\nsink(@p: i64, q: i64) {\n    p = q\n}\n";
    assert_eq!(lint(src), Vec::new());
}

#[test]
fn missing_law_is_info_and_only_applies_to_stdlib() {
    let src = "+double(x: i64) => x * 2\n";
    let std_ctx = LintContext::for_path("stdlib/math/twice.sr");
    let findings = lint_with(src, &std_ctx);
    assert_eq!(rules(&findings), [("S005", None)]);
    assert_eq!(findings[0].severity, Severity::Info);
    assert!(lint(src).is_empty());
    let mut with_law = std_ctx.clone();
    with_law.companions.push(
        "#law(\"doubles\")\ndoubles(x: i64) {\n    #assert(double(x) == x + x)\n}\n".to_owned(),
    );
    assert!(lint_with(src, &with_law).is_empty());
}

#[test]
fn redundant_plus_and_puns_are_fixed_safely() {
    // `{id: id}` stays: a map literal and a punned record are different values.
    let src = "+User = {\n    +id: i64\n    +name(self) : str => \"n\"\n}\n+f(id: i64) : User {\n    rec = {id: id, other: 2}\n    print(rec)\n    u: User = .{id: id}\n    print(u)\n    User{id: id}\n}\n";
    let findings = lint(src);
    assert_eq!(
        rules(&findings),
        [
            ("S007", None),
            ("S007", None),
            ("S008", Some("id")),
            ("S008", Some("id"))
        ]
    );
    let fixed = apply_fixes(src, &findings);
    assert_eq!(
        fixed,
        "+User = {\n    id: i64\n    name(self) : str => \"n\"\n}\n+f(id: i64) : User {\n    rec = {id: id, other: 2}\n    print(rec)\n    u: User = .{id}\n    print(u)\n    User{id}\n}\n"
    );
    assert!(lint(&fixed).is_empty());
}

#[test]
fn config_allows_denies_and_scopes_exceptions() {
    let findings = lint("helper() => 1\nother() => 2\n");
    let config = LintConfig {
        allow: Vec::new(),
        deny: vec!["unused".into()],
        exceptions: vec![Exception {
            rule: "S003".into(),
            path: "orders.sr".into(),
            item: Some("other".into()),
            reason: "kept for the next release".into(),
        }],
    };
    config.validate().unwrap();
    let kept = config.apply("app/orders.sr", findings.clone());
    assert_eq!(rules(&kept), [("S003", Some("helper"))]);
    assert_eq!(kept[0].severity, Severity::Error);
    assert_eq!(
        config.apply("app/other_orders.sr", findings.clone()).len(),
        2
    );
    let off = LintConfig {
        allow: vec!["S003".into()],
        ..LintConfig::default()
    };
    assert!(off.apply("app/orders.sr", findings).is_empty());
}

#[test]
fn config_rejects_unknown_rules_and_missing_reasons() {
    let unknown = LintConfig {
        allow: vec!["S999".into()],
        ..LintConfig::default()
    };
    assert!(unknown.validate().is_err());
    let no_reason = LintConfig {
        exceptions: vec![Exception {
            rule: "S003".into(),
            path: "a.sr".into(),
            ..Exception::default()
        }],
        ..LintConfig::default()
    };
    assert!(no_reason.validate().is_err());
}

#[test]
fn parse_errors_are_reported_not_linted() {
    assert!(lint_source("+f( {\n", &LintContext::default()).is_err());
}

#[test]
fn findings_carry_one_based_positions() {
    let findings = lint("P = {\n    x: i64\n}\n+f(x: i64) : P {\n    P{x: x}\n}\n");
    assert_eq!((findings[0].line, findings[0].column), (5, 7));
}
