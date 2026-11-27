# Super · Pod features

<h1>Pod features</h1>
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
<p><strong>Related:</strong> <a href="cfg.html">Conditional compilation</a> · <a href="spec.html#311-conditional-compilation">Normative specification</a></p>
