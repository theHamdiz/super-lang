//! Automatic outlining of oversized C function bodies.
//!
//! Aggressive comptime unrolling and monomorphization can produce single C functions
//! large enough that GCC/Clang refuse to inline callees, spill locals, or degrade
//! register allocation. When a lowered body exceeds
//! [`super::DEFAULT_OUTLINE_STMT_THRESHOLD`], cohesive regions (outer `for` loops first,
//! then contiguous statement packs) are extracted into helper `static` functions that
//! take an explicit frame pointer (`T *restrict fr`).

use std::fmt::Write as _;

/// Default maximum emitted C statements before a function body is outlined.
pub const DEFAULT_OUTLINE_STMT_THRESHOLD: usize = 1500;

/// One local / parameter field that lives in an outlining frame.
#[derive(Clone, Debug)]
pub struct FrameField {
    pub cty: String,
    pub name: String,
}

/// Result of outlining one oversized function definition.
#[derive(Clone, Debug)]
pub struct OutlineResult {
    /// `typedef struct { … } <frame_ty>;` plus helper `static` definitions.
    pub helpers: String,
    /// Replacement function body (decls + calls into helpers + epilogue).
    pub body: String,
    /// Number of helper functions emitted.
    #[allow(dead_code)] // inspected by unit tests and diagnostics
    pub helper_count: usize,
}

/// Count approximate C statements in `src` (semicolons + control-flow openers).
pub fn count_stmts(src: &str) -> usize {
    let mut n = 0usize;
    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("/*") || t.starts_with("//") || t.starts_with("*") {
            continue;
        }
        n += t.matches(';').count();
        if t.starts_with("for ")
            || t.starts_with("for(")
            || t.starts_with("while ")
            || t.starts_with("while(")
            || t.starts_with("if ")
            || t.starts_with("if(")
            || t.starts_with("switch ")
            || t.starts_with("switch(")
        {
            n += 1;
        }
    }
    n
}

/// Parse declaration lines like `    int64_t l0_i = 0;` into frame fields.
pub fn parse_decl_fields(decls: &str) -> Vec<FrameField> {
    let mut fields = Vec::new();
    for line in decls.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("/*") {
            continue;
        }
        // Skip stack arrays: `alignas(64) double a0_buf[32];`
        if t.contains('[') {
            continue;
        }
        // `size_t sup_mark = g_sup_stack.cursor;`
        if let Some(field) = parse_scalar_decl(t) {
            fields.push(field);
        }
    }
    fields
}

fn parse_scalar_decl(t: &str) -> Option<FrameField> {
    let t = t.trim_end_matches(';').trim();
    // Strip initializer.
    let lhs = t.split('=').next()?.trim();
    // Normalize `T *name` / `T* name` / `T * name` into separate tokens.
    let normalized = lhs.replace('*', " * ");
    let mut parts: Vec<&str> = normalized
        .split_whitespace()
        .filter(|p| {
            !matches!(
                *p,
                "static" | "const" | "volatile" | "register" | "alignas(64)"
            )
        })
        .collect();
    if parts.len() < 2 {
        return None;
    }
    let name = parts.pop()?.to_string();
    if !is_ident(&name) {
        return None;
    }
    // Join type tokens: `int64_t` + `*` → `int64_t *`
    let mut cty = String::new();
    for p in parts {
        if p == "*" {
            if !cty.is_empty() && !cty.ends_with(' ') {
                cty.push(' ');
            }
            cty.push('*');
        } else {
            if !cty.is_empty() {
                cty.push(' ');
            }
            cty.push_str(p);
        }
    }
    if !cty.ends_with(' ') {
        cty.push(' ');
    }
    Some(FrameField { cty, name })
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Split a function body into top-level regions: each outer `for`/`while` is one region,
/// and runs of other statements are packed until they approach `pack_limit`.
///
/// OpenMP pragma lines immediately above a loop are glued to that loop region so outlining
/// never inserts statements between `#pragma omp …` and its `for`.
pub fn split_regions(body: &str, pack_limit: usize) -> Vec<String> {
    let lines: Vec<&str> = body.lines().collect();
    let mut regions = Vec::new();
    let mut i = 0usize;
    let mut pack = String::new();
    let mut pack_stmts = 0usize;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        let is_loop = (trimmed.starts_with("for ")
            || trimmed.starts_with("for(")
            || trimmed.starts_with("while ")
            || trimmed.starts_with("while("))
            && trimmed.contains('{');

        if is_loop {
            // Peel trailing OpenMP / par-note lines out of the current pack and attach them
            // to the loop region (must sit directly above the `for`).
            let mut glued = String::new();
            if !pack.is_empty() {
                let mut pack_lines: Vec<&str> = pack.lines().collect();
                let mut peel: Vec<String> = Vec::new();
                while let Some(last) = pack_lines.last() {
                    let t = last.trim();
                    if t.is_empty()
                        || t.starts_with("#pragma omp")
                        || t.starts_with("/* par:")
                        || t.starts_with("/*@OMP")
                    {
                        peel.push(pack_lines.pop().unwrap().to_string());
                        continue;
                    }
                    break;
                }
                pack = pack_lines.iter().map(|l| format!("{l}\n")).collect();
                pack_stmts = count_stmts(&pack);
                for line in peel.into_iter().rev() {
                    glued.push_str(&line);
                    glued.push('\n');
                }
                if !pack.is_empty() {
                    regions.push(std::mem::take(&mut pack));
                    pack_stmts = 0;
                }
            }
            let (block, next) = take_brace_block(&lines, i);
            glued.push_str(&block);
            regions.push(glued);
            i = next;
            continue;
        }

        pack.push_str(line);
        pack.push('\n');
        pack_stmts += count_stmts(line);
        i += 1;
        if pack_stmts >= pack_limit {
            regions.push(std::mem::take(&mut pack));
            pack_stmts = 0;
        }
    }
    if !pack.is_empty() {
        regions.push(pack);
    }
    // Merge tiny adjacent regions so we do not emit a helper per semicolon.
    merge_small_regions(regions, pack_limit)
}

fn take_brace_block(lines: &[&str], start: usize) -> (String, usize) {
    let mut out = String::new();
    let mut depth = 0i32;
    let mut i = start;
    while i < lines.len() {
        let line = lines[i];
        out.push_str(line);
        out.push('\n');
        for ch in line.chars() {
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }
        i += 1;
        if depth <= 0 && i > start {
            break;
        }
    }
    (out, i)
}

fn merge_small_regions(regions: Vec<String>, pack_limit: usize) -> Vec<String> {
    if regions.is_empty() {
        return regions;
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_n = 0usize;
    for r in regions {
        let n = count_stmts(&r);
        if cur.is_empty() {
            cur = r;
            cur_n = n;
            continue;
        }
        if cur_n + n <= pack_limit {
            cur.push_str(&r);
            cur_n += n;
        } else {
            out.push(std::mem::take(&mut cur));
            cur = r;
            cur_n = n;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Outline `body` when it exceeds `threshold` statements.
///
/// `fn_stem` is the C function name without signature (e.g. `super_u_hot_3`).
/// `fields` are locals/params that must round-trip through the frame.
/// `param_names` are by-value parameters already in scope in the outer function — they are
/// copied into the frame before the first helper call.
pub fn outline_if_needed(
    fn_stem: &str,
    decls: &str,
    body: &str,
    fields: &[FrameField],
    threshold: usize,
) -> Option<OutlineResult> {
    if threshold == 0 {
        return None;
    }
    let total = count_stmts(body);
    if total <= threshold {
        return None;
    }
    // Target ~half the threshold per helper so GCC/Clang stay well under inlining cliffs.
    let pack_limit = (threshold / 2).max(64);
    let regions = split_regions(body, pack_limit);
    if regions.len() <= 1
        && count_stmts(regions.first().map(|s| s.as_str()).unwrap_or("")) <= threshold
    {
        // Single region still oversized (one giant loop) — outline its interior.
        return outline_giant_loop(fn_stem, decls, body, fields, threshold);
    }
    if regions.len() <= 1 {
        return outline_giant_loop(fn_stem, decls, body, fields, threshold);
    }

    let frame_ty = format!("{fn_stem}_fr");
    let mut helpers = String::new();
    let _ = writeln!(
        helpers,
        "/* outlined: {total} stmts > threshold {threshold} */"
    );
    let _ = writeln!(helpers, "typedef struct {frame_ty} {{");
    for f in fields {
        emit_frame_field(&mut helpers, f);
    }
    // Always keep a scratch int for loop indices when fields are empty.
    if fields.is_empty() {
        let _ = writeln!(helpers, "  int64_t _outline_pad;");
    }
    let _ = writeln!(helpers, "}} {frame_ty};");

    let _ = decls; // Locals stay declared in the outer function; helpers get them via `fr`.
    let mut new_body = String::new();
    let _ = writeln!(new_body, "  {frame_ty} fr;");
    for f in fields {
        let _ = writeln!(new_body, "  fr.{} = {};", f.name, f.name);
    }

    let mut helper_count = 0usize;
    for (hi, region) in regions.iter().enumerate() {
        let n = count_stmts(region);
        // OpenMP regions must stay intact for the host compiler's nest analysis.
        if region.contains("#pragma omp") {
            for f in fields {
                let _ = writeln!(new_body, "  {} = fr.{};", f.name, f.name);
            }
            new_body.push_str(region);
            for f in fields {
                let _ = writeln!(new_body, "  fr.{} = {};", f.name, f.name);
            }
            continue;
        }
        // Keep tiny prologue/epilogue packs inline in the outer function.
        if n < 8 && (hi == 0 || hi + 1 == regions.len()) {
            // Sync from frame → locals for packs that still run in the outer fn.
            for f in fields {
                let _ = writeln!(new_body, "  {} = fr.{};", f.name, f.name);
            }
            new_body.push_str(region);
            for f in fields {
                let _ = writeln!(new_body, "  fr.{} = {};", f.name, f.name);
            }
            continue;
        }
        let hname = format!("{fn_stem}_ol{hi}");
        let _ = writeln!(helpers, "static void {hname}({frame_ty} *restrict fr) {{");
        for f in fields {
            let _ = writeln!(helpers, "  {}{} = fr->{};", f.cty, f.name, f.name);
        }
        helpers.push_str(&indent_block(region, "  "));
        for f in fields {
            let _ = writeln!(helpers, "  fr->{} = {};", f.name, f.name);
        }
        let _ = writeln!(helpers, "}}");
        let _ = writeln!(new_body, "  {hname}(&fr);");
        helper_count += 1;
    }

    for f in fields {
        let _ = writeln!(new_body, "  {} = fr.{};", f.name, f.name);
    }

    if helper_count == 0 {
        return None;
    }
    Some(OutlineResult {
        helpers,
        body: new_body,
        helper_count,
    })
}

fn indent_block(block: &str, prefix: &str) -> String {
    let mut out = String::new();
    for line in block.lines() {
        if line.trim().is_empty() {
            out.push('\n');
            continue;
        }
        // Body lines already carry their own indent; keep relative shape under `prefix`.
        let stripped = line.strip_prefix("    ").unwrap_or(line);
        out.push_str(prefix);
        out.push_str(stripped);
        out.push('\n');
    }
    out
}

fn emit_frame_field(out: &mut String, f: &FrameField) {
    // `cty` already carries a trailing space (`int64_t ` / `int64_t * `).
    let _ = writeln!(out, "  {}{};", f.cty, f.name);
}

/// Outline the interior of a single giant `for`/`while` when the whole body is one region.
fn outline_giant_loop(
    fn_stem: &str,
    decls: &str,
    body: &str,
    fields: &[FrameField],
    threshold: usize,
) -> Option<OutlineResult> {
    let lines: Vec<&str> = body.lines().collect();
    let mut loop_start = None;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if (t.starts_with("for ")
            || t.starts_with("for(")
            || t.starts_with("while ")
            || t.starts_with("while("))
            && t.contains('{')
        {
            loop_start = Some(i);
            break;
        }
    }
    let start = loop_start?;
    // Pull immediately-preceding OpenMP pragmas with the loop so the nest stays valid.
    let mut pragma_lo = start;
    while pragma_lo > 0 {
        let prev = lines[pragma_lo - 1].trim();
        if prev.is_empty()
            || prev.starts_with("#pragma omp")
            || prev.starts_with("/* par:")
            || prev.starts_with("/*@OMP")
        {
            pragma_lo -= 1;
            continue;
        }
        break;
    }
    let (loop_block, after) = take_brace_block(&lines, start);
    let loop_stmts = count_stmts(&loop_block);
    if loop_stmts <= threshold {
        return None;
    }

    // Peel header `for (...) {` and trailing `}`.
    let mut loop_lines: Vec<&str> = loop_block.lines().collect();
    if loop_lines.len() < 3 {
        return None;
    }
    let header = loop_lines.remove(0).to_string();
    let induct = for_induction_var(&header);
    // Drop closing brace line.
    while loop_lines
        .last()
        .is_some_and(|l| l.trim() == "}" || l.trim().is_empty())
    {
        let last = loop_lines.last().map(|s| s.trim().to_string());
        loop_lines.pop();
        if last.as_deref() == Some("}") {
            break;
        }
    }
    let interior: String = loop_lines.iter().map(|l| format!("{l}\n")).collect();
    if count_stmts(&interior) < 8 {
        return None;
    }

    let frame_ty = format!("{fn_stem}_fr");
    let hname = format!("{fn_stem}_ol_loop");
    let mut helpers = String::new();
    let _ = writeln!(
        helpers,
        "/* outlined giant loop: {loop_stmts} stmts > threshold {threshold} */"
    );
    let _ = writeln!(helpers, "typedef struct {frame_ty} {{");
    for f in fields {
        emit_frame_field(&mut helpers, f);
    }
    if fields.is_empty() {
        let _ = writeln!(helpers, "  int64_t _outline_pad;");
    }
    let _ = writeln!(helpers, "}} {frame_ty};");
    let induct_param = induct
        .as_ref()
        .map(|v| format!(", int64_t {v}"))
        .unwrap_or_default();
    let _ = writeln!(
        helpers,
        "static void {hname}({frame_ty} *restrict fr{induct_param}) {{"
    );
    for f in fields {
        let _ = writeln!(helpers, "  {}{} = fr->{};", f.cty, f.name, f.name);
    }
    helpers.push_str(&indent_block(&interior, "  "));
    for f in fields {
        let _ = writeln!(helpers, "  fr->{} = {};", f.name, f.name);
    }
    let _ = writeln!(helpers, "}}");

    let _ = decls;
    let mut new_body = String::new();
    // Preamble before any OpenMP pragma attached to the loop.
    for line in &lines[..pragma_lo] {
        new_body.push_str(line);
        new_body.push('\n');
    }
    let _ = writeln!(new_body, "  {frame_ty} fr;");
    for f in fields {
        let _ = writeln!(new_body, "  fr.{} = {};", f.name, f.name);
    }
    // Re-emit pragma lines so they sit directly above the `for`.
    for line in &lines[pragma_lo..start] {
        new_body.push_str(line);
        new_body.push('\n');
    }
    // Rewrite loop to call helper each iteration.
    new_body.push_str(&header);
    new_body.push('\n');
    let induct_arg = induct
        .as_ref()
        .map(|v| format!(", {v}"))
        .unwrap_or_default();
    let _ = writeln!(new_body, "    {hname}(&fr{induct_arg});");
    let _ = writeln!(new_body, "  }}");
    for f in fields {
        let _ = writeln!(new_body, "  {} = fr.{};", f.name, f.name);
    }
    for line in &lines[after..] {
        new_body.push_str(line);
        new_body.push('\n');
    }

    Some(OutlineResult {
        helpers,
        body: new_body,
        helper_count: 1,
    })
}

/// Extract `i` from `for (int64_t i = …;` / `for (i = …;`.
fn for_induction_var(header: &str) -> Option<String> {
    let after_for = header.split_once("for")?.1;
    let inside = after_for.split_once('(')?.1.split_once(';')?.0;
    let lhs = inside.split('=').next()?.trim();
    let name = lhs.split_whitespace().last()?.to_string();
    if is_ident(&name) { Some(name) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_stmts_ignores_comments_and_blank_lines() {
        let src =
            "\n  // hi\n  x = 1;\n  y = 2;\n  for (int i = 0; i < 3; ++i) {\n    z = i;\n  }\n";
        assert!(count_stmts(src) >= 4);
    }

    #[test]
    fn parse_decl_fields_extracts_scalars() {
        let decls =
            "    int64_t l0_i = 0;\n    double l1_f = 0.0;\n    int64_t *l2_ai = nullptr;\n";
        let f = parse_decl_fields(decls);
        assert_eq!(f.len(), 3);
        assert_eq!(f[0].name, "l0_i");
        assert!(f[0].cty.contains("int64_t"));
        assert_eq!(f[2].name, "l2_ai");
    }

    #[test]
    fn outline_splits_oversized_body_into_helpers() {
        let mut body = String::new();
        for i in 0..200 {
            body.push_str(&format!("    x = x + {i};\n"));
        }
        body.push_str("    for (int64_t it = 0; it < 10; ++it) {\n");
        for i in 0..200 {
            body.push_str(&format!("      x = x + {i};\n"));
        }
        body.push_str("    }\n");
        for i in 0..200 {
            body.push_str(&format!("    x = x + {i};\n"));
        }
        let fields = vec![FrameField {
            cty: "int64_t ".into(),
            name: "x".into(),
        }];
        let r = outline_if_needed(
            "super_u_stress_1",
            "    int64_t x = 0;\n",
            &body,
            &fields,
            150,
        )
        .expect("should outline");
        assert!(r.helper_count >= 1, "helpers={}", r.helper_count);
        assert!(r.helpers.contains("typedef struct super_u_stress_1_fr"));
        assert!(r.helpers.contains("*restrict fr"));
        assert!(r.body.contains("_ol"));
        // Outer function keeps typed locals; helpers promote via frame → C locals.
        assert!(r.helpers.contains("int64_t x = fr->x;"));
    }

    #[test]
    fn below_threshold_is_untouched() {
        let body = "    x = 1;\n    return x;\n";
        let fields = vec![FrameField {
            cty: "int64_t ".into(),
            name: "x".into(),
        }];
        assert!(outline_if_needed("f", "    int64_t x = 0;\n", body, &fields, 1500).is_none());
    }
}
