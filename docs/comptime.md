# Super · Comptime

<h1>Comptime</h1>
<p>Constants, table literals, and compile-time branches are evaluated before AOT C23 emission or VM bytecode load. Restrictions: no I/O, no host calls in pure comptime tables unless explicitly allowed by the module.</p>
<pre class="sr-block"><code class="language-sr">+TABLE = [0, 1, 4, 9, 16]

+square(n: i64) : i64 {
  if n &lt; 0 { return 0 }
  if n &gt;= len(TABLE) { return n * n }
  TABLE[n]
}</code></pre>
<p>Run: <code>super check examples/doc_comptime/main.sr</code></p>
<p><strong>Next:</strong> <a href="macros-pods.html">Macros &amp; Pods</a> · <strong>Related:</strong> <a href="spec.html">Spec</a></p>
