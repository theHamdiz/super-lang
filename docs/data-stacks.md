# Super · Data stacks

<h1>Data stacks</h1>
<p>Super allocates transient data on scoped bump stacks (lexical frame, HTTP request, game frame). Destination-driven APIs write through caller-provided buffers where possible, reducing allocator traffic — this is the primary driver of the published <strong>Memory</strong> axis result for SuperAot.</p>
<pre class="sr-block"><code class="language-sr">super run examples/doc_data_stack/main.sr</code></pre>
<h2>Limits and known gaps</h2>
<p>Non-<code>@</code> reference escape rules are specified in <code>SPECIFICATION.md</code> §lifetimes; full diagnostic coverage is still expanding in 0.1.0.</p>
<p><strong>Next:</strong> <a href="stdlib-ref.html">Stdlib</a> · <strong>Related:</strong> <a href="benchmarks.html">Benchmarks</a></p>
