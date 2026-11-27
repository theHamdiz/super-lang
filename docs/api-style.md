# Super · API Style Guide

<h1>API Style Guide</h1>
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
<p><strong>Full contributor policy:</strong> <a href="https://github.com/theHamdiz/super-lang/blob/main/CONTRIBUTING.md#api-style-guide">CONTRIBUTING.md</a></p>
