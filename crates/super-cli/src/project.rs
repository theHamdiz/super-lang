use crate::pkg::PodKind;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Create a Pod of the given kind (`bin`, `lib`, or `macro`).
pub fn scaffold_kind(root: &Path, name: &str, kind: PodKind) -> io::Result<()> {
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("assets"))?;
    create_new(
        &root.join(crate::pkg::MANIFEST_POD),
        &crate::pkg::default_manifest_kind(name, kind),
    )?;
    create_new(&root.join("assets").join(".gitkeep"), "")?;
    let (source, contents) = match kind {
        PodKind::Lib => (
            "lib.sr",
            "/// Returns a friendly greeting.\n+greet(name: str) => \"Hello, {name}!\"\n",
        ),
        PodKind::Macro => (
            "macros.sr",
            "\
//! Distributable comptime macros for this Pod.\n\
\n\
/// Compile-time width available to importers via `+#`.\n\
+#WIDTH = 8\n\
\n\
/// Quote of `6 * 7` — splice with `${#ANSWER}` at use sites.\n\
+#ANSWER = '{ 6 * 7 }\n\
\n\
/// Specialize addition by a compile-time constant.\n\
+add_n(#n: i64, x: i64) : i64 => x + #n\n\
\n\
test \"WIDTH is eight\" {\n\
    assert_eq(#WIDTH, 8)\n\
}\n\
\n\
test \"add_n specializes\" {\n\
    assert_eq(add_n(3, 4), 7)\n\
}\n\
\n\
test \"ANSWER splices to forty-two\" {\n\
    assert_eq(${#ANSWER}, 42)\n\
}\n",
        ),
        PodKind::Bin => (
            "main.sr",
            "+main() {\n    print(\"Hello from Super!\")\n}\n",
        ),
    };
    create_new(&root.join("src").join(source), contents)?;
    create_new(&root.join(".gitignore"), "/target\n/vendor\n")?;
    Ok(())
}

fn create_new(path: &Path, contents: &str) -> io::Result<()> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(contents.as_bytes())
}

/// Result of formatting a set of files.
#[derive(Debug, Default)]
pub struct FormatReport {
    pub checked: usize,
    /// Files whose layout differs from `super fmt` output.
    pub changed: Vec<PathBuf>,
}

/// Format every `.sr` file under `paths`. With `check`, report files that would
/// change instead of writing them.
pub fn format_paths(paths: &[PathBuf], check: bool) -> io::Result<FormatReport> {
    let mut report = FormatReport::default();
    for path in paths {
        for file in collect_sources(path)? {
            let source = fs::read_to_string(&file)?;
            let formatted = super_frontend::format::format_source(&source).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {error}", file.display()),
                )
            })?;
            report.checked += 1;
            if source != formatted {
                if !check {
                    fs::write(&file, formatted)?;
                }
                report.changed.push(file);
            }
        }
    }
    Ok(report)
}

/// Result of linting a set of files.
#[derive(Debug, Default)]
pub struct LintReport {
    pub checked: usize,
    /// Mechanical fixes applied with `--fix`.
    pub fixed: usize,
    pub findings: Vec<(PathBuf, super_frontend::lint::Finding)>,
}

/// Lint every `.sr` file under `paths` with the `[lint]` table of its nearest
/// `Pod.toml`, or of `config` when given. Companion `*.law.sr` / `*.proof.sr`
/// files count as references.
pub fn lint_paths(paths: &[PathBuf], fix: bool, config: Option<&Path>) -> io::Result<LintReport> {
    use super_frontend::lint::{LintConfig, LintContext, apply_fixes, lint_source};
    let invalid = |message: String| io::Error::new(io::ErrorKind::InvalidData, message);
    let mut report = LintReport::default();
    let mut configs: std::collections::HashMap<PathBuf, (PathBuf, LintConfig)> = Default::default();
    let fixed_config = match config {
        Some(path) => {
            let parsed = crate::pkg::parse_manifest(path).map_err(invalid)?;
            let config = parsed.lint.to_config();
            config
                .validate()
                .map_err(|e| invalid(format!("{}: [lint] {e}", path.display())))?;
            let root = path.parent().unwrap_or(Path::new(".")).to_path_buf();
            Some((root, config))
        }
        None => None,
    };
    for path in paths {
        for file in collect_sources(path)? {
            let name = file.to_string_lossy();
            if name.ends_with(".law.sr") || name.ends_with(".proof.sr") {
                continue;
            }
            let (root, config) = if let Some(fixed) = &fixed_config {
                fixed.clone()
            } else if let Some(manifest) = crate::pkg::find_manifest(&file) {
                if !configs.contains_key(&manifest) {
                    let parsed = crate::pkg::parse_manifest(&manifest).map_err(invalid)?;
                    let config = parsed.lint.to_config();
                    config
                        .validate()
                        .map_err(|e| invalid(format!("{}: [lint] {e}", manifest.display())))?;
                    let root = manifest.parent().unwrap_or(Path::new(".")).to_path_buf();
                    configs.insert(manifest.clone(), (root, config));
                }
                configs[&manifest].clone()
            } else {
                (PathBuf::from("."), LintConfig::default())
            };
            let relative = file
                .strip_prefix(&root)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            let display = file.to_string_lossy().replace('\\', "/");
            let mut ctx = LintContext::for_path(&display);
            let stem = file.with_extension("");
            for suffix in ["law.sr", "proof.sr"] {
                if let Ok(text) = fs::read_to_string(stem.with_extension(suffix)) {
                    ctx.companions.push(text);
                }
            }
            let mut source = fs::read_to_string(&file)?;
            let lint = |source: &str| {
                lint_source(source, &ctx)
                    .map(|findings| config.apply(&relative, findings))
                    .map_err(|e| invalid(format!("{}: {e}", file.display())))
            };
            let mut findings = lint(&source)?;
            if fix && findings.iter().any(|f| f.fix.is_some()) {
                let fixed = apply_fixes(&source, &findings);
                report.fixed += findings.iter().filter(|f| f.fix.is_some()).count();
                fs::write(&file, &fixed)?;
                source = fixed;
                findings = lint(&source)?;
            }
            report.checked += 1;
            report
                .findings
                .extend(findings.into_iter().map(|f| (file.clone(), f)));
        }
    }
    Ok(report)
}

fn collect_sources(path: &Path) -> io::Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    let mut files = Vec::new();
    fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                if !matches!(
                    path.file_name().and_then(|s| s.to_str()),
                    Some(".git" | "target" | "vendor")
                ) {
                    walk(&path, files)?;
                }
            } else if path.extension().and_then(|s| s.to_str()) == Some("sr") {
                files.push(path);
            }
        }
        Ok(())
    }
    walk(path, &mut files)?;
    files.sort();
    Ok(files)
}

pub fn generate_docs(path: &Path) -> io::Result<String> {
    let mut markdown = String::from("# API documentation\n\n");
    for file in collect_sources(path)? {
        let source = fs::read_to_string(&file)?;
        let label = file.display().to_string();
        markdown.push_str(&render_source_docs(&label, &source));
    }
    Ok(markdown)
}

/// Render Markdown from `///` / `//!` / `/**` / `/*!` comments attached by the parser.
pub fn render_source_docs(label: &str, source: &str) -> String {
    use super_core::{Diagnostics, Interner};
    use super_frontend::ast::Item;
    use super_frontend::{Lexer, Parser};

    let mut interner = Interner::with_capacity(128);
    let mut diagnostics = Diagnostics::new();
    let tokens = Lexer::new(source, &mut interner, &mut diagnostics).tokenize();
    let mut parser = Parser::new(&tokens, source, &mut interner, &mut diagnostics);
    let module = parser.parse_module();
    let mut out = String::new();
    out.push_str("## `");
    out.push_str(label);
    out.push_str("`\n\n");
    let module_docs = interner.resolve(module.docs);
    if !module_docs.is_empty() {
        out.push_str(module_docs);
        out.push_str("\n\n");
    }
    for item in &module.items {
        match item {
            Item::Function(func) => {
                push_decl(
                    &mut out,
                    "###",
                    &decl_line(source, func.span),
                    interner.resolve(func.docs),
                );
            }
            Item::Macro(def) => {
                push_decl(
                    &mut out,
                    "###",
                    &decl_line(source, def.span),
                    interner.resolve(def.docs),
                );
            }
            Item::Binding(binding) => {
                push_decl(
                    &mut out,
                    "###",
                    &decl_line(source, binding.span),
                    interner.resolve(binding.docs),
                );
            }
            Item::Struct(def) => {
                push_decl(
                    &mut out,
                    "###",
                    &decl_line(source, def.span),
                    interner.resolve(def.docs),
                );
                for field in &def.fields {
                    push_decl(
                        &mut out,
                        "####",
                        &decl_line(source, field.span),
                        interner.resolve(field.docs),
                    );
                }
                for method in &def.methods {
                    push_decl(
                        &mut out,
                        "####",
                        &decl_line(source, method.span),
                        interner.resolve(method.docs),
                    );
                }
            }
            Item::Sum(def) => {
                push_decl(
                    &mut out,
                    "###",
                    &decl_line(source, def.span),
                    interner.resolve(def.docs),
                );
                for variant in &def.variants {
                    push_decl(
                        &mut out,
                        "####",
                        &decl_line(source, variant.span),
                        interner.resolve(variant.docs),
                    );
                }
                for method in &def.methods {
                    push_decl(
                        &mut out,
                        "####",
                        &decl_line(source, method.span),
                        interner.resolve(method.docs),
                    );
                }
            }
            Item::State(state) => {
                push_decl(
                    &mut out,
                    "###",
                    &decl_line(source, state.span),
                    interner.resolve(state.docs),
                );
            }
            Item::Test(test) => {
                push_decl(
                    &mut out,
                    "###",
                    &decl_line(source, test.span),
                    interner.resolve(test.docs),
                );
            }
            Item::Rewrite(rule) => {
                push_decl(
                    &mut out,
                    "###",
                    &decl_line(source, rule.span),
                    interner.resolve(rule.docs),
                );
            }
            Item::Use(import) => {
                push_decl(
                    &mut out,
                    "###",
                    &decl_line(source, import.span),
                    interner.resolve(import.docs),
                );
            }
            Item::Trait(def) => {
                push_decl(
                    &mut out,
                    "###",
                    &decl_line(source, def.span),
                    interner.resolve(def.docs),
                );
                for method in &def.methods {
                    push_decl(
                        &mut out,
                        "####",
                        &decl_line(source, method.span),
                        interner.resolve(method.docs),
                    );
                }
            }
        }
    }
    out
}

fn push_decl(out: &mut String, heading: &str, title: &str, docs: &str) {
    if docs.is_empty() && !title.starts_with('+') {
        return;
    }
    out.push_str(heading);
    out.push_str(" `");
    out.push_str(title.trim());
    out.push_str("`\n\n");
    if !docs.is_empty() {
        out.push_str(docs);
        out.push_str("\n\n");
    }
}

fn decl_line(source: &str, span: super_core::Span) -> String {
    let lo = span.lo.as_usize().min(source.len());
    source[lo..].lines().next().unwrap_or("").trim().to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn doc_renderer_uses_parser_attachments() {
        let md = super::render_source_docs(
            "lib.sr",
            "//! Greetings.\n/// Returns a friendly greeting.\n+greet(name: str) => \"Hello, {name}!\"\n",
        );
        assert!(md.contains("Greetings."));
        assert!(md.contains("Returns a friendly greeting."));
        assert!(md.contains("greet(name: str)"));
    }

    #[test]
    fn lint_paths_reads_pod_config_and_fixes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("Pod.toml"),
            "[pod]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[lint]\ndeny = [\"S001\"]\n\n[[lint.exception]]\nrule = \"unused\"\npath = \"src/main.sr\"\nitem = \"kept\"\nreason = \"used by the next release\"\n",
        )
        .unwrap();
        let file = dir.path().join("src/main.sr");
        std::fs::write(
            &file,
            "P = {\n    x: i64\n}\nkept() => 1\nBadName() => 2\n+main() {\n    x = 1\n    print(P{x: x}.x)\n    print(BadName())\n}\n",
        )
        .unwrap();
        let paths = vec![dir.path().to_path_buf()];
        let report = super::lint_paths(&paths, false, None).unwrap();
        let found: Vec<_> = report
            .findings
            .iter()
            .map(|(_, f)| (f.rule.id(), f.severity))
            .collect();
        use super_frontend::lint::Severity;
        assert_eq!(
            found,
            [("S001", Severity::Error), ("S008", Severity::Warning)]
        );
        let report = super::lint_paths(&paths, true, None).unwrap();
        assert_eq!(report.fixed, 1);
        assert!(
            std::fs::read_to_string(&file)
                .unwrap()
                .contains("print(P{x}.x)")
        );
        assert_eq!(report.findings.len(), 1);
    }

    #[test]
    fn lint_paths_rejects_bad_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Pod.toml"), "[lint]\nallow = [\"S042\"]\n").unwrap();
        std::fs::write(dir.path().join("a.sr"), "+f() => 1\n").unwrap();
        let error = super::lint_paths(&[dir.path().to_path_buf()], false, None).unwrap_err();
        assert!(
            error.to_string().contains("unknown lint rule `S042`"),
            "{error}"
        );
    }

    #[test]
    fn lint_paths_uses_an_explicit_config_for_trees_without_a_pod() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("lib/net")).unwrap();
        std::fs::write(
            dir.path().join("lib/net/a.sr"),
            "helper() => 1\n+f() => 2\n",
        )
        .unwrap();
        let paths = vec![dir.path().join("lib")];
        assert_eq!(
            super::lint_paths(&paths, false, None)
                .unwrap()
                .findings
                .len(),
            1
        );
        let config = dir.path().join("lib/lint.toml");
        std::fs::write(
            &config,
            "[[lint.exception]]\nrule = \"S003\"\npath = \"net/a.sr\"\nreason = \"fixture\"\n",
        )
        .unwrap();
        assert!(
            super::lint_paths(&paths, false, Some(&config))
                .unwrap()
                .findings
                .is_empty()
        );
    }

    #[test]
    fn format_paths_checks_without_writing_then_writes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.sr");
        std::fs::write(&file, "+main() {\nprint(\"x\")\n}\n").unwrap();
        let paths = vec![dir.path().to_path_buf()];
        let report = super::format_paths(&paths, true).unwrap();
        assert_eq!(report.checked, 1);
        assert_eq!(report.changed, vec![file.clone()]);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "+main() {\nprint(\"x\")\n}\n"
        );
        super::format_paths(&paths, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "+main() {\n    print(\"x\")\n}\n"
        );
        assert!(
            super::format_paths(&paths, true)
                .unwrap()
                .changed
                .is_empty()
        );
    }
}
