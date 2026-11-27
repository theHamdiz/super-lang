# Super · Verifying releases

<h1 id="verifying-super-releases-010">Verifying Super releases (0.1.0)</h1>
<h2 id="checksums">Checksums</h2>
<p>Release bundles ship <code>SHA256SUMS</code> beside each archive on GitHub Releases.</p>
<pre><code class="language-bash">curl -fsSLO &quot;https://github.com/theHamdiz/super-lang/releases/download/v0.1.0/SHA256SUMS&quot;
curl -fsSLO &quot;https://github.com/theHamdiz/super-lang/releases/download/v0.1.0/super-0.1.0-x86_64-unknown-linux-gnu.tar.gz&quot;
sha256sum -c SHA256SUMS</code></pre>
<p>The installer scripts (<code>install.sh</code>, <code>install.ps1</code>) perform the same check before extracting into <code>~/.super/</code>.</p>
<p>Each unpacked bundle also contains <code>build-info.txt</code> (commit SHA, <code>rustc -V</code>, UTC build stamp, and <code>SOURCE_DATE_EPOCH</code> when set).</p>
<h2 id="github-build-attestations-partial">GitHub build attestations (PARTIAL)</h2>
<p>Tag releases from workflow <code>release-bundle.yml</code> attach a <strong>SLSA-style build provenance</strong> attestation for the aggregated <code>SHA256SUMS</code> manifest (not per-matrix archive attestations yet).</p>
<pre><code class="language-bash">gh release download v0.1.0 -p SHA256SUMS
gh attestation verify SHA256SUMS --owner theHamdiz --repo super-lang</code></pre>
<p>Per-archive <code>gh attestation verify</code> will be documented here once each tarball is attested individually.</p>
<h2 id="supported-version">Supported version</h2>
<p>Security fixes are accepted for <strong>0.1.0</strong> only until a newer stable line is announced — see <a href="../SECURITY.md">SECURITY.md</a>.</p>
<h2 id="reproducibility">Reproducibility</h2>
<p>Release packaging sets <code>SOURCE_DATE_EPOCH</code>, remaps absolute paths, sorts tar members, and uses <code>gzip -n</code>. Full recipe and per-target claim table: <a href="REPRODUCE_BUILDS.md"><code>REPRODUCE_BUILDS.md</code></a>.</p>
<pre><code class="language-bash">export SOURCE_DATE_EPOCH=&quot;$(git log -1 --format=%ct)&quot;
export SUPER_VERSION=0.1.0
export SUPER_TARGET=x86_64-unknown-linux-gnu
./scripts/package_release.sh
scripts/repro_check.sh --target x86_64-unknown-linux-gnu</code></pre>
<p><strong>Status (Round 7):</strong> both claimed Linux targets are byte-identical across two isolated trees:</p>
<div class="table-wrap"><table class="data"><thead><tr><th>Target</th><th>Dual-build SHA256</th></tr></thead><tbody><tr><td><code>x86_64-unknown-linux-gnu</code></td><td><code>86f5608570a5533241436ad898dd1c2892fe6ef7633a2b56413e563bd83e067c</code></td></tr><tr><td><code>x86_64-unknown-linux-musl</code></td><td><code>6dcdeb01febcc285b8ae7e91e34fcdfaf4c929b9e7f1555d644be1d9b79dc773</code></td></tr></tbody></table></div>
<p>Musl builds use pinned <code>rust:1-alpine@sha256:8d0e82ab…</code>. macOS / Windows / FreeBSD are <strong>not</strong> claimed as byte-reproducible.</p>
<h2 id="what-this-does-not-prove">What this does not prove</h2>
<p>Checksum verification does not audit compiler correctness, sandbox boundaries, or crypto implementations — see <a href="SAFETY_CLAIMS.md">SAFETY_CLAIMS.md</a>. Attestations prove which workflow produced <code>SHA256SUMS</code>, not that the Super language semantics are correct.</p><p><strong>Related:</strong> <a href="platform.html">Platform</a> · <a href="install-security.html">Install security</a></p>
