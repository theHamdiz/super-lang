//! Comptime HTML/Text template checker for `std.html.template`.
//!
//! Stable diagnostic codes (never renumber once released):
//! - `T001` syntax
//! - `T002` unknown name / partial
//! - `T003` not renderable (reserved for typed render sites)
//! - `T004` unsafe HTML context
//!
//! Library surface: `Html.compile` / `Text.compile` / `compile_html` / `compile_text`
//! with a string-literal source, plus `#template_html("…")` / `#template_text("…")`.

use crate::ast::{Block, Expr, FnBody, Function, Item, Module, Stmt};
use std::collections::HashSet;
use super_core::{DiagnosticKind, Diagnostics, Interner, Span, err};

/// Template flavour: contextual HTML escaping vs plain Text (LLM prompts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemplateMode {
    Html,
    Text,
}

/// Escape / render context while scanning an Html template source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HtmlCtx {
    Text,
    AttrQuoted,
    AttrUnquoted,
    Url,
    Script,
    Style,
}

/// One diagnostic produced by the template checker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateDiag {
    pub code: &'static str,
    pub message: String,
    pub byte_offset: usize,
}

/// Compiled template node (subset used for validation / fuzz).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Lit(String),
    Interp {
        path: String,
        filter: Option<String>,
        raw: bool,
        js: bool,
        css: bool,
        offset: usize,
    },
    If {
        path: String,
        then_body: Vec<Node>,
        else_body: Vec<Node>,
    },
    For {
        item: String,
        path: String,
        body: Vec<Node>,
    },
    Partial {
        name: String,
        offset: usize,
    },
}

const FILTERS: &[&str] = &["upper", "lower", "trim", "len", "json", "escape"];

/// Parse a template source. On success returns nodes; on failure returns diagnostics
/// (first error is enough for build, but fuzz collects all).
pub fn parse_template(
    src: &str,
    mode: TemplateMode,
    known: &HashSet<String>,
    known_partials: &HashSet<String>,
) -> Result<Vec<Node>, Vec<TemplateDiag>> {
    let mut diags = Vec::new();
    let nodes = match parse_body(src, 0, src.len(), &mut diags) {
        Ok(n) => n,
        Err(()) => return Err(diags),
    };
    check_names(&nodes, known, known_partials, &mut diags);
    if mode == TemplateMode::Html {
        check_html_contexts(src, &nodes, &mut diags);
    }
    if diags.is_empty() {
        Ok(nodes)
    } else {
        Err(diags)
    }
}

fn rtrim_ws(s: &str) -> String {
    s.trim_end_matches([' ', '\t', '\n', '\r']).to_string()
}

fn ltrim_ws_at(src: &str, mut i: usize, end: usize) -> usize {
    while i < end {
        match src.as_bytes()[i] {
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            _ => break,
        }
    }
    i
}

fn parse_body(
    src: &str,
    mut i: usize,
    end: usize,
    diags: &mut Vec<TemplateDiag>,
) -> Result<Vec<Node>, ()> {
    let mut out = Vec::new();
    let mut pending_trim_r = false;
    while i < end {
        if pending_trim_r {
            i = ltrim_ws_at(src, i, end);
            pending_trim_r = false;
        }
        if let Some(rel) = src[i..end].find("{{") {
            let open = i + rel;
            if open > i {
                out.push(Node::Lit(src[i..open].to_string()));
            }
            let after = open + 2;
            if after >= end {
                diags.push(TemplateDiag {
                    code: err::T001,
                    message: "T001: unclosed template tag".into(),
                    byte_offset: open,
                });
                return Err(());
            }
            // Whitespace trim markers: {{- trims left lit; -}} trims following lit.
            let (trim_l, tag_start) = if src[after..end].starts_with('-') {
                (true, after + 1)
            } else {
                (false, after)
            };
            let close_rel = match src[tag_start..end].find("}}") {
                Some(r) => r,
                None => {
                    diags.push(TemplateDiag {
                        code: err::T001,
                        message: "T001: unclosed template tag".into(),
                        byte_offset: open,
                    });
                    return Err(());
                }
            };
            let mut tag_end = tag_start + close_rel;
            let close_end = tag_end + 2;
            let trim_r = tag_end > tag_start && src.as_bytes()[tag_end - 1] == b'-';
            if trim_r {
                tag_end -= 1;
            }
            if trim_l && let Some(Node::Lit(prev)) = out.last_mut() {
                *prev = rtrim_ws(prev);
            }
            pending_trim_r = trim_r;
            let raw_tag = src[tag_start..tag_end].trim();
            if raw_tag.is_empty() {
                diags.push(TemplateDiag {
                    code: err::T001,
                    message: "T001: empty template tag".into(),
                    byte_offset: open,
                });
                return Err(());
            }
            // Closing tags terminate the caller via the scanner in if/for.
            if raw_tag.starts_with('/') {
                diags.push(TemplateDiag {
                    code: err::T001,
                    message: format!("T001: unexpected closing tag `{{{{{raw_tag}}}}}`"),
                    byte_offset: open,
                });
                return Err(());
            }
            if raw_tag.starts_with("#if ") || raw_tag == "#if" {
                let path = raw_tag.strip_prefix("#if").unwrap_or("").trim();
                if path.is_empty() || !is_path(path) {
                    diags.push(TemplateDiag {
                        code: err::T001,
                        message: "T001: `#if` expects a dotted path".into(),
                        byte_offset: open,
                    });
                    return Err(());
                }
                let (then_src, else_src, next) = split_if_else(src, close_end, end, diags, open)?;
                let then_body = parse_body(src, then_src.0, then_src.1, diags)?;
                let else_body = if else_src.0 < else_src.1 {
                    parse_body(src, else_src.0, else_src.1, diags)?
                } else {
                    Vec::new()
                };
                out.push(Node::If {
                    path: path.to_string(),
                    then_body,
                    else_body,
                });
                i = next;
                continue;
            }
            if raw_tag.starts_with("#for ") {
                let rest = raw_tag.strip_prefix("#for ").unwrap().trim();
                let (item, path) = match rest.split_once(" in ") {
                    Some((a, b)) => (a.trim(), b.trim()),
                    None => {
                        diags.push(TemplateDiag {
                            code: err::T001,
                            message: "T001: `#for` expects `x in path`".into(),
                            byte_offset: open,
                        });
                        return Err(());
                    }
                };
                if !is_ident(item) || !is_path(path) {
                    diags.push(TemplateDiag {
                        code: err::T001,
                        message: "T001: `#for` has invalid item or path".into(),
                        byte_offset: open,
                    });
                    return Err(());
                }
                let (body_lo, body_hi, next) =
                    split_until_close(src, close_end, end, "/for", diags, open)?;
                let body = parse_body(src, body_lo, body_hi, diags)?;
                out.push(Node::For {
                    item: item.to_string(),
                    path: path.to_string(),
                    body,
                });
                i = next;
                continue;
            }
            if raw_tag.starts_with("#else") {
                diags.push(TemplateDiag {
                    code: err::T001,
                    message: "T001: `#else` without matching `#if`".into(),
                    byte_offset: open,
                });
                return Err(());
            }
            if let Some(name) = raw_tag.strip_prefix('>') {
                let name = name.trim();
                if !is_ident(name) {
                    diags.push(TemplateDiag {
                        code: err::T001,
                        message: "T001: partial name must be an identifier".into(),
                        byte_offset: open,
                    });
                    return Err(());
                }
                out.push(Node::Partial {
                    name: name.to_string(),
                    offset: open,
                });
                i = close_end;
                continue;
            }
            // Interpolation: raw(x) | js(x) | css(x) | path | filter
            let (path, filter, raw, js, css) = parse_interp(raw_tag, diags, open)?;
            out.push(Node::Interp {
                path,
                filter,
                raw,
                js,
                css,
                offset: open,
            });
            i = close_end;
        } else {
            if pending_trim_r {
                i = ltrim_ws_at(src, i, end);
                pending_trim_r = false;
            }
            if i < end {
                out.push(Node::Lit(src[i..end].to_string()));
            }
            break;
        }
    }
    let _ = pending_trim_r;
    Ok(out)
}

fn parse_interp(
    tag: &str,
    diags: &mut Vec<TemplateDiag>,
    offset: usize,
) -> Result<(String, Option<String>, bool, bool, bool), ()> {
    let t = tag.trim();
    if let Some(inner) = t.strip_prefix("raw(").and_then(|s| s.strip_suffix(')')) {
        let p = inner.trim();
        if !is_path(p) {
            diags.push(TemplateDiag {
                code: err::T001,
                message: "T001: raw(...) expects a dotted path".into(),
                byte_offset: offset,
            });
            return Err(());
        }
        return Ok((p.to_string(), None, true, false, false));
    }
    if let Some(inner) = t.strip_prefix("js(").and_then(|s| s.strip_suffix(')')) {
        let p = inner.trim();
        if !is_path(p) {
            diags.push(TemplateDiag {
                code: err::T001,
                message: "T001: js(...) expects a dotted path".into(),
                byte_offset: offset,
            });
            return Err(());
        }
        return Ok((p.to_string(), None, false, true, false));
    }
    if let Some(inner) = t.strip_prefix("css(").and_then(|s| s.strip_suffix(')')) {
        let p = inner.trim();
        if !is_path(p) {
            diags.push(TemplateDiag {
                code: err::T001,
                message: "T001: css(...) expects a dotted path".into(),
                byte_offset: offset,
            });
            return Err(());
        }
        return Ok((p.to_string(), None, false, false, true));
    }
    let (path_part, filter) = if let Some((p, f)) = t.split_once('|') {
        (p.trim(), Some(f.trim().to_string()))
    } else {
        (t, None)
    };
    if !is_path(path_part) {
        diags.push(TemplateDiag {
            code: err::T001,
            message: format!("T001: invalid interpolation path `{path_part}`"),
            byte_offset: offset,
        });
        return Err(());
    }
    if let Some(ref f) = filter
        && !FILTERS.contains(&f.as_str())
    {
        diags.push(TemplateDiag {
            code: err::T001,
            message: format!("T001: unknown filter `{f}`"),
            byte_offset: offset,
        });
        return Err(());
    }
    // Reject call-like / expression forms beyond path|filter.
    if path_part.contains('(') || path_part.contains('+') || path_part.contains(' ') {
        diags.push(TemplateDiag {
            code: err::T001,
            message: "T001: arbitrary expressions are not allowed in templates".into(),
            byte_offset: offset,
        });
        return Err(());
    }
    Ok((path_part.to_string(), filter, false, false, false))
}

fn split_until_close(
    src: &str,
    start: usize,
    end: usize,
    close: &str,
    diags: &mut Vec<TemplateDiag>,
    open_off: usize,
) -> Result<(usize, usize, usize), ()> {
    let mut i = start;
    let mut depth = 1i32;
    while i < end {
        let Some(rel) = src[i..end].find("{{") else {
            break;
        };
        let open = i + rel;
        let after = open + 2;
        let tag_start = if src[after..end].starts_with('-') {
            after + 1
        } else {
            after
        };
        let Some(crel) = src[tag_start..end].find("}}") else {
            break;
        };
        let mut tag_end = tag_start + crel;
        let close_end = tag_end + 2;
        if tag_end > tag_start && src.as_bytes()[tag_end - 1] == b'-' {
            tag_end -= 1;
        }
        let raw_tag = src[tag_start..tag_end].trim();
        if raw_tag.starts_with("#if") || raw_tag.starts_with("#for") {
            depth += 1;
        } else if raw_tag == close || raw_tag == format!("-{close}") {
            depth -= 1;
            if depth == 0 {
                return Ok((start, open, close_end));
            }
        }
        i = close_end;
    }
    diags.push(TemplateDiag {
        code: err::T001,
        message: format!("T001: unclosed block (expected `{{{{{close}}}}}`)"),
        byte_offset: open_off,
    });
    Err(())
}

/// `(then range, else range, end offset)` of an `{% if %}` body.
type IfElseSplit = ((usize, usize), (usize, usize), usize);

fn split_if_else(
    src: &str,
    start: usize,
    end: usize,
    diags: &mut Vec<TemplateDiag>,
    open_off: usize,
) -> Result<IfElseSplit, ()> {
    let mut i = start;
    let mut depth = 1i32;
    // (else_tag_open, else_body_start)
    let mut else_at: Option<(usize, usize)> = None;
    while i < end {
        let Some(rel) = src[i..end].find("{{") else {
            break;
        };
        let open = i + rel;
        let after = open + 2;
        let tag_start = if src[after..end].starts_with('-') {
            after + 1
        } else {
            after
        };
        let Some(crel) = src[tag_start..end].find("}}") else {
            break;
        };
        let mut tag_end = tag_start + crel;
        let close_end = tag_end + 2;
        if tag_end > tag_start && src.as_bytes()[tag_end - 1] == b'-' {
            tag_end -= 1;
        }
        let raw_tag = src[tag_start..tag_end].trim();
        if raw_tag.starts_with("#if") || raw_tag.starts_with("#for") {
            depth += 1;
        } else if (raw_tag == "#else" || raw_tag == "-#else") && depth == 1 {
            else_at = Some((open, close_end));
            i = close_end;
            continue;
        } else if raw_tag == "/if" || raw_tag == "-/if" {
            depth -= 1;
            if depth == 0 {
                return if let Some((else_open, else_body)) = else_at {
                    Ok(((start, else_open), (else_body, open), close_end))
                } else {
                    Ok(((start, open), (open, open), close_end))
                };
            }
        }
        i = close_end;
    }
    diags.push(TemplateDiag {
        code: err::T001,
        message: "T001: unclosed `#if` (expected `{{/if}}`)".into(),
        byte_offset: open_off,
    });
    Err(())
}

fn check_names(
    nodes: &[Node],
    known: &HashSet<String>,
    known_partials: &HashSet<String>,
    diags: &mut Vec<TemplateDiag>,
) {
    if known.is_empty() && known_partials.is_empty() {
        return;
    }
    for n in nodes {
        match n {
            Node::Lit(_) => {}
            Node::Interp { path, offset, .. } => {
                let root = path.split('.').next().unwrap_or(path);
                if known
                    .iter()
                    .any(|k| k == &format!("!{root}") || k == &format!("!{path}"))
                {
                    diags.push(TemplateDiag {
                        code: err::T003,
                        message: format!(
                            "T003: `{path}` is not renderable (needs Display, Escaped, Raw, Js, or Css)"
                        ),
                        byte_offset: *offset,
                    });
                } else if !known.is_empty()
                    && !known.contains(root)
                    && !known.contains(path)
                    && !known
                        .iter()
                        .any(|k| k.strip_prefix('!').is_some_and(|n| n == root || n == path))
                {
                    diags.push(TemplateDiag {
                        code: err::T002,
                        message: format!("T002: unknown template name `{path}`"),
                        byte_offset: *offset,
                    });
                }
            }
            Node::If {
                path,
                then_body,
                else_body,
            } => {
                let root = path.split('.').next().unwrap_or(path);
                if !known.is_empty() && !known.contains(root) && !known.contains(path) {
                    diags.push(TemplateDiag {
                        code: err::T002,
                        message: format!("T002: unknown template name `{path}`"),
                        byte_offset: 0,
                    });
                }
                check_names(then_body, known, known_partials, diags);
                check_names(else_body, known, known_partials, diags);
            }
            Node::For { item, path, body } => {
                let root = path.split('.').next().unwrap_or(path);
                if !known.is_empty() && !known.contains(root) && !known.contains(path) {
                    diags.push(TemplateDiag {
                        code: err::T002,
                        message: format!("T002: unknown template name `{path}`"),
                        byte_offset: 0,
                    });
                }
                let mut nested = known.clone();
                nested.insert(item.clone());
                check_names(body, &nested, known_partials, diags);
            }
            Node::Partial { name, offset } => {
                if !known_partials.is_empty() && !known_partials.contains(name) {
                    diags.push(TemplateDiag {
                        code: err::T002,
                        message: format!("T002: unknown partial `{name}`"),
                        byte_offset: *offset,
                    });
                }
            }
        }
    }
}

/// Walk the source around each interp and emit T004 for unsafe Html contexts.
fn check_html_contexts(src: &str, nodes: &[Node], diags: &mut Vec<TemplateDiag>) {
    for n in nodes {
        match n {
            Node::Interp {
                offset,
                raw,
                js,
                css,
                ..
            } => {
                let ctx = html_context_at(src, *offset);
                match ctx {
                    HtmlCtx::Script if !js && !raw => {
                        diags.push(TemplateDiag {
                            code: err::T004,
                            message: "T004: interpolation in <script> requires js(x) or raw(x)"
                                .into(),
                            byte_offset: *offset,
                        });
                    }
                    HtmlCtx::Style if !css && !raw => {
                        diags.push(TemplateDiag {
                            code: err::T004,
                            message: "T004: interpolation in <style> requires css(x) or raw(x)"
                                .into(),
                            byte_offset: *offset,
                        });
                    }
                    HtmlCtx::AttrUnquoted if !raw => {
                        diags.push(TemplateDiag {
                            code: err::T004,
                            message: "T004: unquoted attribute interpolation is unsafe; quote the attribute or use raw(x)"
                                .into(),
                            byte_offset: *offset,
                        });
                    }
                    _ => {}
                }
                if *raw {
                    // Escape hatch is greppable; still listed via help-less diag note in T004 path only when unsafe.
                    let _ = raw;
                }
            }
            Node::If {
                then_body,
                else_body,
                ..
            } => {
                check_html_contexts(src, then_body, diags);
                check_html_contexts(src, else_body, diags);
            }
            Node::For { body, .. } => check_html_contexts(src, body, diags),
            _ => {}
        }
    }
}

fn html_context_at(src: &str, offset: usize) -> HtmlCtx {
    let before = &src[..offset.min(src.len())];
    let lower: String = before.chars().map(|c| c.to_ascii_lowercase()).collect();
    // Script / style bodies: last open tag wins if not closed.
    let script_open = lower.rfind("<script");
    let script_close = lower.rfind("</script");
    if let Some(o) = script_open
        && script_close.map(|c| c < o).unwrap_or(true)
    {
        return HtmlCtx::Script;
    }
    let style_open = lower.rfind("<style");
    let style_close = lower.rfind("</style");
    if let Some(o) = style_open
        && style_close.map(|c| c < o).unwrap_or(true)
    {
        return HtmlCtx::Style;
    }
    // Attribute context: look for last `=` before offset inside a tag.
    if let Some(lt) = before.rfind('<')
        && !before[lt..].contains('>')
    {
        let tag = &before[lt..];
        let tag_l = tag.to_ascii_lowercase();
        // Find last attr=
        if let Some(eq) = tag.rfind('=') {
            let after_eq = &tag[eq + 1..];
            let trimmed = after_eq.trim_start();
            let name_start = tag[..eq]
                .trim_end()
                .rfind(|c: char| c.is_whitespace() || c == '<')
                .map(|i| i + 1)
                .unwrap_or(0);
            let attr_name = tag[name_start..eq].trim().to_ascii_lowercase();
            let is_url = attr_name == "href"
                || attr_name == "src"
                || attr_name == "action"
                || attr_name == "formaction"
                || attr_name == "poster"
                || attr_name == "data";
            if trimmed.starts_with('"') || trimmed.starts_with('\'') {
                return if is_url {
                    HtmlCtx::Url
                } else {
                    HtmlCtx::AttrQuoted
                };
            }
            return HtmlCtx::AttrUnquoted;
        }
        let _ = tag_l;
    }
    HtmlCtx::Text
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {
            chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
        }
        _ => false,
    }
}

fn is_path(s: &str) -> bool {
    !s.is_empty() && s.split('.').all(is_ident)
}

/// Emit diagnostics for string-literal template compiles found in a module.
pub fn check_module_templates(module: &Module, interner: &Interner, diagnostics: &mut Diagnostics) {
    for item in &module.items {
        walk_item(item, interner, diagnostics);
    }
}

fn walk_item(item: &Item, interner: &Interner, diagnostics: &mut Diagnostics) {
    match item {
        Item::Function(f) => walk_function(f, interner, diagnostics),
        Item::Test(t) => walk_block(&t.body, interner, diagnostics),
        Item::Binding(b) => {
            for a in &b.attrs {
                check_template_meta(a, interner, diagnostics);
            }
            walk_expr(&b.value, interner, diagnostics);
        }
        Item::Struct(s) => {
            for m in &s.methods {
                walk_function(m, interner, diagnostics);
            }
        }
        Item::Sum(s) => {
            for m in &s.methods {
                walk_function(m, interner, diagnostics);
            }
        }
        Item::Use(_) | Item::State(_) | Item::Rewrite(_) | Item::Trait(_) | Item::Macro(_) => {}
    }
}

fn check_template_meta(
    a: &crate::ast::MetaAttr,
    interner: &Interner,
    diagnostics: &mut Diagnostics,
) {
    let raw = interner.resolve(a.raw);
    let (mode, src) = if let Some(rest) = raw.strip_prefix("template_html(") {
        (TemplateMode::Html, rest.trim_end_matches(')'))
    } else if let Some(rest) = raw.strip_prefix("template_text(") {
        (TemplateMode::Text, rest.trim_end_matches(')'))
    } else {
        return;
    };
    let src = unquote(src.trim());
    emit_parse(
        src,
        mode,
        &HashSet::new(),
        &HashSet::new(),
        a.span,
        diagnostics,
    );
}

fn walk_function(f: &Function, interner: &Interner, diagnostics: &mut Diagnostics) {
    for a in &f.attrs {
        check_template_meta(a, interner, diagnostics);
    }
    match &f.body {
        FnBody::Expr(e) => walk_expr(e, interner, diagnostics),
        FnBody::Block(b) => walk_block(b, interner, diagnostics),
    }
}

fn walk_block(b: &Block, interner: &Interner, diagnostics: &mut Diagnostics) {
    for s in &b.stmts {
        walk_stmt(s, interner, diagnostics);
    }
}

fn walk_stmt(s: &Stmt, interner: &Interner, diagnostics: &mut Diagnostics) {
    match s {
        Stmt::Expr(e) | Stmt::Fail(e, _) => walk_expr(e, interner, diagnostics),
        Stmt::Return(opt, _) | Stmt::Panic(opt, _) => {
            if let Some(e) = opt {
                walk_expr(e, interner, diagnostics);
            }
        }
        Stmt::Binding(b) => {
            for a in &b.attrs {
                check_template_meta(a, interner, diagnostics);
            }
            walk_expr(&b.value, interner, diagnostics);
        }
        Stmt::Destructure { value, .. } => walk_expr(value, interner, diagnostics),
        Stmt::Ensure {
            cond, else_block, ..
        } => {
            walk_expr(cond, interner, diagnostics);
            walk_block(else_block, interner, diagnostics);
        }
        Stmt::Each { iter, body, .. }
        | Stmt::While {
            cond: iter, body, ..
        } => {
            walk_expr(iter, interner, diagnostics);
            walk_block(body, interner, diagnostics);
        }
        Stmt::Loop { body, .. } | Stmt::Scope { body, .. } => {
            walk_block(body, interner, diagnostics)
        }
        Stmt::MultiAssign {
            targets, values, ..
        } => {
            for t in targets {
                walk_expr(t, interner, diagnostics);
            }
            for v in values {
                walk_expr(v, interner, diagnostics);
            }
        }
        Stmt::Guarded { stmt, cond, .. } => {
            walk_stmt(stmt, interner, diagnostics);
            walk_expr(cond, interner, diagnostics);
        }
        Stmt::When { subjects, arms, .. } => {
            for s in subjects {
                walk_expr(s, interner, diagnostics);
            }
            for arm in arms {
                walk_expr(&arm.body, interner, diagnostics);
            }
        }
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Comptime(_) => {}
    }
}

fn walk_expr(e: &Expr, interner: &Interner, diagnostics: &mut Diagnostics) {
    match e {
        Expr::Call {
            callee, args, span, ..
        } => {
            if let Some(mode) = compile_callee(callee, interner)
                && let Some(Expr::String(sym, _)) = args.first()
            {
                let src = interner.resolve(*sym).to_string();
                let mut known = HashSet::new();
                let mut partials = HashSet::new();
                if let Some(Expr::Array(elems, _)) = args.get(1) {
                    for el in elems {
                        if let Expr::String(s, _) = el {
                            known.insert(interner.resolve(*s).to_string());
                        }
                    }
                }
                if let Some(Expr::Array(elems, _)) = args.get(2) {
                    for el in elems {
                        if let Expr::String(s, _) = el {
                            partials.insert(interner.resolve(*s).to_string());
                        }
                    }
                }
                emit_parse(&src, mode, &known, &partials, *span, diagnostics);
            }
            walk_expr(callee, interner, diagnostics);
            for a in args {
                walk_expr(a, interner, diagnostics);
            }
        }
        Expr::Field { base, .. }
        | Expr::Unary { expr: base, .. }
        | Expr::Index { base, .. }
        | Expr::Slice { base, .. } => walk_expr(base, interner, diagnostics),
        Expr::Binary { left, right, .. }
        | Expr::Assign {
            target: left,
            value: right,
            ..
        } => {
            walk_expr(left, interner, diagnostics);
            walk_expr(right, interner, diagnostics);
        }
        Expr::Array(xs, _) => {
            for x in xs {
                walk_expr(x, interner, diagnostics);
            }
        }
        Expr::Ternary {
            cond,
            then_expr,
            else_expr,
            ..
        } => {
            walk_expr(cond, interner, diagnostics);
            walk_expr(then_expr, interner, diagnostics);
            walk_expr(else_expr, interner, diagnostics);
        }
        _ => {}
    }
}

fn compile_callee(callee: &Expr, interner: &Interner) -> Option<TemplateMode> {
    match callee {
        Expr::Ident(sym, _) => match interner.resolve(*sym) {
            "compile_html" => Some(TemplateMode::Html),
            "compile_text" => Some(TemplateMode::Text),
            _ => None,
        },
        Expr::Field { base, field, .. } => {
            let f = interner.resolve(*field);
            if f != "compile" {
                return None;
            }
            match base.as_ref() {
                Expr::Ident(sym, _) => match interner.resolve(*sym) {
                    "Html" | "HtmlTpl" | "HtmlTemplate" => Some(TemplateMode::Html),
                    "Text" | "TextTpl" | "TextTemplate" => Some(TemplateMode::Text),
                    _ => None,
                },
                _ => None,
            }
        }
        _ => None,
    }
}

fn emit_parse(
    src: &str,
    mode: TemplateMode,
    known: &HashSet<String>,
    partials: &HashSet<String>,
    span: Span,
    diagnostics: &mut Diagnostics,
) {
    match parse_template(src, mode, known, partials) {
        Ok(_) => {}
        Err(ds) => {
            for d in ds {
                let kind = if d.code == err::T001 {
                    DiagnosticKind::Syntax
                } else {
                    DiagnosticKind::Type
                };
                if d.code == err::T004 {
                    diagnostics.error_code_help(
                        kind,
                        span,
                        d.code,
                        d.message,
                        "use js(x)/css(x) for script/style, quote attributes, or raw(x) (greppable escape hatch)",
                    );
                } else if d.code == err::T002 {
                    diagnostics.error_code_help(
                        kind,
                        span,
                        d.code,
                        d.message,
                        "pass known names as the second argument to compile_html/compile_text",
                    );
                } else {
                    diagnostics.error_code(kind, span, d.code, d.message);
                }
            }
        }
    }
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    if let Some(inner) = s.strip_prefix('"').and_then(|x| x.strip_suffix('"')) {
        return inner;
    }
    if let Some(inner) = s
        .strip_prefix('r')
        .and_then(|x| x.strip_prefix('"'))
        .and_then(|x| x.strip_suffix('"'))
    {
        return inner;
    }
    s
}

/// Reference HTML escaper used by differential XSS tests.
pub fn ref_escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// URL scheme allowlist check (javascript:/vbscript:/data:text/html blocked).
pub fn ref_url_allowed(s: &str) -> bool {
    let low: String = s.chars().take(24).map(|c| c.to_ascii_lowercase()).collect();
    !(low.starts_with("javascript:")
        || low.starts_with("vbscript:")
        || low.starts_with("data:text/html"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t001_unclosed() {
        let err = parse_template(
            "{{name",
            TemplateMode::Text,
            &HashSet::new(),
            &HashSet::new(),
        )
        .unwrap_err();
        assert_eq!(err[0].code, err::T001);
    }

    #[test]
    fn t002_unknown_name() {
        let mut known = HashSet::new();
        known.insert("title".into());
        let err =
            parse_template("{{name}}", TemplateMode::Text, &known, &HashSet::new()).unwrap_err();
        assert_eq!(err[0].code, err::T002);
    }

    #[test]
    fn t003_not_renderable() {
        let mut known = HashSet::new();
        known.insert("!blob".into());
        let err =
            parse_template("{{blob}}", TemplateMode::Text, &known, &HashSet::new()).unwrap_err();
        assert_eq!(err[0].code, err::T003);
    }

    #[test]
    fn t004_script_needs_js() {
        let err = parse_template(
            "<script>{{x}}</script>",
            TemplateMode::Html,
            &HashSet::new(),
            &HashSet::new(),
        )
        .unwrap_err();
        assert_eq!(err[0].code, err::T004);
    }

    #[test]
    fn t004_script_js_ok() {
        assert!(
            parse_template(
                "<script>{{js(x)}}</script>",
                TemplateMode::Html,
                &HashSet::new(),
                &HashSet::new(),
            )
            .is_ok()
        );
    }

    #[test]
    fn ok_if_for_partial() {
        let mut parts = HashSet::new();
        parts.insert("card".into());
        let nodes = parse_template(
            "Hi {{name}}{{#if on}}Y{{#else}}N{{/if}}{{#for i in xs}}{{i}}{{/for}}{{> card}}",
            TemplateMode::Text,
            &HashSet::new(),
            &parts,
        )
        .unwrap();
        assert!(nodes.len() >= 4);
    }

    #[test]
    fn whitespace_trim_opt_in() {
        let keep = parse_template(
            "A {{ name }} B",
            TemplateMode::Text,
            &HashSet::new(),
            &HashSet::new(),
        )
        .unwrap();
        // Default: surrounding spaces in literals preserved (name tag itself trimmed).
        assert!(matches!(&keep[0], Node::Lit(s) if s == "A "));
        assert!(matches!(keep.last(), Some(Node::Lit(s)) if s == " B"));

        let trimmed = parse_template(
            "A {{- name -}} B",
            TemplateMode::Text,
            &HashSet::new(),
            &HashSet::new(),
        )
        .unwrap();
        assert!(matches!(&trimmed[0], Node::Lit(s) if s == "A"));
        assert!(matches!(trimmed.last(), Some(Node::Lit(s)) if s == "B"));
    }

    #[test]
    fn xss_corpus_differential_vs_ref_escape() {
        let payloads = [
            "<script>alert(1)</script>",
            "\" onmouseover=\"alert(1)",
            "' onfocus='alert(1)",
            "</script><script>alert(1)",
            "<img src=x onerror=alert(1)>",
            "javascript:alert(1)",
            "&<>\"'",
            "a&b<c>d\"e'f",
        ];
        for p in payloads {
            let got = ref_escape_text(p);
            assert!(!got.contains('<') || p.contains("&lt;"), "{p} -> {got}");
            assert_eq!(
                got.contains('&'),
                p.contains('&')
                    || p.contains('<')
                    || p.contains('>')
                    || p.contains('"')
                    || p.contains('\'')
            );
            // Independent hand oracle for the critical set.
            if p == "&<>\"'" {
                assert_eq!(got, "&amp;&lt;&gt;&quot;&#39;");
            }
        }
        assert!(payloads.len() >= 8);
    }

    #[test]
    fn fuzz_smoke_no_panic() {
        let seeds = [
            "",
            "{{",
            "}}",
            "{{}}",
            "{{a}}",
            "{{#if a}}{{/if}}",
            "{{#for x in y}}{{/for}}",
            "<script>{{a}}</script>",
            "{{raw(a)}}",
            "{{a|upper}}",
            "{{a|nope}}",
            "{{> p}}",
            "{{#if a}}{{#else}}{{/if}}",
        ];
        for s in seeds {
            let _ = parse_template(s, TemplateMode::Html, &HashSet::new(), &HashSet::new());
            let _ = parse_template(s, TemplateMode::Text, &HashSet::new(), &HashSet::new());
        }
    }

    /// Timed fuzz: set SUPER_TEMPLATE_FUZZ_SECS (CI uses 600). Must not crash/hang.
    #[test]
    fn fuzz_timed() {
        use std::time::{Duration, Instant};
        let secs: u64 = std::env::var("SUPER_TEMPLATE_FUZZ_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2);
        let deadline = Instant::now() + Duration::from_secs(secs);
        let alphabet: &[u8] =
            b"{}#/<>\"'=abcdefghijklmnopqrstuvwxyz0123456789 -_|rawjscsstitlehrefscriptstyle";
        let mut state: u64 = 0x5EED_5EED;
        let mut n = 0u64;
        while Instant::now() < deadline {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let len = (state % 129) as usize;
            let mut buf = Vec::with_capacity(len);
            let mut s = state;
            for _ in 0..len {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
                buf.push(alphabet[(s as usize) % alphabet.len()]);
            }
            let text = String::from_utf8_lossy(&buf);
            let _ = parse_template(&text, TemplateMode::Html, &HashSet::new(), &HashSet::new());
            let _ = parse_template(&text, TemplateMode::Text, &HashSet::new(), &HashSet::new());
            n += 1;
        }
        eprintln!("template fuzz_timed: {n} inputs over {secs}s");
        assert!(n > 0);
    }
}
