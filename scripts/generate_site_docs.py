#!/usr/bin/env python3
"""Generate docs/*.html and *.md doc pages + llms.txt from repo sources."""
from __future__ import annotations

import html
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs"
STDLIB = ROOT / "stdlib"
sys.path.insert(0, str(ROOT / "scripts"))
from md_render import render as render_markdown  # noqa: E402
from site_urls import asset_href, canonical_url, page_href, write_pretty_page  # noqa: E402


def nav(active: str, from_slug: str) -> str:
    pages = [
        ("home", "Home", "home"),
        ("quickstart", "Quickstart", "quickstart"),
        ("docs-hub", "Docs", "docs"),
        ("spec", "Spec", "spec"),
        ("benchmarks", "Benchmarks", "benchmarks"),
        ("https://github.com/theHamdiz/super-lang", "GitHub", "github"),
        ("https://discord.gg/ZvgmreZsuY", "Discord", "discord"),
    ]
    links = []
    for href, label, key in pages:
        cur = ' aria-current="page"' if key == active else ""
        if href.startswith("http"):
            links.append(f'<a href="{href}" rel="noopener noreferrer"{cur}>{label}</a>')
        else:
            links.append(f'<a href="{page_href(from_slug, href)}"{cur}>{label}</a>')
    return f"""<header class="site-nav">
  <a class="mark" href="{page_href(from_slug, "home")}">Super</a>
  <nav>{"".join(links)}</nav>
  <a class="cta" href="{page_href(from_slug, "quickstart")}">Quickstart</a>
</header>"""


def head(title: str, desc: str, slug: str) -> str:
    url = canonical_url(slug)
    t = html.escape(title)
    d = html.escape(desc)
    css = asset_href(slug, "site.css")
    return f"""<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<link rel="canonical" href="{url}"/>
<meta name="description" content="{d}"/>
<meta property="og:type" content="website"/>
<meta property="og:title" content="{t}"/>
<meta property="og:description" content="{d}"/>
<meta property="og:url" content="{url}"/>
<link rel="stylesheet" href="{css}"/>
<title>{t}</title>"""


def wrap(active: str, title: str, desc: str, slug: str, body: str) -> str:
    return f"""<!DOCTYPE html>
<html lang="en">
<head>
{head(title, desc, slug)}
</head>
<body>
{nav(active, slug)}
<section class="section">
{body}
</section>
<footer class="site-footer"><p>Super 0.1.0 · <a href="{page_href(slug, "home")}">Home</a></p></footer>
</body>
</html>
"""


def stdlib_rows() -> tuple[list[str], int]:
    modules: list[tuple[str, str, bool, bool]] = []
    files = sorted(STDLIB.rglob("*.sr"))
    for p in files:
        if p.name in ("mod.sr",) or p.name.endswith(".law.sr") or p.name.endswith(".proof.sr"):
            continue
        rel = p.relative_to(STDLIB).as_posix()
        mod = rel[:-3].replace("/", ".")
        law = p.with_name(p.stem + ".law.sr")
        proof = p.with_name(p.stem + ".proof.sr")
        modules.append((mod, rel, law.is_file(), proof.is_file()))
    rows = []
    for mod, rel, has_law, has_proof in sorted(modules, key=lambda x: x[0]):
        badges = []
        if has_law:
            badges.append("law")
        if has_proof:
            badges.append("proof")
        badge = ", ".join(badges) if badges else "—"
        rows.append(
            f"<tr><td><code>{html.escape(mod)}</code></td>"
            f'<td><a href="https://github.com/theHamdiz/super-lang/blob/main/stdlib/{html.escape(rel)}">{html.escape(rel)}</a></td>'
            f"<td>{badge}</td></tr>"
        )
    return rows, len(modules)


def main() -> int:
    pages: dict[str, tuple[str, str, str, str]] = {
        "docs-hub.html": (
            "docs",
            "Super · Docs",
            "Documentation hub for Super 0.1.0.",
            """<h1>Documentation</h1>
<ul>
<li><a href="quickstart.html">Quickstart</a></li>
<li><a href="comptime.html">Comptime</a></li>
<li><a href="macros-pods.html">Macros and Pods</a></li>
<li><a href="law-proof.html">Law and proof</a></li>
<li><a href="testing.html">Testing</a></li>
<li><a href="http-server.html">HTTP server</a></li>
<li><a href="data-stacks.html">Data stacks</a></li>
<li><a href="stdlib-ref.html">Stdlib reference</a></li>
<li><a href="versioning.html">Versioning</a></li>
<li><a href="install-security.html">Install security</a></li>
<li><a href="syntax.html">Language reference</a></li>
<li><a href="cfg.html">Conditional compilation and Pod features</a></li>
<li><a href="features.html">Pod feature selection</a></li>
<li><a href="api-style.html">API Style Guide</a></li>
<li><a href="platform.html">Platform support</a></li>
<li><a href="safety-claims.html">Safety claims</a></li>
<li><a href="reproduce-benchmarks.html">Reproduce benchmarks (community)</a></li>
<li><a href="verifying-releases.html">Verifying releases</a></li>
<li><a href="https://github.com/theHamdiz/super-lang/blob/main/ROADMAP.md">Roadmap</a></li>
<li><a href="https://github.com/theHamdiz/super-lang/blob/main/SECURITY.md">Security</a></li>
</ul>
<p><strong>Next:</strong> <a href="quickstart.html">Quickstart</a> · <strong>Related:</strong> <a href="spec.html">Spec</a></p>""",
        ),
        "comptime.html": (
            "docs",
            "Super · Comptime",
            "Compile-time evaluation in Super.",
            """<h1>Comptime</h1>
<p>Constants, table literals, and compile-time branches are evaluated before AOT C23 emission or VM bytecode load. Restrictions: no I/O, no host calls in pure comptime tables unless explicitly allowed by the module.</p>
<pre class="sr-block"><code class="language-sr">+TABLE = [0, 1, 4, 9, 16]

+square(n: i64) : i64 {
  if n &lt; 0 { return 0 }
  if n &gt;= len(TABLE) { return n * n }
  TABLE[n]
}</code></pre>
<p>Run: <code>super check examples/doc_comptime/main.sr</code></p>
<p><strong>Next:</strong> <a href="macros-pods.html">Macros &amp; Pods</a> · <strong>Related:</strong> <a href="spec.html">Spec</a></p>""",
        ),
        "macros-pods.html": (
            "docs",
            "Super · Macros and Pods",
            "Source-distributed macros and Pod archives.",
            """<h1>Macros and Pods</h1>
<p>Macros ship as Super source (no opaque binary plugins). Pods are <strong>tar+zstd</strong> archives with a <code>pod.manifest</code> first entry — see <code>SPECIFICATION.md</code> §Pod.</p>
<pre class="sr-block"><code class="language-sr">// examples/doc_pod/
super check examples/doc_pod/main.sr
super pod verify examples/doc_pod</code></pre>
<p><strong>Next:</strong> <a href="law-proof.html">Law &amp; proof</a> · <strong>Related:</strong> <a href="quickstart.html">Quickstart</a></p>""",
        ),
        "law-proof.html": (
            "docs",
            "Super · Law and proof",
            "Law and proof sidecars beside modules.",
            """<h1>Law and proof tier</h1>
<p><code>*.law.sr</code> states invariants; <code>*.proof.sr</code> sketches discharge. <code>super verify</code> complements <code>super test</code> — it does not replace runtime tests.</p>
<pre class="sr-block"><code>super verify examples/04_law_verify/src/main.sr</code></pre>
<p>Expected (toolchain 0.1.0): <code>super verify: N passed, 0 failed</code> with structural + comptime laws.</p>
<p><strong>Next:</strong> <a href="testing.html">Testing</a> · <strong>Related:</strong> <a href="stdlib-ref.html">Stdlib</a></p>""",
        ),
        "testing.html": (
            "docs",
            "Super · Testing",
            "First-class test blocks.",
            """<h1>Testing</h1>
<p><code>test "name" { ... }</code> blocks live beside implementation and run with <code>super test</code>. Stripped from <code>super build --release</code>.</p>
<pre class="sr-block"><code class="language-sr">test "parses a port" {
  port = parse_port("8080")
  #assert(port &gt; 0)
  assert_eq(port, 8080)
  #assert_err(parse_port(""), "empty port")
}</code></pre>
<p><code>#assert(expr)</code> evaluates captured operands once and includes their values in a failure. Use <code>assert_eq(actual, expected)</code> when equality is the intent. <code>#assert_err(expr)</code> requires an error; its optional second argument requires the exact error text.</p>
<pre><code>super test examples/doc_test/</code></pre>
<p><strong>Next:</strong> <a href="data-stacks.html">Data stacks</a> · <strong>Related:</strong> <a href="law-proof.html">Law/proof</a></p>""",
        ),
        "http-server.html": (
            "docs",
            "Super · HTTP server",
            "Typed routes and the App.serve path.",
            """<h1>Build an HTTP server</h1>
<p>Route handlers have the static type <code>Fn[http.Request, http.Response]</code>. A route with the wrong arity or return type is rejected before the server starts.</p>
<pre class="sr-block"><code class="language-sr">use std.net.http

app = #routes {
  get "/health" {
    http.ok({status: "ok"})
  }
}

@app.serve("127.0.0.1:8080")</code></pre>
<p><code>#routes { ... }</code> builds an <code>App</code> through typed calls and rejects duplicate method/path pairs. The clean route methods are <code>get</code>, <code>post</code>, <code>put</code>, <code>patch</code>, and <code>delete</code>.</p>
<p><code>App.serve(address)</code> is the normal serving path. It installs graceful signal handling and uses the default server limits. Use <code>serve_with(address, ServeConfig)</code> to override limits. Applications that need direct lifecycle control may use <code>http.Server</code> and <code>sys.signal.Notify</code>.</p>
<p>This path is not allocation-free yet. Measured on the C23 AOT build, the hello server makes 256 heap allocations per request and frees 7 of them, so memory grows with the number of requests served. A test pins these counts. Request-scoped reclamation is not implemented; see the 0.1.0 changelog.</p>
<p><strong>Windows C23 AOT sockets: Winsock2 (WSAPoll) in 0.1.0.</strong> Treat production HTTP on Windows as Rust-hosted VM only until native Windows AOT socket CI is green.</p>
<p><strong>Next:</strong> <a href="testing.html">Test the API</a> · <strong>Related:</strong> <a href="api-style.html">API style</a> · <a href="platform.html">Platform support</a></p>""",
        ),
        "data-stacks.html": (
            "docs",
            "Super · Data stacks",
            "Scoped arenas and memory model overview.",
            """<h1>Data stacks</h1>
<p>Super allocates transient data on scoped bump stacks (lexical frame, HTTP request, game frame). Destination-driven APIs write through caller-provided buffers where possible, reducing allocator traffic — this is the primary driver of the published <strong>Memory</strong> axis result for SuperAot.</p>
<pre class="sr-block"><code class="language-sr">super run examples/doc_data_stack/main.sr</code></pre>
<h2>Limits and known gaps</h2>
<p>Non-<code>@</code> reference escape rules are specified in <code>SPECIFICATION.md</code> §lifetimes; full diagnostic coverage is still expanding in 0.1.0.</p>
<p><strong>Next:</strong> <a href="stdlib-ref.html">Stdlib</a> · <strong>Related:</strong> <a href="benchmarks.html">Benchmarks</a></p>""",
        ),
        "versioning.html": (
            "docs",
            "Super · Versioning",
            "Super 0.1.0 language and product version policy.",
            """<h1>Versioning and stability</h1>
<p>The product, crates, CLI, installers, site, and release tag are <strong>0.1.0</strong>. The language specification documents the Round 5 revision of Super 0.1.0; that revision is not a separate product version. The published benchmarks remain the frozen 0.1.0 measurement, taken on a single machine. Breaking syntax or stdlib changes may land until a future <strong>1.0</strong> stability promise. Releases follow <code>vMAJOR.MINOR.PATCH</code> tags; see <code>CHANGELOG.md</code>.</p>
<p><strong>Next:</strong> <a href="install-security.html">Install security</a> · <strong>Related:</strong> <a href="https://github.com/theHamdiz/super-lang/blob/main/CONTRIBUTING.md">Contributing</a></p>""",
        ),
        "install-security.html": (
            "docs",
            "Super · Install security",
            "curl|sh safety and manual install.",
            """<h1>Installer security</h1>
<ol>
<li>Download and read first: <code>curl -fsSL https://thehamdiz.github.io/super-lang/install.sh -o install.sh && less install.sh</code></li>
<li>Release SHA256: verify <code>SHA256SUMS</code> on GitHub release <code>v0.1.0</code> (see release assets).</li>
<li>Manual path: download <code>super-0.1.0-*.tar.gz</code>, extract <code>bin/super</code> and <code>stdlib/</code> under <code>~/.super/</code>.</li>
<li>Installer touches: <code>~/.super/bin</code>, <code>~/.super/lib/stdlib</code>, optional registry/cache dirs — never <code>~/.cargo</code>.</li>
</ol>
<p><strong>Next:</strong> <a href="install.html">Install</a> · <strong>Related:</strong> <a href="quickstart.html">Quickstart</a></p>""",
        ),
        "syntax.html": (
            "docs",
            "Super · Language reference",
            "Core syntax — declarations, UFCS, errors.",
            """<h1>Language reference (0.1.0)</h1>
<p><strong><code>+</code> before a definition means exported.</strong> Every top-level definition without <code>+</code> is private to its module.</p>
<p><strong>Keywords are the only bare syntax.</strong> Every compile-time, user-extensible construct is a <code>#</code>-prefixed macro, such as <code>#assert(…)</code> or <code>#routes { … }</code>; everything else is a function or a method.</p>
<ul>
<li><strong>Declarations</strong> — bindings use <code>name =</code>; exported definitions use <code>+name =</code>. There is no <code>fn</code> or <code>let</code>.</li>
<li><code>@receiver</code> — in-place mutation calls (SPEC §D).</li>
<li><strong>UFCS</strong> — <code>value.method(args)</code> (SPEC §D).</li>
<li><code>it</code> / <code>index</code> — implicit in <code>for</code> / <code>when</code> / pipelines.</li>
<li><code>?</code> <code>!</code> <code>or</code> — optionals, value errors, recovery (SPEC §F).</li>
</ul>
<p><strong>Next:</strong> <a href="spec.html">Full spec</a> · <strong>Related:</strong> <a href="book.html">Book</a></p>""",
        ),
        "platform.html": (
            "docs",
            "Super · Platform support",
            "Super 0.1.0 platform support tiers, PAL backends, JIT fallback, and release artifacts.",
            """<h1>Platform support</h1>
<p>Super <strong>0.1.0</strong> assigns Tier 1 to platforms with native CI host tests, Pod feature configurations, and a C23 hello-world. Tier 2 receives compile and native smoke coverage without the full production networking or release-runner guarantees. On Windows, Tier 1 covers the CLI, VM, and C23 console smoke only — <strong>Windows C23 AOT sockets: Winsock2 (WSAPoll) in 0.1.0.</strong></p>
<div class="table-wrap"><table class="data"><thead><tr><th>Platform</th><th>Tier</th><th>PAL (runtime)</th><th>AOT sockets</th><th>JIT</th><th>Archive suffix</th></tr></thead><tbody>
<tr><td>Linux x86_64 glibc</td><td>1</td><td>epoll + io_uring</td><td>supported (C23)</td><td>native + VM fallback</td><td><code>x86_64-unknown-linux-gnu.tar.gz</code></td></tr>
<tr><td>Linux ARM64 glibc</td><td>1</td><td>epoll + io_uring</td><td>supported (C23)</td><td>native + VM fallback</td><td><code>aarch64-unknown-linux-gnu.tar.gz</code></td></tr>
<tr><td>Linux x86_64 musl</td><td>1</td><td>epoll + io_uring</td><td>supported (C23)</td><td>native + VM fallback</td><td><code>x86_64-unknown-linux-musl.tar.gz</code></td></tr>
<tr><td>macOS x86_64</td><td>1</td><td>kqueue</td><td>supported (C23)</td><td>native + VM fallback</td><td><code>x86_64-apple-darwin.tar.gz</code></td></tr>
<tr><td>macOS ARM64</td><td>1</td><td>kqueue</td><td>supported (C23)</td><td>native + VM fallback</td><td><code>aarch64-apple-darwin.tar.gz</code></td></tr>
<tr><td>Windows x86_64</td><td>1</td><td>Rust host WSAPoll (VM)</td><td>Winsock2 (WSAPoll).</td><td>native + VM fallback</td><td><code>x86_64-pc-windows-gnu.zip</code></td></tr>
<tr><td>FreeBSD x86_64</td><td>2</td><td>kqueue</td><td>supported (C23)</td><td>native + VM fallback</td><td><code>x86_64-unknown-freebsd.tar.gz</code></td></tr>
</tbody></table></div>
<p><code>install.sh</code> supports Linux, macOS, and FreeBSD. Windows PowerShell 5.1 and 7+ use <code>install.ps1</code>. Both require SHA256 verification and abort on corrupt input.</p>
<p>Generated C23 avoids Linux-only headers on other targets. <strong>Windows C23 AOT sockets: Winsock2 (WSAPoll) in 0.1.0.</strong> C23 AOT binaries link Winsock2 (<code>-lws2_32</code>) and use <code>WSAPoll</code> for readiness. TLS/QUIC on Windows remain out of scope for 0.1.0.</p>
<p>WASI uses <code>super build --target wasm32-wasi</code> and is not an installer bundle.</p>
<p><strong>Next:</strong> <a href="install.html">Install</a> · <a href="cfg.html">Conditional compilation</a> · <a href="spec.html">Spec</a></p>""",
        ),
        "cfg.html": (
            "docs",
            "Super · Conditional compilation",
            "Super #cfg predicates and Pod feature selection.",
            """<h1>Conditional compilation and Pod features</h1>
<p>Super 0.1.0 evaluates <code>#cfg(...)</code> on complete items before imports, macros, type checking, laws/proofs, and code generation.</p>
<pre class="sr-block"><code class="language-sr">#cfg(target_os = "linux")
use std.sys.io_uring

#cfg(all(unix, feature = "tls"))
+serve() =&gt; start_tls()

#cfg(not(feature = "tls"))
+serve() =&gt; fail "TLS feature disabled"</code></pre>
<h2>Predicates</h2>
<p><code>all(...)</code>, <code>any(...)</code>, <code>not(...)</code>, <code>unix</code>, <code>windows</code>, <code>debug_assertions</code>, <code>test</code>, and value predicates for <code>target_os</code>, <code>target_arch</code>, <code>target_env</code>, <code>target_family</code>, and <code>feature</code>.</p>
<h2>Pod features</h2>
<pre><code>[features]
default = ["tls"]
tls = []
full-io = ["tls"]</code></pre>
<p>Build with <code>--features tls,full-io</code>, <code>--all-features</code>, or <code>--no-default-features</code>. Unknown features and malformed predicates fail the build. Disabled imports and law/proof items are removed before macro execution.</p>
<p><strong>Related:</strong> <a href="spec.html#311-conditional-compilation">Normative specification</a> · <a href="platform.html">Platform matrix</a></p>""",
        ),
        "features.html": (
            "docs",
            "Super · Pod features",
            "Declare Pod features and select them from the Super CLI.",
            """<h1>Pod features</h1>
<p>Pod manifests declare a closed feature set. Defaults are enabled unless <code>--no-default-features</code> is present, and feature dependencies are expanded transitively before <code>#cfg(feature = "...")</code> is evaluated.</p>
<pre><code>[features]
default = ["tls"]
tls = []
full-io = ["tls"]</code></pre>
<pre><code>super check --features tls,full-io src/main.sr
super test --all-features .
super build --no-default-features src/main.sr</code></pre>
<p><code>--features</code>, <code>--all-features</code>, and <code>--no-default-features</code> are accepted by feature-aware build, check, run, and test commands. Unknown feature names, dependency cycles, and malformed declarations fail before compilation.</p>
<p>Use <code>#cfg(feature = "tls")</code> on complete items. Disabled items are stripped before imports, macro expansion, semantic analysis, laws/proofs, and code generation.</p>
<p><strong>Related:</strong> <a href="cfg.html">Conditional compilation</a> · <a href="spec.html#311-conditional-compilation">Normative specification</a></p>""",
        ),
        "api-style.html": (
            "docs",
            "Super · API Style Guide",
            "Canonical names and cost-transparent APIs for Super contributors.",
            """<h1>API Style Guide</h1>
<p>Super uses one short canonical name for one concept. Receiver context avoids stutter: <code>users.len()</code>, <code>app.get(...)</code>, and <code>map.get(key)</code>.</p>
<ul>
<li>Use <code>snake_case</code> values/functions and <code>UpperCamelCase</code> types/variants.</li>
<li>Use verbs for actions and nouns or adjectives for values and predicates.</li>
<li>Let types constrain meaning: <code>len(text)</code> and <code>len(items)</code>, never type-prefixed duplicates.</li>
<li>Canonical collection operations are <code>len</code>, <code>get</code>, and <code>contains</code>.</li>
<li>Scope methods to their owning type. Use macros only for syntax, source capture, item generation, or comptime structure.</li>
<li>Do not hide allocation, blocking I/O, global mutation, or unrelated linear work behind a cheap-looking accessor.</li>
<li>Callback APIs statically constrain arity and return types; runtimes never silently discard callback values.</li>
</ul>
<p>During 0.x, canonical APIs may change in a breaking release. Complete each rename across compiler, stdlib, examples, tests, book, specification, site, agent text, and editor tooling. Do not retain aliases or shims.</p>
<p><strong>Full contributor policy:</strong> <a href="https://github.com/theHamdiz/super-lang/blob/main/CONTRIBUTING.md#api-style-guide">CONTRIBUTING.md</a></p>""",
        ),
    }

    extra_md = [
        ("safety-claims", "Safety claims", "What Super 0.1.0 does and does not verify.", "SAFETY_CLAIMS.md"),
        (
            "reproduce-benchmarks",
            "Reproduce benchmarks",
            "Community harness runs without overwriting frozen scores.",
            "REPRODUCE_BENCHMARKS.md",
        ),
        (
            "verifying-releases",
            "Verifying releases",
            "SHA256SUMS and planned attestations for 0.1.0 bundles.",
            "VERIFYING_RELEASES.md",
        ),
    ]
    for slug, short, desc, filename in extra_md:
        md_path = DOCS / filename
        if md_path.is_file():
            body = render_markdown(md_path.read_text(encoding="utf-8"))
            pages[f"{slug}.html"] = (
                "docs",
                f"Super · {short}",
                desc,
                body + f'<p><strong>Related:</strong> <a href="platform.html">Platform</a> · <a href="install-security.html">Install security</a></p>',
            )

    rows, mod_count = stdlib_rows()
    stdlib_body = f"""<h1>Standard library</h1>
<p><strong>{mod_count}</strong> implementation modules ({len(list(STDLIB.rglob('*.sr')))} <code>.sr</code> files including law/proof sidecars). Generated from <code>stdlib/</code>.</p>
<p><code>len(value)</code> and <code>value.len()</code> are the canonical length operations for arrays, strings, maps, buffers, and vectors. String length is measured in UTF-8 bytes; use <code>char_count(text)</code> for Unicode scalar values.</p>
<div class="table-wrap"><table class="data"><thead><tr><th>Module</th><th>Source</th><th>Law / proof</th></tr></thead><tbody>
{''.join(rows)}
</tbody></table></div>
<p><strong>Next:</strong> <a href="syntax.html">Syntax</a> · <strong>Related:</strong> <a href="spec.html">Spec</a></p>"""
    pages["stdlib-ref.html"] = ("docs", "Super · Stdlib", f"{mod_count} modules", stdlib_body)

    qs = (ROOT / "QUICKSTART.md").read_text(encoding="utf-8")
    pages["quickstart.html"] = (
        "quickstart",
        "Super · Quickstart",
        "Install, run, test, verify.",
        render_markdown(qs) + '<p><strong>Next:</strong> <a href="syntax.html">Language</a></p>',
    )

    for fname, (active, title, desc, body) in pages.items():
        slug = fname.replace(".html", "")
        html_out = wrap(active, title, desc, slug, body)
        write_pretty_page(DOCS, slug, html_out)
        md_path = DOCS / f"{slug}.md"
        md_path.write_text(f"# {title}\n\n{body}\n", encoding="utf-8")

    llms = f"""# Super (llms.txt)

Super 0.1.0 — stack-oriented language with C23 AOT, VM, and numeric JIT.

- Quickstart: https://thehamdiz.github.io/super-lang/quickstart/
- Language: https://thehamdiz.github.io/super-lang/syntax/
- HTTP server: https://thehamdiz.github.io/super-lang/http-server/
- API style: https://thehamdiz.github.io/super-lang/api-style/
- Conditional compilation: https://thehamdiz.github.io/super-lang/cfg/
- Pod features: https://thehamdiz.github.io/super-lang/features/
- Platforms: https://thehamdiz.github.io/super-lang/platform/
- Spec: https://thehamdiz.github.io/super-lang/spec/
- Stdlib ({mod_count} modules): https://thehamdiz.github.io/super-lang/stdlib-ref/
- Law/proof: https://thehamdiz.github.io/super-lang/law-proof/
- Data stacks: https://thehamdiz.github.io/super-lang/data-stacks/
- Home: https://thehamdiz.github.io/super-lang/
- Benchmarks: https://thehamdiz.github.io/super-lang/benchmarks/
- Reproduce: https://github.com/theHamdiz/super-lang/blob/main/benchmarks/REPRODUCE.md
- Community reproduce: https://thehamdiz.github.io/super-lang/reproduce-benchmarks/
- Safety claims: https://thehamdiz.github.io/super-lang/safety-claims/
- Roadmap: https://github.com/theHamdiz/super-lang/blob/main/ROADMAP.md
- Security: https://github.com/theHamdiz/super-lang/blob/main/SECURITY.md
- Windows note: Windows C23 AOT sockets: Winsock2 (WSAPoll) in 0.1.0.
- REPORT: https://github.com/theHamdiz/super-lang/blob/main/docs/REPORT.md
- results.json: https://github.com/theHamdiz/super-lang/blob/main/docs/results.json
"""
    (DOCS / "llms.txt").write_text(llms, encoding="utf-8")
    full = llms + "\n\n---\n\n" + (ROOT / "SPECIFICATION.md").read_text(encoding="utf-8")[:120000]
    (DOCS / "llms-full.txt").write_text(full, encoding="utf-8")

    print(f"Wrote {len(pages)} HTML pages; stdlib modules={mod_count}")
    return 0


if __name__ == "__main__":
    sys.exit(main())