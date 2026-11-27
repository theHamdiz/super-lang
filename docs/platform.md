# Super · Platform support

<h1>Platform support</h1>
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
<p>Generated C23 avoids Linux-only headers on other targets. <strong>Windows C23 AOT sockets: Winsock2 (WSAPoll) in 0.1.0.</strong> C23 AOT binaries link Winsock2 (<code>-lws2_32</code>) and use <code>WSAPoll</code> for readiness. <strong>Unsupported on Windows AOT in 0.1.0:</strong> TLS, QUIC, Linux <code>epoll</code>/<code>io_uring</code>, and IOCP-style evented I/O (Windows readiness is <code>WSAPoll</code> only; TCP proven in CI, not TLS/QUIC/IOCP).</p>
<p><strong>Release archive reproducibility:</strong> only <code>x86_64-unknown-linux-gnu</code> and <code>x86_64-unknown-linux-musl</code> are claimed byte-identical. <strong>macOS, FreeBSD, and Windows archives are not reproducible</strong> — see <a href="REPRODUCE_BUILDS.md"><code>docs/REPRODUCE_BUILDS.md</code></a>.</p>
<p>WASI uses <code>super build --target wasm32-wasi</code> and is not an installer bundle.</p>
<p><strong>Next:</strong> <a href="install.html">Install</a> · <a href="cfg.html">Conditional compilation</a> · <a href="spec.html">Spec</a></p>
