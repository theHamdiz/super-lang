//! Conditional-compilation predicates for `#cfg(...)`.

use std::collections::BTreeSet;

use crate::ast::{Item, MetaAttrKind, Module};
use super_core::{DiagnosticKind, Diagnostics, Interner};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CfgContext {
    pub target_os: String,
    pub target_arch: String,
    pub target_env: String,
    pub target_family: String,
    pub features: BTreeSet<String>,
    pub debug_assertions: bool,
    pub test: bool,
}

impl Default for CfgContext {
    fn default() -> Self {
        Self::host()
    }
}

impl CfgContext {
    pub fn host() -> Self {
        Self {
            target_os: std::env::consts::OS.into(),
            target_arch: std::env::consts::ARCH.into(),
            target_env: host_target_env().into(),
            target_family: if cfg!(windows) { "windows" } else { "unix" }.into(),
            features: BTreeSet::new(),
            debug_assertions: cfg!(debug_assertions),
            test: false,
        }
    }

    pub fn with_features(mut self, features: impl IntoIterator<Item = String>) -> Self {
        self.features.extend(features);
        self
    }
}

fn host_target_env() -> &'static str {
    if cfg!(target_env = "gnu") {
        "gnu"
    } else if cfg!(target_env = "musl") {
        "musl"
    } else if cfg!(target_env = "msvc") {
        "msvc"
    } else {
        ""
    }
}

pub fn evaluate(predicate: &str, context: &CfgContext) -> Result<bool, String> {
    let predicate = predicate.trim();
    if predicate.is_empty() {
        return Err("empty #cfg predicate".into());
    }
    if let Some(inner) = call(predicate, "all") {
        let parts = split_top_level(inner)?;
        return parts
            .into_iter()
            .try_fold(true, |result, part| Ok(result && evaluate(part, context)?));
    }
    if let Some(inner) = call(predicate, "any") {
        let parts = split_top_level(inner)?;
        return parts
            .into_iter()
            .try_fold(false, |result, part| Ok(result || evaluate(part, context)?));
    }
    if let Some(inner) = call(predicate, "not") {
        let parts = split_top_level(inner)?;
        if parts.len() != 1 {
            return Err("not(...) accepts exactly one predicate".into());
        }
        return Ok(!evaluate(parts[0], context)?);
    }
    if predicate == "unix" {
        return Ok(context.target_family == "unix");
    }
    if predicate == "windows" {
        return Ok(context.target_family == "windows");
    }
    if predicate == "debug_assertions" {
        return Ok(context.debug_assertions);
    }
    if predicate == "test" {
        return Ok(context.test);
    }
    let Some((key, raw_value)) = predicate.split_once('=') else {
        return Err(format!("unknown #cfg predicate `{predicate}`"));
    };
    let key = key.trim();
    let value = raw_value.trim().trim_matches('"');
    if value.is_empty() {
        return Err(format!("#cfg `{key}` requires a non-empty value"));
    }
    match key {
        "target_os" => Ok(context.target_os == value),
        "target_arch" => Ok(context.target_arch == value),
        "target_env" => Ok(context.target_env == value),
        "target_family" => Ok(context.target_family == value),
        "feature" => Ok(context.features.contains(value)),
        _ => Err(format!("unknown #cfg key `{key}`")),
    }
}

fn call<'a>(predicate: &'a str, name: &str) -> Option<&'a str> {
    predicate
        .strip_prefix(name)?
        .strip_prefix('(')?
        .strip_suffix(')')
}

fn split_top_level(input: &str) -> Result<Vec<&str>, String> {
    let mut parts = Vec::new();
    let mut depth = 0u32;
    let mut quoted = false;
    let mut start = 0usize;
    for (index, ch) in input.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| "unbalanced `)` in #cfg".to_string())?;
            }
            ',' if !quoted && depth == 0 => {
                let part = input[start..index].trim();
                if part.is_empty() {
                    return Err("empty predicate in #cfg combinator".into());
                }
                parts.push(part);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    if quoted || depth != 0 {
        return Err("unbalanced #cfg predicate".into());
    }
    let tail = input[start..].trim();
    if tail.is_empty() {
        return Err("empty predicate in #cfg combinator".into());
    }
    parts.push(tail);
    Ok(parts)
}

/// Remove parser-generated cfg guards before import resolution or macro expansion.
pub fn strip_cfg_items(
    module: &mut Module,
    interner: &Interner,
    diagnostics: &mut Diagnostics,
    context: &CfgContext,
) {
    let mut source = std::mem::take(&mut module.items).into_iter();
    let mut retained = Vec::new();
    while let Some(item) = source.next() {
        let Item::Binding(guard) = &item else {
            retained.push(item);
            continue;
        };
        if interner.resolve(guard.name) != "__cfg_guard" {
            retained.push(item);
            continue;
        }
        let Some(guarded) = source.next() else {
            diagnostics.error_code(
                DiagnosticKind::Syntax,
                guard.span,
                super_core::err::E0001,
                "#cfg is not attached to an item",
            );
            break;
        };
        let mut enabled = true;
        for attr in &guard.attrs {
            let MetaAttrKind::Cfg(raw) = attr.kind else {
                continue;
            };
            match evaluate(interner.resolve(raw), context) {
                Ok(matches) => enabled &= matches,
                Err(message) => {
                    diagnostics.error_code(
                        DiagnosticKind::Syntax,
                        attr.span,
                        super_core::err::E0001,
                        message,
                    );
                    enabled = false;
                }
            }
        }
        if enabled {
            retained.push(guarded);
        }
    }
    module.items = retained;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Lexer, Parser};
    use super_core::SourceFile;

    fn linux(features: &[&str]) -> CfgContext {
        CfgContext {
            target_os: "linux".into(),
            target_arch: "x86_64".into(),
            target_env: "gnu".into(),
            target_family: "unix".into(),
            features: features.iter().map(|feature| (*feature).into()).collect(),
            debug_assertions: false,
            test: true,
        }
    }

    #[test]
    fn evaluates_predicates_and_combinators() {
        let context = linux(&["uring", "tls"]);
        assert!(evaluate(r#"target_os="linux""#, &context).unwrap());
        assert!(evaluate(r#"feature="uring""#, &context).unwrap());
        assert!(evaluate(r#"all(unix, any(feature="uring", windows))"#, &context).unwrap());
        assert!(evaluate(r#"not(target_arch="aarch64")"#, &context).unwrap());
    }

    #[test]
    fn rejects_unknown_and_unbalanced_predicates() {
        let context = linux(&[]);
        assert!(evaluate("banana", &context).is_err());
        assert!(evaluate("all(unix, not(windows)", &context).is_err());
    }

    #[test]
    fn strips_disabled_items_before_semantic_analysis() {
        let source = SourceFile::new(
            "cfg.sr",
            r#"
#cfg(target_os = "windows")
+windows_only() => missing_windows_symbol
#cfg(all(target_os = "linux", feature = "uring"))
+linux_uring() => 42
"#,
        );
        let mut interner = Interner::new();
        let mut diagnostics = Diagnostics::new();
        let tokens = Lexer::new(&source.text, &mut interner, &mut diagnostics).tokenize();
        let mut parser = Parser::new(&tokens, &source.text, &mut interner, &mut diagnostics);
        let mut module = parser.parse_module();
        strip_cfg_items(&mut module, &interner, &mut diagnostics, &linux(&["uring"]));
        assert!(!diagnostics.has_errors(), "{diagnostics:?}");
        assert_eq!(module.items.len(), 1);
        let Item::Function(function) = &module.items[0] else {
            panic!("expected retained function");
        };
        assert_eq!(interner.resolve(function.name), "linux_uring");
    }

    #[test]
    fn preserves_test_keyword_inside_cfg_predicates() {
        let source = SourceFile::new(
            "cfg_test.sr",
            r#"
#cfg(test)
test "enabled" { assert_eq(1, 1) }
#cfg(not(test))
test "disabled" { assert_eq(1, 2) }
"#,
        );
        let mut interner = Interner::new();
        let mut diagnostics = Diagnostics::new();
        let tokens = Lexer::new(&source.text, &mut interner, &mut diagnostics).tokenize();
        let mut parser = Parser::new(&tokens, &source.text, &mut interner, &mut diagnostics);
        let mut module = parser.parse_module();
        strip_cfg_items(&mut module, &interner, &mut diagnostics, &linux(&[]));
        assert!(!diagnostics.has_errors(), "{diagnostics:?}");
        assert_eq!(module.items.len(), 1);
        let Item::Test(test) = &module.items[0] else {
            panic!("expected retained test");
        };
        assert_eq!(interner.resolve(test.name), "enabled");
    }
}
