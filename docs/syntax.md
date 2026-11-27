# Super · Language reference

<h1>Language reference (0.1.0)</h1>
<p><strong><code>+</code> before a definition means exported.</strong> Every top-level definition without <code>+</code> is private to its module.</p>
<p><strong>Keywords are the only bare syntax.</strong> Every compile-time, user-extensible construct is a <code>#</code>-prefixed macro, such as <code>#assert(…)</code> or <code>#routes { … }</code>; everything else is a function or a method.</p>
<ul>
<li><strong>Declarations</strong> — bindings use <code>name =</code>; exported definitions use <code>+name =</code>. There is no <code>fn</code> or <code>let</code>.</li>
<li><code>@receiver</code> — in-place mutation calls (SPEC §D).</li>
<li><strong>UFCS</strong> — <code>value.method(args)</code> (SPEC §D).</li>
<li><code>it</code> / <code>index</code> — implicit in <code>for</code> / <code>when</code> / pipelines.</li>
<li><code>?</code> <code>!</code> <code>or</code> — optionals, value errors, recovery (SPEC §F).</li>
</ul>
<p><strong>Next:</strong> <a href="spec.html">Full spec</a> · <strong>Related:</strong> <a href="book.html">Book</a></p>
