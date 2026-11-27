# Super · HTTP server

<h1>Build an HTTP server</h1>
<p>Route handlers have the static type <code>Fn[http.Request, http.Response]</code>. A route with the wrong arity or return type is rejected before the server starts.</p>
<pre class="sr-block"><code class="language-sr">use std.net.http

app = #routes {
  get "/health" {
    http.ok({status: "ok"})
  }
}

@app.serve("127.0.0.1:8080")</code></pre>
<p><code>#routes { ... }</code> builds an <code>App</code> through typed calls and rejects duplicate method/path pairs. The clean route methods are <code>get</code>, <code>post</code>, <code>put</code>, <code>patch</code>, and <code>delete</code>.</p>
<p><code>App.serve(address)</code> is the normal serving path. It installs graceful signal handling and uses the default server limits. Use <code>serve_with(address, ServeConfig)</code> to override limits. Applications that need direct lifecycle control may use <code>http.Server</code> and <code>sys.signal.Notify</code>.</p>
<p>This path is not allocation-free yet. Measured on the C23 AOT build, the hello server makes 256 heap allocations per request and frees 7 of them, so memory grows with the number of requests served. A test pins these counts. Request-scoped reclamation is not implemented; see the 0.1.0 changelog.</p>
<p><strong>Windows C23 AOT sockets: Winsock2 (WSAPoll) in 0.1.0.</strong> Treat production HTTP on Windows as Rust-hosted VM only until native Windows AOT socket CI is green.</p>
<p><strong>Next:</strong> <a href="testing.html">Test the API</a> · <strong>Related:</strong> <a href="api-style.html">API style</a> · <a href="platform.html">Platform support</a></p>
