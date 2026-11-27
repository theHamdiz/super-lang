use std::fs;
use std::process::Command;

fn super_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_super"))
}

#[test]
fn check_rejects_non_response_route_handler_with_route_context() {
    let temp = tempfile::tempdir().unwrap();
    let bad = temp.path().join("bad_route.sr");
    fs::write(
        &bad,
        r#"
use std.net.http
+main() {
    app = App.new()
    @app.get("/ping", |_req| { "pong" })
}
"#,
    )
    .unwrap();
    let output = super_command()
        .args(["check", bad.to_str().unwrap()])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("route \"/ping\" (App.get)"), "{stderr}");
    assert!(stderr.contains("Response"), "{stderr}");
    assert!(stderr.contains("str"), "{stderr}");
    assert!(stderr.contains("http.ok"), "{stderr}");

    let good = temp.path().join("good_route.sr");
    fs::write(
        &good,
        r#"
use std.net.http
+main() {
    app = App.new()
    @app.get("/ping", |_req| { ok({pong: true}) })
}
"#,
    )
    .unwrap();
    let output = super_command()
        .args(["check", good.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn routes_block_preserves_handler_diagnostics() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("routes_type_error.sr");
    fs::write(
        &source,
        r#"
use std.net.http
+main() {
    app = #routes {
        get "/macro-ping" { "pong" }
    }
}
"#,
    )
    .unwrap();
    let output = super_command()
        .args(["check", source.to_str().unwrap()])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains("route \"/macro-ping\" (App.get)"),
        "{stderr}"
    );
    assert!(stderr.contains("Response"), "{stderr}");
    assert!(stderr.contains("str"), "{stderr}");
    assert!(stderr.contains("\"pong\""), "{stderr}");
}

#[test]
fn fmt_check_stdin_and_in_place_round_trip() {
    use std::io::Write;
    let ugly = "+main() {\nprint(\"x\")   \n\n\n}\n";
    let pretty = "+main() {\n    print(\"x\")\n}\n";
    let mut child = super_command()
        .args(["fmt", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(ugly.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), pretty);

    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("main.sr");
    fs::write(&file, ugly).unwrap();
    let check = super_command()
        .args(["fmt", "--check"])
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(check.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&check.stdout).contains("main.sr"));
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        ugly,
        "--check must not write"
    );
    assert!(
        super_command()
            .arg("fmt")
            .arg(&file)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), pretty);
    let again = super_command()
        .args(["fmt", "--check"])
        .arg(&file)
        .output()
        .unwrap();
    assert!(again.status.success());
}

#[test]
fn fmt_refuses_files_that_do_not_lex() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("bad.sr");
    fs::write(&file, "x = \"open\n").unwrap();
    let output = super_command().arg("fmt").arg(&file).output().unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&file).unwrap(), "x = \"open\n");
}

#[test]
fn lint_reports_fixes_and_lists_rules() {
    let rules = super_command().args(["lint", "--rules"]).output().unwrap();
    let table = String::from_utf8_lossy(&rules.stdout);
    for id in [
        "S001", "S002", "S003", "S004", "S005", "S006", "S007", "S008",
    ] {
        assert!(table.contains(id), "{table}");
    }
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("app.sr");
    fs::write(
        &file,
        "+P = {\n    +x: i64\n}\n+make(x: i64) : P => P{x: x}\n",
    )
    .unwrap();
    let output = super_command().arg("lint").arg(&file).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("app.sr:2:5: warning[S007 redundant-plus]"),
        "{stdout}"
    );
    assert!(stdout.contains("warning[S008 redundant-pun]"), "{stdout}");
    let fixed = super_command()
        .args(["lint", "--fix"])
        .arg(&file)
        .output()
        .unwrap();
    assert!(
        fixed.status.success(),
        "{}",
        String::from_utf8_lossy(&fixed.stderr)
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "+P = {\n    x: i64\n}\n+make(x: i64) : P => P{x}\n"
    );
    let check = super_command().arg("check").arg(&file).output().unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
fn old_bare_forms_point_to_their_macros() {
    let temp = tempfile::tempdir().unwrap();
    for (name, body) in [
        ("old_assert.sr", "test \"t\" {\n    assert(1 == 1)\n}\n"),
        (
            "old_assert_err.sr",
            "f() : i64! { fail \"x\" }\ntest \"t\" {\n    assert_err(f())\n}\n",
        ),
        (
            "old_routes.sr",
            "use std.net.http\n+main() {\n    app = routes {\n        get \"/x\" { ok(1) }\n    }\n}\n",
        ),
    ] {
        let source = temp.path().join(name);
        fs::write(&source, body).unwrap();
        let output = super_command()
            .args(["check", source.to_str().unwrap()])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{name}: {stderr}");
        let macro_name = match name {
            "old_assert.sr" => "assert",
            "old_assert_err.sr" => "assert_err",
            _ => "routes",
        };
        assert!(
            stderr.contains(&format!("`{macro_name}` is a macro: write `#{macro_name}(")),
            "{name}: {stderr}"
        );
    }
}

#[test]
fn user_binding_named_routes_is_an_ordinary_value() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("routes_value.sr");
    fs::write(
        &source,
        r#"
use std.net.http
test "routes is just a name" {
    routes = ["/a", "/b"]
    assert_eq(len(routes), 2)
    app = #routes {
        get "/a" { ok("a") }
    }
    tc = TestClient.new(app)
    assert_eq(@tc.get("/a").status, 200)
}
"#,
    )
    .unwrap();
    let output = super_command()
        .args(["test", source.to_str().unwrap()])
        .output()
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{combined}");
    assert!(combined.contains("2 assertion(s) passed"), "{combined}");
}

#[test]
fn assert_captures_expression_and_evaluates_once() {
    let temp = tempfile::tempdir().unwrap();
    let passing = temp.path().join("assert_once.sr");
    fs::write(
        &passing,
        r#"
touch(@value: i64) : bool {
    value += 1
    true
}
test "once" {
    value = 0
    #assert(touch(@value))
    assert_eq(value, 1)
}
"#,
    )
    .unwrap();
    let output = super_command()
        .args(["test", passing.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let failing = temp.path().join("assert_message.sr");
    fs::write(
        &failing,
        r#"
use std.core.str

test "message" {
    body = "hello"
    #assert(contains(body, "x"))
}
"#,
    )
    .unwrap();
    let output = super_command()
        .args(["test", failing.to_str().unwrap()])
        .output()
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "{combined}");
    assert!(combined.contains("assertion failed at"), "{combined}");
    assert!(combined.contains("contains(body, \"x\")"), "{combined}");
    assert!(combined.contains("body ="), "{combined}");
    assert!(combined.contains("hello"), "{combined}");
    assert!(combined.contains("\"x\" ="), "{combined}");
    assert!(combined.contains("Bool(false)"), "{combined}");
}

#[test]
fn assert_err_checks_error_values_and_optional_text() {
    let temp = tempfile::tempdir().unwrap();
    let passing = temp.path().join("assert_err.sr");
    fs::write(
        &passing,
        r#"
fails() : i64! {
    fail "boom"
}
test "error assertion" {
    #assert_err(fails())
    #assert_err(fails(), "boom")
}
"#,
    )
    .unwrap();
    let output = super_command()
        .args(["test", passing.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("2 assertion(s) passed"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );

    let failing = temp.path().join("assert_err_value.sr");
    fs::write(
        &failing,
        r#"
test "expected error" {
    #assert_err(42)
}
"#,
    )
    .unwrap();
    let output = super_command()
        .args(["test", failing.to_str().unwrap()])
        .output()
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "{combined}");
    assert!(combined.contains("expected error"), "{combined}");
    assert!(combined.contains("\n42\n"), "{combined}");
}

#[test]
fn init_scaffolds_a_checkable_package() {
    let temp = tempfile::tempdir().unwrap();
    let status = super_command()
        .arg("init")
        .current_dir(temp.path())
        .status()
        .unwrap();
    assert!(status.success());
    let manifest = fs::read_to_string(temp.path().join("Pod.toml")).unwrap();
    assert!(manifest.contains("[pod]"));
    assert!(manifest.contains("kind = \"bin\""));
    assert!(manifest.contains("[profile.test]"));
    assert!(manifest.contains("[profile.release]"));
    assert!(manifest.contains("lto = \"fat\""));
    assert!(manifest.contains("profile = \"dev\""));
    assert!(manifest.contains("backend = \"c23\""));
    assert!(manifest.contains("mode = \"aot\""));
    assert!(temp.path().join("src/main.sr").is_file());
    assert!(temp.path().join("assets/.gitkeep").is_file());
    assert!(temp.path().join(".gitignore").is_file());

    let status = super_command()
        .arg("check")
        .current_dir(temp.path())
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn build_selects_cfg_items_from_pod_features() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(
        temp.path().join("Pod.toml"),
        r#"
[pod]
name = "cfg-features"
version = "0.1.0"
[features]
default = ["pro"]
pro = []
"#,
    )
    .unwrap();
    fs::write(
        temp.path().join("src/main.sr"),
        r#"
#cfg(feature = "pro")
+message() : str => "pro"
#cfg(not(feature = "pro"))
+message() : str => "base"
+main() { print(message()) }
"#,
    )
    .unwrap();

    for args in [
        vec!["build"],
        vec!["build", "--no-default-features"],
        vec!["build", "--all-features"],
        vec!["build", "--features", "pro"],
    ] {
        let output = super_command()
            .args(args)
            .current_dir(temp.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let unknown = super_command()
        .args(["build", "--features", "missing"])
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(!unknown.status.success());
    assert!(
        String::from_utf8_lossy(&unknown.stderr).contains("unknown Pod feature"),
        "{}",
        String::from_utf8_lossy(&unknown.stderr)
    );
}

#[test]
fn test_command_sets_cfg_test_before_collecting_tests() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("cfg_test.sr");
    fs::write(
        &source,
        r#"
#cfg(test)
test "enabled during super test" { assert_eq(1, 1) }

#cfg(not(test))
test "must be stripped" { assert_eq(1, 2) }
"#,
    )
    .unwrap();

    let output = super_command().arg("test").arg(&source).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(
        stdout.contains("1 assertion(s) passed") || stderr.contains("1 assertion(s) passed"),
        "{stdout}\n{stderr}"
    );
}

#[test]
fn new_macro_pod_scaffolds_and_tests() {
    let temp = tempfile::tempdir().unwrap();
    let status = super_command()
        .args(["new", "--macro", "doubler"])
        .current_dir(temp.path())
        .status()
        .unwrap();
    assert!(status.success());
    let root = temp.path().join("doubler");
    let manifest = fs::read_to_string(root.join("Pod.toml")).unwrap();
    assert!(manifest.contains("kind = \"macro\""));
    assert!(root.join("src/macros.sr").is_file());
    let tested = super_command()
        .arg("test")
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        tested.status.success(),
        "{}",
        String::from_utf8_lossy(&tested.stderr)
    );
}

#[test]
fn consumer_imports_macro_pod() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    assert!(
        super_command()
            .args(["new", "--macro", "doubler"])
            .current_dir(temp.path())
            .status()
            .unwrap()
            .success()
    );
    assert!(
        super_command()
            .args(["new", "--bin", "app"])
            .current_dir(temp.path())
            .status()
            .unwrap()
            .success()
    );
    fs::write(
        app.join("src/main.sr"),
        "use doubler\n+main() {\n    print(add_n(2, 5))\n}\n",
    )
    .unwrap();
    assert!(
        super_command()
            .args(["add", "../doubler"])
            .current_dir(&app)
            .status()
            .unwrap()
            .success()
    );
    let checked = super_command()
        .arg("check")
        .current_dir(&app)
        .output()
        .unwrap();
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
}

#[test]
fn add_path_dependency_and_check_import() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    let utils = temp.path().join("utils");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::create_dir_all(utils.join("src")).unwrap();
    fs::write(
        app.join("Pod.toml"),
        "[pod]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
    )
    .unwrap();
    fs::write(
        utils.join("Pod.toml"),
        "[pod]\nname = \"utils\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
    )
    .unwrap();
    fs::write(
        app.join("src/main.sr"),
        "use utils\n+main() {\n    print(answer())\n}\n",
    )
    .unwrap();
    fs::write(utils.join("src/lib.sr"), "+answer() : i64 => 42\n").unwrap();

    let status = super_command()
        .args(["add", "../utils"])
        .current_dir(&app)
        .status()
        .unwrap();
    assert!(status.success());
    assert!(app.join("Pod.lock").is_file());

    let status = super_command()
        .arg("check")
        .current_dir(&app)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn build_reads_super_toml_profile_and_fetches_dependency() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    let utils = temp.path().join("utils");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::create_dir_all(utils.join("src")).unwrap();
    fs::write(
        app.join("Pod.toml"),
        r#"[pod]
name = "app"
version = "0.1.0"

[build]
profile = "release"

[dependencies]
utils = { path = "../utils" }
"#,
    )
    .unwrap();
    fs::write(
        utils.join("Pod.toml"),
        "[pod]\nname = \"utils\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
    )
    .unwrap();
    fs::write(
        app.join("src/main.sr"),
        "use utils\n+main() {\n    print(answer())\n}\n",
    )
    .unwrap();
    fs::write(utils.join("src/lib.sr"), "+answer() : i64 => 42\n").unwrap();

    let from_src = super_command()
        .arg("r")
        .current_dir(app.join("src"))
        .output()
        .unwrap();
    assert!(
        from_src.status.success(),
        "run from package subdir failed: {}",
        String::from_utf8_lossy(&from_src.stderr)
    );
    assert!(String::from_utf8_lossy(&from_src.stdout).contains("42"));
    assert!(app.join("Pod.lock").is_file());

    let built = super_command().arg("b").current_dir(&app).output().unwrap();
    assert!(
        built.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    let stdout = String::from_utf8_lossy(&built.stdout);
    let stderr = String::from_utf8_lossy(&built.stderr);
    assert!(
        !stdout.contains("42"),
        "build must not run the program:\n{stdout}"
    );
    assert!(stderr.contains("Finished"), "{stderr}");
    assert!(stderr.contains("Compiling"), "{stderr}");
    assert!(stderr.contains("profile release"), "{stderr}");
    assert!(stderr.contains("-flto"), "{stderr}");
    assert!(stderr.contains("-funroll-loops"), "{stderr}");
    assert!(stderr.contains("[aot c23]"), "{stderr}");
    let bin = app.join("target/release/app");
    assert!(bin.is_file(), "release build should be a binary: {stderr}");
    let bytes = fs::read(&bin).unwrap();
    assert!(
        bytes.starts_with(&[0x7f, b'E', b'L', b'F']),
        "expected an ELF executable, got {} bytes starting {:?}",
        bytes.len(),
        &bytes[..bytes.len().min(16)]
    );
    assert!(!app.join("target/release/app.superb").is_file());
    let ran = Command::new(&bin).output().unwrap();
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert!(String::from_utf8_lossy(&ran.stdout).contains("42"));

    let debug = super_command()
        .args(["b", "-d"])
        .current_dir(&app)
        .output()
        .unwrap();
    assert!(
        debug.status.success(),
        "{}",
        String::from_utf8_lossy(&debug.stderr)
    );
    let stdout = String::from_utf8_lossy(&debug.stdout);
    let stderr = String::from_utf8_lossy(&debug.stderr);
    assert!(stderr.contains("profile dev"), "{stderr}\n{stdout}");
    assert!(stderr.contains("-O0"), "{stderr}");
    assert!(stderr.contains("Finished"), "{stderr}");
    assert!(app.join("target/dev/app").is_file(), "{stderr}");
    assert!(!app.join("target/dev/app.superb-dev").is_file());
}

#[test]
fn bytecode_mode_writes_a_module_instead_of_a_binary() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("hello");
    fs::create_dir_all(&app).unwrap();
    assert!(
        super_command()
            .arg("init")
            .current_dir(&app)
            .status()
            .unwrap()
            .success()
    );
    let built = super_command()
        .args(["b", "--mode", "bytecode"])
        .current_dir(&app)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&built.stderr);
    assert!(built.status.success(), "{stderr}");
    assert!(stderr.contains("Writing"), "{stderr}");
    assert!(stderr.contains("Finished"), "{stderr}");
    assert!(stderr.contains("[bytecode"), "{stderr}");
    assert!(!stderr.contains("Compiling"), "{stderr}");
    let module = fs::read_to_string(app.join("target/dev/hello.superb-dev")).unwrap();
    assert!(module.starts_with("superb1 "), "{module}");
    assert!(!app.join("target/dev/hello").exists());
}

#[test]
fn help_lists_every_command() {
    let help = super_command().arg("--help").output().unwrap();
    assert!(
        help.status.success(),
        "{}",
        String::from_utf8_lossy(&help.stderr)
    );
    let text = String::from_utf8_lossy(&help.stdout);
    for name in [
        "new",
        "init",
        "fmt",
        "doc",
        "add",
        "remove",
        "update",
        "fetch",
        "vendor",
        "publish",
        "completions",
        "run",
        "test",
        "check",
        "build",
        "install",
        "uninstall",
        "list",
        "which",
        "clean",
        "emit-c",
        "lsp",
        "bench",
        "bindgen",
        "disasm",
        "help",
    ] {
        assert!(text.contains(name), "missing `{name}` in:\n{text}");
    }
    let install = super_command().args(["help", "install"]).output().unwrap();
    let install_text = String::from_utf8_lossy(&install.stdout);
    assert!(install.status.success(), "{install_text}");
    assert!(install_text.contains("PATH"), "{install_text}");
    assert!(install_text.contains("-O3"), "{install_text}");
    let clean = super_command().args(["clean", "--help"]).output().unwrap();
    let clean_text = String::from_utf8_lossy(&clean.stdout);
    assert!(clean.status.success(), "{clean_text}");
    assert!(clean_text.contains("-d"), "{clean_text}");
    assert!(clean_text.contains("-r"), "{clean_text}");
    assert!(clean_text.contains("-a"), "{clean_text}");
    assert!(
        clean_text.to_ascii_lowercase().contains("default"),
        "{clean_text}"
    );
}

#[test]
fn install_ships_a_release_binary_and_uninstall_removes_it() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("hello");
    let bin_dir = temp.path().join("bin");
    let data = temp.path().join("data");
    fs::create_dir_all(&app).unwrap();
    let status = super_command()
        .arg("init")
        .current_dir(&app)
        .status()
        .unwrap();
    assert!(status.success());

    let installed = super_command()
        .args(["i"])
        .current_dir(app.join("src"))
        .env("SUPER_INSTALL_DIR", &bin_dir)
        .env("SUPER_DATA_DIR", &data)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&installed.stdout);
    let stderr = String::from_utf8_lossy(&installed.stderr);
    assert!(
        installed.status.success(),
        "install failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stderr.contains("profile release"), "{stderr}");
    assert!(stderr.contains("-O3"), "{stderr}");
    assert!(stderr.contains("-flto"), "{stderr}");
    assert!(stderr.contains("Finished"), "{stderr}");
    assert!(stdout.contains("installed hello"), "{stdout}");
    let bin = bin_dir.join(if cfg!(windows) { "hello.exe" } else { "hello" });
    assert!(bin.is_file(), "missing {}", bin.display());

    let ran = Command::new(&bin).output().unwrap();
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert!(String::from_utf8_lossy(&ran.stdout).contains("Hello from Super!"));

    let listed = super_command()
        .arg("list")
        .env("SUPER_DATA_DIR", &data)
        .output()
        .unwrap();
    assert!(listed.status.success());
    assert!(String::from_utf8_lossy(&listed.stdout).contains("hello"));

    let which = super_command()
        .args(["which", "hello"])
        .env("SUPER_DATA_DIR", &data)
        .output()
        .unwrap();
    assert!(which.status.success());
    assert_eq!(
        String::from_utf8_lossy(&which.stdout).trim(),
        bin.display().to_string()
    );

    let removed = super_command()
        .args(["u", "hello"])
        .env("SUPER_DATA_DIR", &data)
        .output()
        .unwrap();
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(!bin.exists());
}

#[test]
fn clean_debug_release_and_all_keep_the_other_profile() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("hello");
    fs::create_dir_all(&app).unwrap();
    assert!(
        super_command()
            .arg("init")
            .current_dir(&app)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        super_command()
            .args(["b", "-d"])
            .current_dir(&app)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        super_command()
            .args(["b", "-r"])
            .current_dir(&app)
            .status()
            .unwrap()
            .success()
    );
    assert!(app.join("target/dev").is_dir());
    assert!(app.join("target/release").is_dir());

    let debug = super_command()
        .args(["clean", "-d"])
        .current_dir(app.join("src"))
        .output()
        .unwrap();
    assert!(
        debug.status.success(),
        "{}",
        String::from_utf8_lossy(&debug.stderr)
    );
    assert!(String::from_utf8_lossy(&debug.stdout).contains("debug"));
    assert!(!app.join("target/dev").exists());
    assert!(app.join("target/release").is_dir());

    let release = super_command()
        .args(["clean", "-r"])
        .current_dir(&app)
        .output()
        .unwrap();
    assert!(release.status.success());
    assert!(!app.join("target/release").exists());

    assert!(
        super_command()
            .args(["b", "-d"])
            .current_dir(&app)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        super_command()
            .args(["b", "-r"])
            .current_dir(&app)
            .status()
            .unwrap()
            .success()
    );
    let all = super_command()
        .arg("clean")
        .current_dir(&app)
        .output()
        .unwrap();
    assert!(
        all.status.success(),
        "{}",
        String::from_utf8_lossy(&all.stderr)
    );
    assert!(String::from_utf8_lossy(&all.stdout).contains("all"));
    assert!(!app.join("target").exists());
}

#[test]
fn install_refuses_a_library_package() {
    let temp = tempfile::tempdir().unwrap();
    let status = super_command()
        .args(["new", "--lib", "utils"])
        .current_dir(temp.path())
        .status()
        .unwrap();
    assert!(status.success());
    let installed = super_command()
        .arg("install")
        .current_dir(temp.path().join("utils"))
        .env("SUPER_INSTALL_DIR", temp.path().join("bin"))
        .env("SUPER_DATA_DIR", temp.path().join("data"))
        .output()
        .unwrap();
    assert!(!installed.status.success());
    let stderr = String::from_utf8_lossy(&installed.stderr);
    assert!(
        stderr.contains("src/main.sr") || stderr.contains("library"),
        "{stderr}"
    );
}

#[test]
fn check_alias_colors_steps_and_accepts_nested_c_types() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("nested");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(
        app.join("Pod.toml"),
        "[pod]\nname = \"nested\"\nversion = \"0.1.0\"\n\n[build]\nprofile = \"dev\"\nbackend = \"c23\"\nmode = \"aot\"\n",
    )
    .unwrap();
    fs::write(
        app.join("src/payload.sr"),
        "+Payload = { n: i64 = 1 }\n+Mode = {\n On\n Off\n}\n",
    )
    .unwrap();
    fs::write(
        app.join("src/main.sr"),
        "use payload\n+Holder = {\n value: Payload\n flag: Mode\n}\n+main() {\n    print(4)\n}\n",
    )
    .unwrap();

    let help = super_command().arg("--help").output().unwrap();
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(help.status.success(), "{help_text}");
    assert!(help_text.contains("[alias: c]"), "{help_text}");

    let checked = super_command()
        .arg("c")
        .current_dir(&app)
        .env("FORCE_COLOR", "1")
        .env_remove("NO_COLOR")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&checked.stderr);
    assert!(checked.status.success(), "{stderr}");
    assert!(stderr.contains("Checking"), "{stderr}");
    assert!(stderr.contains("Compiling"), "{stderr}");
    assert!(stderr.contains("cc -fsyntax-only"), "{stderr}");
    assert!(stderr.contains("Finished"), "{stderr}");
    assert!(stderr.contains("\u{1b}[38;2;0;214;170m"), "{stderr}");
    assert!(app.join("target/check/nested.c").is_file(), "{stderr}");
}

#[test]
fn targets_lists_catalog_including_wasm() {
    let out = super_command().arg("targets").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("wasm32-wasi"), "{stdout}");
    assert!(stdout.contains("x86_64-unknown-linux-gnu"), "{stdout}");
    assert!(stdout.contains("host:"), "{stdout}");
}

#[test]
fn build_wasm32_wasi_writes_nested_artifact() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("wasm_app");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(
        app.join("Pod.toml"),
        r#"[pod]
name = "wasm_app"
version = "0.1.0"
kind = "bin"

[build]
profile = "dev"
backend = "c23"
mode = "aot"
target = "wasm32-wasi"
"#,
    )
    .unwrap();
    fs::write(app.join("src/main.sr"), "+main() {\n    print(1)\n}\n").unwrap();

    let built = super_command().arg("b").current_dir(&app).output().unwrap();
    let stderr = String::from_utf8_lossy(&built.stderr);
    if !built.status.success()
        && (stderr.contains("bits/libc-header-start.h")
            || stderr.contains("unable to find")
            || stderr.contains("No such file")
            || !cross_cc_available("wasm32-wasi"))
    {
        eprintln!("skip wasm32-wasi build: no usable wasm/cross toolchain\n{stderr}");
        return;
    }
    assert!(
        built.status.success(),
        "wasm build failed: {stderr}\n{}",
        String::from_utf8_lossy(&built.stdout)
    );
    assert!(stderr.contains("wasm32-wasi"), "{stderr}");
    let wasm = app.join("target/wasm32-wasi/dev/wasm_app.wasm");
    assert!(wasm.is_file(), "missing {wasm:?}: {stderr}");
    let bytes = fs::read(&wasm).unwrap();
    assert!(
        bytes.starts_with(&[0x00, 0x61, 0x73, 0x6d]),
        "expected wasm magic, got {:?}",
        &bytes[..bytes.len().min(8)]
    );
}

#[test]
fn build_cli_target_overrides_manifest_and_cross_compiles() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("cross_app");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(
        app.join("Pod.toml"),
        r#"[pod]
name = "cross_app"
version = "0.1.0"
kind = "bin"

[build]
profile = "dev"
backend = "c23"
mode = "aot"
"#,
    )
    .unwrap();
    fs::write(app.join("src/main.sr"), "+main() {\n    print(2)\n}\n").unwrap();

    let built = super_command()
        .args(["b", "--target", "aarch64-unknown-linux-gnu"])
        .current_dir(&app)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&built.stderr);
    if !built.status.success()
        && (stderr.contains("bits/libc-header-start.h")
            || stderr.contains("unable to find")
            || stderr.contains("No such file")
            || !cross_cc_available("aarch64-unknown-linux-gnu"))
    {
        eprintln!("skip aarch64 cross build: no usable cross toolchain\n{stderr}");
        return;
    }
    assert!(
        built.status.success(),
        "cross build failed: {stderr}\n{}",
        String::from_utf8_lossy(&built.stdout)
    );
    let bin = app.join("target/aarch64-unknown-linux-gnu/dev/cross_app");
    assert!(bin.is_file(), "missing {bin:?}: {stderr}");
    let file = std::process::Command::new("file")
        .arg(&bin)
        .output()
        .unwrap();
    let desc = String::from_utf8_lossy(&file.stdout);
    assert!(
        desc.contains("ARM") || desc.contains("aarch64"),
        "expected aarch64 binary, got {desc}"
    );
}

/// True when `zig cc` or a matching clang/gcc cross front-end is on PATH.
fn cross_cc_available(triple: &str) -> bool {
    if Command::new("zig")
        .arg("version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        return true;
    }
    let candidates = [
        format!("{triple}-gcc"),
        format!("{triple}-clang"),
        "clang".to_string(),
    ];
    candidates.into_iter().any(|c| {
        Command::new(c)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

#[test]
fn mcp_stdio_initialize_and_overview() {
    let mut child = super_command()
        .arg("mcp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{}}}}"#
        )
        .unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"super_overview","arguments":{{}}}}}}"#
        )
        .unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":3,"method":"exit"}}"#).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("super-mcp"), "{stdout}");
    assert!(stdout.contains("stdlib_modules"), "{stdout}");
    assert!(stdout.contains("2024-11-05"), "{stdout}");
}

#[test]
fn pod_pack_extract_verify_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Pod.toml"),
        r#"[pod]
name = "cli_pod"
version = "0.1.0"
kind = "lib"
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/lib.sr"),
        r#"
+answer() : i64 => 7

test "answer" {
    assert_eq(answer(), 7)
}
"#,
    )
    .unwrap();
    let pod = root.join("cli_pod.pod");
    let out = super_command()
        .args(["pod", "pack", ".", "-o"])
        .arg(&pod)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "pack failed: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(pod.is_file());
    assert!(root.join("Pod.lock").is_file());
    let lock = fs::read_to_string(root.join("Pod.lock")).unwrap();
    assert!(lock.contains("content_hash") && lock.contains("sha256:"));

    let dest = root.join("extracted");
    let extracted = super_command()
        .args(["pod", "extract"])
        .arg(&pod)
        .args(["-d"])
        .arg(&dest)
        .output()
        .unwrap();
    assert!(
        extracted.status.success(),
        "{}",
        String::from_utf8_lossy(&extracted.stderr)
    );
    assert!(dest.join("src/lib.sr").is_file());
    assert!(dest.join("pod.manifest").is_file());

    let verified = super_command()
        .args(["pod", "verify"])
        .arg(&pod)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
}

/// Pack gates must compile the JIT path without executing `main` — otherwise
/// HTTP/server entrypoints hang forever after "listening on …".
#[test]
fn pod_pack_gate_does_not_execute_blocking_main() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Pod.toml"),
        r#"[pod]
name = "pack_no_serve"
version = "0.1.0"
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/main.sr"),
        r#"
+main() {
    print("listening on 127.0.0.1:3999")
    // Busy work that would hang a pack gate if main were executed.
    n = 0
    for 0..200000000 {
        n += 1
    }
    n
}

test "smoke" {
    assert_eq(1, 1)
}
"#,
    )
    .unwrap();
    let pod = root.join("out.pod");
    let mut child = super_command()
        .args(["pod", "pack", ".", "-o"])
        .arg(&pod)
        .current_dir(root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
    let status = loop {
        match child.try_wait().unwrap() {
            Some(status) => break status,
            None if std::time::Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("pod pack hung — JIT gate likely executed blocking main");
            }
            None => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    };
    let stdout = {
        let mut buf = Vec::new();
        if let Some(mut out) = child.stdout.take() {
            let _ = std::io::Read::read_to_end(&mut out, &mut buf);
        }
        String::from_utf8_lossy(&buf).into_owned()
    };
    let stderr = {
        let mut buf = Vec::new();
        if let Some(mut err) = child.stderr.take() {
            let _ = std::io::Read::read_to_end(&mut err, &mut buf);
        }
        String::from_utf8_lossy(&buf).into_owned()
    };
    assert!(status.success(), "pack failed: {stderr}\n{stdout}");
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("pod pack gate: VM + JIT + C23 green"),
        "missing pack gate success: {combined}"
    );
    assert!(
        !combined.contains("listening on 127.0.0.1:3999"),
        "pack must not run main/listen: {combined}"
    );
    assert!(pod.is_file());
}
