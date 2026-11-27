# Super · Conditional compilation

<h1>Conditional compilation and Pod features</h1>
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
<p><strong>Related:</strong> <a href="spec.html#311-conditional-compilation">Normative specification</a> · <a href="platform.html">Platform matrix</a></p>
