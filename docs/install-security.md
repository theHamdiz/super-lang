# Super · Install security

<h1>Installer security</h1>
<ol>
<li>Download and read first: <code>curl -fsSL https://thehamdiz.github.io/super-lang/install.sh -o install.sh && less install.sh</code></li>
<li>Release SHA256: verify <code>SHA256SUMS</code> on GitHub release <code>v0.1.0</code> (see release assets).</li>
<li>Manual path: download <code>super-0.1.0-*.tar.gz</code>, extract <code>bin/super</code> and <code>stdlib/</code> under <code>~/.super/</code>.</li>
<li>Installer touches: <code>~/.super/bin</code>, <code>~/.super/lib/stdlib</code>, optional registry/cache dirs — never <code>~/.cargo</code>.</li>
</ol>
<p><strong>Next:</strong> <a href="install.html">Install</a> · <strong>Related:</strong> <a href="quickstart.html">Quickstart</a></p>
