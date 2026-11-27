# Super · Testing

<h1>Testing</h1>
<p><code>test "name" { ... }</code> blocks live beside implementation and run with <code>super test</code>. Stripped from <code>super build --release</code>.</p>
<pre class="sr-block"><code class="language-sr">test "parses a port" {
  port = parse_port("8080")
  #assert(port &gt; 0)
  assert_eq(port, 8080)
  #assert_err(parse_port(""), "empty port")
}</code></pre>
<p><code>#assert(expr)</code> evaluates captured operands once and includes their values in a failure. Use <code>assert_eq(actual, expected)</code> when equality is the intent. <code>#assert_err(expr)</code> requires an error; its optional second argument requires the exact error text.</p>
<pre><code>super test examples/doc_test/</code></pre>
<p><strong>Next:</strong> <a href="data-stacks.html">Data stacks</a> · <strong>Related:</strong> <a href="law-proof.html">Law/proof</a></p>
