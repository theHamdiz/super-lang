# Super · Quickstart

<h1 id="super-010-in-60-minutes">Super 0.1.0 in 60 Minutes</h1>
<blockquote><p>Rapid field guide. Full book: <a href="https://github.com/theHamdiz/super-lang/blob/main/BOOK.md"><code>BOOK.md</code></a> / <a href="https://thehamdiz.github.io/super-lang/downloads/SUPER_BOOK.pdf"><code>SUPER_BOOK.pdf</code></a>. Normative: <a href="https://github.com/theHamdiz/super-lang/blob/main/SPECIFICATION.md"><code>SPECIFICATION.md</code></a>.</p></blockquote>
<p>Install (toolchain → <code>~/.super</code>, not <code>~/.cargo</code>):</p>
<pre><code class="language-bash">sh -c &#x27;f=$(mktemp) &amp;&amp; trap &quot;rm -f -- \&quot;$f\&quot;&quot; 0 &amp;&amp; curl --proto =https -fL https://thehamdiz.github.io/super-lang/install.sh -o &quot;$f&quot; &amp;&amp; sh &quot;$f&quot; &quot;$@&quot;&#x27; sh
# or from a checkout: ./scripts/install.sh</code></pre>
<p>This guide teaches Super 0.1.0 as shipped: scoped data stacks, <code>@</code> mutation, VM + SIMD JIT, C23 AOT, traits (<code>E0203</code>), and the example suite (<code>examples/01</code>–<code>examples/30</code>).</p>
<pre><code class="language-bash">super check examples/01_space_shooter/src/main.sr
super test examples/
super run examples/03_log_analyzer/src/main.sr
super build --release --c23 examples/08_kv_cache_engine/src/main.sr</code></pre>
<h1 id="focus--conciseness-sugar-parallelism-comptime">Focus — Conciseness, Sugar, Parallelism, Comptime</h1>
<p>Super’s density is intentional. The print edition (<code>SUPER_BOOK.pdf</code>) is a token-compact language for people and coding agents: scoped stacks, memory-safe by construction, C and Rust interop, and native code aimed past idiomatic C. Four ideas organize the book, and the bullets under them are the rest of the surface.</p>
<ol><li><strong>Conciseness</strong> — no <code>fn</code>/<code>let</code>/<code>var</code>/semicolon tax; <code>+</code> exports; inference; <code>Iterable</code> pipeline reductions; postfix <code>if</code>.</li><li><strong>Syntactic sugar with teeth</strong> — <code>@</code> means an in-place, <code>restrict</code>-style write; <code>[T; N]</code> means stack/SoA storage; <code>=&gt;</code> marks pure leaves; <code>T?</code>/<code>T!</code> force handling.</li><li><strong>Parallelism</strong> — <code>par</code> / <code>ParIterable.par_sum()</code> for independent work; <code>for @</code> updates elements; structured <code>spawn</code>/<code>scope</code> without a shared heap.</li><li><strong>Comptime</strong> — <code>#NAME</code>, <code>#for</code> tables, <code>#inline</code> / <code>#unroll</code> specialize into C23 <code>constexpr</code> / <code>.rodata</code> with zero startup cost.</li></ol>
<p>The forms around <code>for</code>, <code>for @</code>, and <code>par</code>. The rest of the surface is the same length:</p>
<ul><li><code>for</code> walks a range, an array, or a pipeline.</li><li><code>for @xs</code> writes each element in place.</li><li><code>#for</code> repeats the loop at compile time.</li><li><code>par</code> states that the iterations are independent.</li><li><code>Iterable.sum()</code> and <code>ParIterable.par_sum()</code> (from <code>std.core.traits</code>) are the reductions.</li><li><code>?</code> is an optional and <code>!</code> is a value error.</li><li><code>or</code> recovers from either in line.</li><li><code>ensure</code> leaves through its <code>else</code> block.</li><li><code>when</code> matches a value against arms.</li><li><code>+</code> exports the next declaration. There is no export list.</li><li><code>value.method(args)</code> is <code>method(value, args)</code>.</li><li>Pipelines read left to right, and the current value is <code>it</code>.</li><li><code>.{}</code> builds a value when the type is already known.</li><li>Structs line up with C. Sums stay a tag plus a payload.</li><li><code>+Name = trait { … }</code> is a method contract. <code>+Name: Trait = { … }</code> declares it on the type and writes the methods there. <code>Display</code>, <code>Eq</code>, <code>Ord</code>, <code>Add</code>, and the other language traits are ambient: declaring one replaces the builtin witness for <code>&quot;{…}&quot;</code>, <code>==</code>, and <code>+</code>.</li><li><code>test</code> blocks see private items and are stripped from a release build.</li><li><code>#</code> names, <code>#unroll</code>, and <code>#assert_size</code> run before the program starts.</li><li><code>scope</code>, <code>spawn</code>, and <code>await</code> structure concurrent work on the same stacks.</li><li><code>&quot;{expr}&quot;</code> interpolates through <code>Display</code>. Arrays, maps, structs, and commands render by that witness. A type that declares <code>Display</code> or <code>ToString</code> replaces the builtin. Raw strings do not interpolate, and <code>\{</code> still forces a literal brace.</li><li>Command-line parsing, a terminal UI, and a desktop UI sit on these same stacks.</li></ul>
<pre class="sr-block"><code class="language-sr">+N = 1024
+square(x: i64) =&gt; x * x

+main() {
    xs = [0; N]
    par i in 0..N { xs[i] = square(i) }
    print(&quot;CHECKSUM={xs.sum()}&quot;)
}</code></pre>
<p>The chapters below expand these themes with full examples, benchmarks, and the thirteen sample projects.</p>
<h1 id="part-i--foundations--philosophy">Part I — Foundations &amp; Philosophy</h1>
<h2 id="1-why-super-exists">1. Why Super exists</h2>
<p>Super is a token-compact language for coding agents and for the people who review what those agents write. Ceremony is what was removed: no <code>fn</code>, no <code>let</code>, no semicolons, and a type annotation only when inference would guess wrong. An agent spends its tokens on the program.</p>
<p>Memory is a scoped data stack, in the same family as Goose. A block, a task, a request, or a frame owns what it creates. A bump cursor allocates, and one rewind releases the whole region. The ordinary path has no tracing collector and no free-list walk.</p>
<p>That stack is memory-safe by construction. The checks happen before the program runs:</p>
<ol><li>Values are local and immutable until the program opts in. There is no hidden mutable alias.</li><li><code>@</code> is the only in-place write, and it is visible on the parameter and at the call.</li><li>A mutable view cannot escape the scope that owns the bytes (<code>E0401</code>), so a dangling pointer is a compile error.</li><li>Running out of stack is a checked failure. It does not overwrite the neighboring region.</li><li>Structs are C layouts with no object header, arrays are contiguous, and sums are a tag plus an inline payload.</li></ol>
<p>C and Rust meet that layout. Structs are C-ABI values. The AOT path emits C23, and a Rust host function registers in one line, including a shared slice. The VM, the native JIT, and the C binary agree on results. On dense kernels the native paths are built to meet and pass idiomatic C; the benchmark checksums are the comparison. The same minimal syntax is for command-line tools, backends and APIs, games, automation, and the programs in between.</p>
<p>Strings, arrays, maps, channels, and host handles still need storage. “Zero-heap” means the ordinary path uses scoped stacks, static storage, caller destinations, and explicit host resources.</p>
<h2 id="2-the-execution-stack">2. The execution stack</h2>
<p>A Super source file passes through a zero-copy-oriented lexer, an arena-shaped AST, semantic analysis, and lowering. It can then run in three related ways:</p>
<ul><li>The VM executes compact register bytecode and is the universal fallback.</li><li>The native JIT recognizes useful kernels and emits executable x86-64 or AArch64 code.</li><li>The C23 backend emits self-contained C and invokes the host compiler for optimized AOT binaries.</li></ul>
<p>C23 is the default AOT standard. Generated code can use <code>constexpr</code>, <code>&lt;stdbit.h&gt;</code>, <code>[[unsequenced]]</code>, <code>restrict</code>, and recognized OpenMP parallel loops. This is not a “C syntax mode” exposed to Super programmers. It is a portable optimization and interoperability substrate.</p>
<p>The JIT does not attempt to be a second giant optimizing compiler. It has a deliberately focused model: find canonical dense loops with safe alias facts, emit packed instructions, handle the scalar tail, and fall back when a shape is not recognized. The C compiler remains the broad optimizer for release AOT.</p>
<h2 id="3-design-commitments">3. Design commitments</h2>
<p>Super keeps a small set of rules that show up everywhere:</p>
<ul><li><strong>Scoped data stacks</strong> own temporary aggregates; leaving a scope rewinds storage in O(1).</li><li><strong><code>@</code> marks in-place mutation</strong> at the call site and on mutable parameters; the compiler derives non-aliasing from that contract.</li><li><strong>Flat C-ABI structs and compact sums</strong> keep layouts deterministic for AOT and FFI.</li><li><strong><code>#</code> is compile time</strong> — typed bindings, loops, quotes, and attributes, not token pasting.</li><li><strong>Routes, tests, UFCS pipelines, and structured concurrency</strong> (<code>scope</code> / <code>spawn</code> / <code>chan</code> / <code>shared</code>) are part of the language, not bolted on later.</li></ul>
<p>C23 is the default AOT substrate (the host compiler sees <code>restrict</code>, OpenMP, and dense arrays). The VM and JIT are the other two execution paths. Performance claims should always be tied to a workload, machine, and checksum gate; the repository’s verified kernel suite is evidence for that run, not a universal law.</p>
<h2 id="4-your-first-module">4. Your first module</h2>
<p>Every <code>.sr</code> file is a module. There is no package declaration and no <code>fn</code> keyword.</p>
<pre class="sr-block"><code class="language-sr">/// Exported greeting.
+greet(name: str) =&gt; &quot;Hello, {name}!&quot;

private_suffix = &quot;from Super&quot;

+main() {
    message = greet(&quot;Amina&quot;)
    print(&quot;{message} {private_suffix}&quot;)
}

test &quot;greeting interpolates&quot; {
    assert_eq(greet(&quot;Amina&quot;), &quot;Hello, Amina!&quot;)
}</code></pre>
<p>The <code>+</code> exports a declaration. <code>private_suffix</code> is visible only inside the module and its tests. Run:</p>
<pre><code class="language-bash">super check hello.sr
super test hello.sr
super run hello.sr</code></pre>
<p>The toolchain’s stable diagnostics are part of the language experience. Undefined names are E0101, mismatched types E0201, missing mutation borrows E0301, and escaping mutable roots E0401. Read the code, not only the prose: the highlighted span normally identifies the shortest correction.</p>
<h1 id="part-ii--complete-language-syntax">Part II — Complete Language Syntax</h1>
<h2 id="5-bindings-literals-and-operators">5. Bindings, literals, and operators</h2>
<p>Bindings infer their type:</p>
<pre class="sr-block"><code class="language-sr">host = &quot;127.0.0.1&quot;
port: u16 = 8080
+MAX_CLIENTS = 65_536
flags = 0xff_00 | 0b1010
ratio = 1.5e2</code></pre>
<p>The <code>+</code> on <code>MAX_CLIENTS</code> exports it; it is not unary plus. Ordinary statements need no semicolon. Decimal, hexadecimal, binary, and octal integers accept <code>_</code>. Strings support interpolation:</p>
<pre class="sr-block"><code class="language-sr">print(&quot;listening on {host}:{port}, flags={flags}&quot;)</code></pre>
<p>Escapes are <code>\n \r \t \0 \\ \&quot; \&#x27; \{ \}</code>, <code>\xNN</code> (ASCII) and <code>\u{1F600}</code>; anything else is a syntax error. For backslash-heavy text use a raw string, <code>r&quot;C:\Users\me&quot;</code> or <code>r#&quot;say &quot;hi&quot; \d+&quot;#</code>, which has no escapes and no interpolation. <code>print</code> and <code>println</code> both end the line (<code>println</code> is the explicit name; <code>print()</code> / <code>println()</code> alone emit a blank line).</p>
<p>Arithmetic, comparison, bitwise, shift, logical, and assignment operators follow familiar precedence. <code>&amp;&amp;</code> and <code>||</code> short-circuit. <code>..</code> creates a half-open iteration range in canonical loops.</p>
<p>Use postfix guards when they make an early exit clearer:</p>
<pre class="sr-block"><code class="language-sr">+clamp_nonnegative(x: i64) : i64 {
    return 0 if x &lt; 0
    x
}</code></pre>
<h2 id="6-arrays-maps-records-and-slices">6. Arrays, maps, records, and slices</h2>
<p>Arrays are contiguous and zero-indexed:</p>
<pre class="sr-block"><code class="language-sr">primes = [2, 3, 5, 7]
pixels = [0; 1920 * 1080]
middle = primes[1..3]</code></pre>
<p><code>[value; N]</code> is the dense stack-array repeat form. The compiler knows the element kind and length and can remove checks in canonical loops.</p>
<p>Brace literals intentionally cover two data shapes:</p>
<pre class="sr-block"><code class="language-sr">ports = {http: 80, https: 443}
headers = {&quot;content-type&quot;: &quot;application/json&quot;, [custom_name]: &quot;yes&quot;}
config = {host, port, timeout: 30}</code></pre>
<p>Quoted or computed keys force a map. Identifier fields create an anonymous record and support punning. Named and inferred struct literals use:</p>
<pre class="sr-block"><code class="language-sr">user = User{id: 7, name: &quot;Amina&quot;}

+make_user(id: i64, name: str) : User  =&gt; .{id, name, balance: 0.0}</code></pre>
<p>The second form works because the return type supplies the expected <code>User</code>.</p>
<h2 id="7-structs-methods-and-sum-types">7. Structs, methods, and sum types</h2>
<pre class="sr-block"><code class="language-sr">+Account = {
    id: i64
    balance: f64 = 0.0
    -token: str = &quot;&quot;

    deposit(@self, amount: f64) : f64 {
        self.balance += amount
        self.balance
    }
}

Payment = {
    Pending
    Settled(amount: f64)
    Rejected(reason: str)

    amount_or(self, fallback: f64) : f64 {
        when self {
            Payment.Settled { amount } =&gt; amount
            else =&gt; fallback
        }
    }
}</code></pre>
<p>Struct fields have deterministic offsets and flat layout. In a public struct, <code>-token</code> remains private. Tests in the defining module can inspect private fields. Sums may declare methods the same way; <code>when</code> arms can bind payloads with <code>Payment.Settled { amount }</code>.</p>
<p>Methods are ordinary statically resolved functions owned by a type. UFCS lets free functions read like methods too:</p>
<pre class="sr-block"><code class="language-sr">balance = @account.deposit(20.0).round(2)</code></pre>
<p>UFCS works on any receiver (numbers, strings, arrays, structs) and chains left to right. The chain and the nested call are the same program:</p>
<pre class="sr-block"><code class="language-sr">print(&quot;{49.0.sqrt().floor().to_int()}&quot;)   // 7
print(&quot;{to_int(floor(sqrt(49.0)))}&quot;)      // 7, identical
n = &quot;Hi&quot;.byte_len()                         // 2</code></pre>
<p>The leading <code>@</code> is visible proof that the pipeline may mutate <code>account</code>. Missing it is E0301.</p>
<p>A trait is signatures only. The struct names it in the header and writes the bodies once:</p>
<pre class="sr-block"><code class="language-sr">+Mark = trait {
    paint(self) : str
}

+Pin: Mark = {
    label: str = &quot;&quot;
    paint(self) : str  =&gt; self.label
}</code></pre>
<p><code>std.gui.widgets</code> defines <code>Widget</code> (<code>paint</code>, <code>hit</code>, <code>measure</code>). <code>Toggle</code>, <code>Dialog</code>, and <code>Sprite</code> in <code>std.game.fx</code> declare it the same way. A missing method, a wrong <code>@</code>, a wrong parameter type, or a wrong return type is E0203. Calls stay static.</p>
<p>Sum values use a tag and inline payload. They make state transitions explicit without allocating a class hierarchy:</p>
<pre class="sr-block"><code class="language-sr">status = Payment.Settled(49.95)
when status {
    Payment.Pending =&gt; print(&quot;waiting&quot;)
    Payment.Settled =&gt; print(&quot;paid&quot;)
    else =&gt; print(&quot;failed&quot;)
}</code></pre>
<h2 id="8-functions-inference-and-monomorphization">8. Functions, inference, and monomorphization</h2>
<p>Functions can be expressions or blocks:</p>
<pre class="sr-block"><code class="language-sr">+square(x: f64) =&gt; x * x
+cube(x: f64) =&gt; x * x * x

+distance(x1: f64, y1: f64, x2: f64, y2: f64) : f64 {
    dx = x2 - x1
    dy = y2 - y1
    (dx * dx + dy * dy).sqrt()
}</code></pre>
<p><code>=&gt;</code> carries an inline preference. Untyped or <code>_</code> parameters are specialized at call sites:</p>
<pre class="sr-block"><code class="language-sr">+identity(value) =&gt; value</code></pre>
<p>Calling <code>identity(3)</code> and <code>identity(&quot;x&quot;)</code> produces concrete specializations rather than requiring a runtime boxed union. Compile-time parameters start with <code>#</code> and also participate in specialization.</p>
<p>Call punning reduces boilerplate when scope names match parameter names:</p>
<pre class="sr-block"><code class="language-sr">+spawn_wave(world, count, speed) { /* ... */ }
world = make_world()
count = 20
speed = 1.5
spawn_wave(.)</code></pre>
<p>Use it only when the names communicate the call clearly.</p>
<h2 id="9-mutation-and-scoped-lifetime">9. Mutation and scoped lifetime</h2>
<p>Super makes mutation a protocol:</p>
<pre class="sr-block"><code class="language-sr">+Account = {
    balance: f64

    discount(@self, percent: f64) : f64 {
        self.balance -= self.balance * percent / 100.0
        self.balance
    }
}

acct = Account{balance: 200.0}
remaining = @acct.discount(10.0)</code></pre>
<p>The method says this value receives exclusive mutable access. The call repeats the fact on the receiver. During that dynamic extent, the region cannot be mutably aliased. This supports both human review and <code>restrict</code>/SIMD lowering.</p>
<p>Most temporary aggregate data lives on a scoped data stack:</p>
<pre class="sr-block"><code class="language-sr">scope {
    scratch = [0.0; 4096]
    for @scratch { it = index * 0.5 }
    consume(scratch)
} // one rewind releases scratch and other temporaries</code></pre>
<p>Do not return a mutable borrow into <code>scratch</code> or persist it in <code>state</code>; the compiler rejects the escape. Destination-driven construction can place a returned aggregate directly in caller-owned storage, avoiding an intermediate copy.</p>
<h2 id="10-loops-implicit-names-and-reductions">10. Loops, implicit names, and reductions</h2>
<p>The shortest loop uses implicit <code>it</code> and <code>index</code>:</p>
<pre class="sr-block"><code class="language-sr">for scores { print(&quot;{index}: {it}&quot;) }
for @scores { it += 5 }
for 0..100 { consume(it) }</code></pre>
<p>Named binders improve nested loops:</p>
<pre class="sr-block"><code class="language-sr">for row in 0..height {
    for col in 0..width {
        pixels[row * width + col] = shade(row, col)
    }
}</code></pre>
<p><code>while</code>, <code>loop</code>, <code>break</code>, and <code>continue</code> cover stateful control:</p>
<pre class="sr-block"><code class="language-sr">i = 0
while i &lt; n {
    i += 1
    continue if disabled[i]
    break if ready(i)
}</code></pre>
<p>Reductions make intent available to the optimizer:</p>
<pre class="sr-block"><code class="language-sr">total = values.sum()
energy = sum v * v for v in velocities
positive = values.keep(it &gt; 0).sum()
checksum = (0..n).map(data[it]).par_sum()</code></pre>
<p>The destination form is useful when a name is consumed later:</p>
<pre class="sr-block"><code class="language-sr">checksum = rows.keep(it.valid).sum()</code></pre>
<p>The compiler can lower recognized parallel reductions to worker-local accumulators and OpenMP on the C23 path.</p>
<h2 id="11-parallel-loops-and-structured-concurrency">11. Parallel loops and structured concurrency</h2>
<p>Use <code>par</code> for independent data parallelism:</p>
<pre class="sr-block"><code class="language-sr">par y in 0..height {
    rows[y] = render_row(y, width)
}</code></pre>
<p>Use <code>scope</code> and <code>spawn</code> for task parallelism:</p>
<pre class="sr-block"><code class="language-sr">scope {
    left = spawn fetch(&quot;/left&quot;)
    right = spawn fetch(&quot;/right&quot;)
    combine(await left, await right)
}</code></pre>
<p>All spawned work completes before scope exit. A bounded channel transfers ownership:</p>
<pre class="sr-block"><code class="language-sr">events = chan[Event](256)
scope {
    producer = spawn produce(events)
    consumer = spawn consume(events)
    await producer
    events.close()
    await consumer
}</code></pre>
<p><code>shared 0</code> creates supported atomic/futex-backed shared state. Prefer channel ownership transfer; use shared cells for genuinely shared counters or coordination. Two sibling spawns may not capture the same <code>@</code> root; E0302 prevents a race from becoming an optimizer alias violation.</p>
<p><code>when</code> can multiplex events and time:</p>
<pre class="sr-block"><code class="language-sr">when {
    events.recv() =&gt; handle(it)
    after 16.ms =&gt; update()
}</code></pre>
<h2 id="12-conditionals-when-and-ensure">12. Conditionals, <code>when</code>, and <code>ensure</code></h2>
<p><code>if</code> is an expression:</p>
<pre class="sr-block"><code class="language-sr">label = if score &gt; 100 { &quot;high&quot; } else { &quot;normal&quot; }
compact = active ? &quot;on&quot; : &quot;off&quot;</code></pre>
<p><code>when</code> selects the first matching arm and exposes the subject as <code>it</code>:</p>
<pre class="sr-block"><code class="language-sr">when latency_us {
    it &lt; 100 =&gt; metrics.fast()
    it &lt; 1000 =&gt; metrics.normal()
    else =&gt; metrics.slow()
}</code></pre>
<p><code>ensure</code> keeps the successful path unindented:</p>
<pre class="sr-block"><code class="language-sr">+process(user: User) {
    ensure user.active else {
        log.warn(&quot;inactive {user.id}&quot;)
        return
    }
    charge(user)
}</code></pre>
<h2 id="13-optionals-and-value-errors">13. Optionals and value errors</h2>
<p><code>T?</code> represents <code>T</code> or <code>none</code>; <code>T!</code> represents <code>T</code> or a value error.</p>
<pre class="sr-block"><code class="language-sr">+find_port(name: str) : i64?  =&gt; if name == &quot;http&quot; { 80 } else { none }

+read_port(path: str) : i64! {
    ensure path != &quot;&quot; else { fail &quot;empty path&quot; }
    8080
}

port = find_port(&quot;custom&quot;) or 3000
checked = read_port(&quot;&quot;) or err {
    print(&quot;configuration failed: {err}&quot;)
    return 3000
}</code></pre>
<p>An optional/error may not silently enter a context requiring its success type. E0202 points to the missing handling edge.</p>
<h2 id="14-modules-pods-tests-routes-and-state">14. Modules, Pods, tests, routes, and state</h2>
<p>Modules are files:</p>
<pre class="sr-block"><code class="language-sr">use models
use models.{User, Task}
use std.net.http
use std.fmt.json</code></pre>
<p>A <strong>Pod</strong> is a Super project. <code>Pod.toml</code> (removed legacy <code>Super.toml</code> still works) defines identity, <code>kind</code> (<code>bin</code> / <code>lib</code> / <code>macro</code>), path/Git dependencies, and build profiles. <code>super new --bin</code>, <code>--lib</code>, or <code>--macro</code> scaffolds the matching entry (<code>src/main.sr</code>, <code>src/lib.sr</code>, or <code>src/macros.sr</code>). <code>super build</code>, <code>super run</code>, and <code>super check</code> find that file from the working directory, fetch what it lists, and use <code>[build] profile</code> when no <code>-d</code>, <code>-r</code>, or <code>--profile</code> was passed. <code>super test</code> uses <code>[profile.test]</code>. Set <code>[build] target</code> / <code>targets = […]</code> or pass <code>--target &lt;TRIPLE&gt;</code> for cross and WebAssembly builds (<code>super targets</code> lists the catalog; wasm artifacts are <code>.wasm</code> under <code>target/&lt;triple&gt;/&lt;profile&gt;/</code>). <code>Pod.lock</code> / <code>Pod.lock</code> records resolved Git commits. Commit the lockfile for applications. <code>super vendor</code> produces a source snapshot under <code>vendor/</code>. Macro Pods distribute comptime with <code>+#NAME</code> exports and <code>#</code>-parameter specialization functions.</p>
<p>Tests are native grammar:</p>
<pre class="sr-block"><code class="language-sr">test &quot;private token is initialized&quot; {
    a = Account{id: 1}
    assert_eq(a.token, &quot;&quot;)
    #assert(a.id &gt; 0)
}</code></pre>
<p><code>test</code> is a keyword. <code>#assert</code> is a macro from <code>std.core.macros</code>, in scope everywhere: keywords are the only bare syntax, every user-extensible compile-time construct is a <code>#</code> macro, and everything else is a function or a method. Write your own with <code>#name { (pattern) =&gt; { template } }</code> (see <code>examples/doc_macros/</code>).</p>
<p>Routes capture method, path, request name, return type, and body:</p>
<pre class="sr-block"><code class="language-sr">#get(&quot;/health&quot;)
health(req: http.Request) : http.Response! {
    http.ok(.{ok: true})
}</code></pre>
<p><code>state</code> declares persistent module-root mutable state:</p>
<pre class="sr-block"><code class="language-sr">state world = ecs.World{}</code></pre>
<p>The hot-reload runtime may preserve its data stack while replacing code. Root lifetime checks prevent transient mutable borrows from being stored into it.</p>
<h2 id="15-compile-time-super">15. Compile-time Super</h2>
<p>Compile-time bindings look like ordinary names with a visible <code>#</code>. Use <code>+#</code> to export them from a macro Pod:</p>
<pre class="sr-block"><code class="language-sr">#WIDTH = 64
+#MASK = 0xff
pixels = [0; #WIDTH * #WIDTH]</code></pre>
<p>Compile-time control can generate tables:</p>
<pre class="sr-block"><code class="language-sr">#HIST_BOUNDS = #for i in 0..6 { i * 200 }

#if #WIDTH &gt;= 64 {
    #simd
    for @pixels { it = index &amp; #MASK }
}</code></pre>
<p>Named comptime functions and quote/splice work with typed code. Scaffold a distributable macro Pod with <code>super new --macro tables</code>:</p>
<pre class="sr-block"><code class="language-sr">+#make_add(#n: i64, x: i64) : i64 = x + #n
+#ANSWER = &#x27;{ 6 * 7 }</code></pre>
<p>Attributes communicate constrained optimizer/domain facts:</p>
<pre class="sr-block"><code class="language-sr">#inline
+mix(a: i64, b: i64) =&gt; (a ^ b) * 0x9e37

#unroll(4)
for 0..16 { accum += lanes[it] }</code></pre>
<p>Use <code>embed &quot;assets/logo.bin&quot;</code> for compile-time file bytes. The dependency is tracked by the meta cache; C23 output can use constant byte tables or <code>#embed</code>.</p>
<h2 id="16-standard-library-map">16. Standard library map</h2>
<p>The shipped library is <strong>100</strong> <code>.sr</code> files under <code>stdlib/</code> (see <code>stdlib/mod.sr</code>). Vendor SaaS clients live in first-party <code>pods/</code> lib Pods.</p>
<ul><li><code>std.core.str</code>, <code>std.core.bytes</code>, <code>std.core.collections</code>, <code>std.core.regex</code>, <code>std.core.uuid</code>, <code>std.core.unicode</code>, <code>std.core.decimal</code>, <code>std.core.bitset</code></li><li><code>std.fmt.json</code>, <code>std.fmt.csv</code>, <code>std.fmt.toml</code>, <code>std.fmt.yaml</code>, <code>std.fmt.msgpack</code>, <code>std.fmt.xml</code>, <code>std.fmt.bin</code>, <code>std.fmt.compress</code>, <code>std.fmt.mime</code></li><li><code>std.sys.fs</code>, <code>std.sys.path</code>, <code>std.sys.env</code>, <code>std.sys.proc</code>, <code>std.sys.time</code>, <code>std.sys.epoll</code>, <code>std.sys.io_uring</code>, <code>std.sys.ffi</code>, <code>std.sys.mmap</code>; CLI apps use <code>std.cli.cmd</code> only (not <code>std.sys.cli</code>)</li><li><code>std.io.poll</code>, <code>std.io.buf</code>, <code>std.io.fs</code>, <code>std.io.stdio</code>, <code>std.io.console</code>, <code>std.io.socket</code>, <code>std.io.archive</code></li><li><code>std.net.http</code>, <code>std.net.ws</code>, <code>std.net.url</code>, <code>std.net.ip</code>, <code>std.net.tcp</code>, <code>std.net.udp</code>, <code>std.net.dns</code>, <code>std.net.sse</code>, <code>std.net.retry</code>, <code>std.net.tls</code>, <code>std.net.smtp</code>, <code>std.net.grpc</code>, <code>std.net.quic</code>, <code>std.net.oauth2</code></li><li><code>std.db.sql</code>, <code>std.db.postgres</code>, <code>std.db.mysql</code>, <code>std.db.redis</code>, <code>std.db.sqlite</code>, <code>std.db.migrate</code>, <code>std.db.clickhouse</code></li><li><code>std.sec.crypto</code>, <code>std.sec.encoding</code>, <code>std.sec.jwt</code>, <code>std.sec.sha2</code>, <code>std.sec.hash</code>, <code>std.sec.pq</code>, <code>std.sec.x509</code>, <code>std.sec.otp</code>, <code>std.sec.argon2</code>, <code>std.sec.vault</code></li><li><code>std.math.core</code>, <code>std.math.linalg</code>, <code>std.math.rand</code>, <code>std.math.noise</code>, <code>std.math.complex</code>, <code>std.math.quat</code>, <code>std.math.stats</code>, <code>std.math.geom</code>, <code>std.math.fft</code></li><li><code>std.game.ecs</code>, <code>std.game.physics2d</code>, <code>std.game.ai</code>, <code>std.game.app</code>, <code>std.game.assets</code>, <code>std.game.tilemap</code>, <code>std.game.fx</code>, <code>std.game.netcode</code>, <code>std.game.audio</code></li><li><code>std.ai.client</code>, <code>std.ai.tensor</code>, <code>std.ai.embed</code></li><li><code>std.diag.test</code>, <code>std.diag.log</code>, <code>std.diag.metrics</code>, <code>std.diag.bench</code></li><li><code>std.cli.cmd</code> (alias <code>std.clap</code>), <code>std.tui</code>, <code>std.gui</code>, <code>std.gui.widgets</code>, <code>std.gui.window</code>. <code>use std.gui as App</code> binds the module under <code>App</code>; <code>use std.gui.{Gui as Desk}</code> adds another name for that export. <code>Widget</code> is the inline contract for <code>paint</code>, <code>hit</code>, and <code>measure</code>.</li></ul>
<p>Vendor Pods: <code>openai</code>, <code>claude</code>, <code>gemini</code>, <code>openrouter</code>, <code>deepseek</code>, <code>elevenlabs</code>, <code>seedance</code>, <code>supabase</code>, <code>neon</code>, <code>cockroachdb</code>, <code>mariadb</code>, <code>meilisearch</code>.</p>
<p>On Linux, epoll/io_uring-backed non-blocking primitives support the network modules via <code>std.io.poll</code>. JSON supports validation/encoding and zero-tree path extraction. ECS and linear algebra expose layouts suitable for dense kernels.</p>
<h1 id="part-iii--design-patterns">Part III — Design Patterns</h1>
<p>The syntax in Part II is small; the engineering consequences are not. This part collects the arrangements that repeatedly produce clear, fast Super programs. The patterns are deliberately concrete. They describe ownership, layout, failure, and compilation boundaries rather than abstract class hierarchies.</p>
<h2 id="17-structure-of-arrays-and-restrict">17. Structure of arrays and <code>@restrict</code></h2>
<p>An array of records is often convenient at an API boundary:</p>
<pre class="sr-block"><code class="language-sr">Body = {
    x: f64
    y: f64
    vx: f64
    vy: f64
}</code></pre>
<p>A structure of arrays (SoA) is usually better for a hot integration pass. Values used together occupy contiguous streams, a field can be traversed without loading unrelated bytes, and mutable roots are visible in the signature:</p>
<pre class="sr-block"><code class="language-sr">+integrate(@x, @y, @vx, @vy, dt: f64) {
    #simd
    for i in 0..x.len() {
        x[i] += vx[i] * dt
        y[i] += vy[i] * dt
    }
}

integrate(@x, @y, @vx, @vy, 1.0 / 60.0)</code></pre>
<p>The four <code>@</code> parameters are exclusive, non-escaping borrows. When semantic analysis proves that they do not overlap, C23 AOT can emit <code>restrict</code> pointers and the JIT can select packed operations. <code>@</code> is therefore more than a visual marker: it is a checked optimizer fact.</p>
<p>Use SoA when a system updates one or two fields over many entities. Prefer a record when a value crosses an FFI boundary, is handled as a unit, or is rarely scanned. A practical game world often uses both: stable component arrays in storage and small records at commands, events, and serialization boundaries.</p>
<p>Do not manufacture alias claims. Passing the same array as two mutable parameters would invalidate the optimization contract and is rejected. If two logical views overlap, redesign the kernel around one root, split into disjoint slices, or stage output in a separate destination.</p>
<h3 id="double-buffering">Double buffering</h3>
<p>Neighborhood algorithms often read old state and write new state. Separate input and output buffers remove loop-carried aliasing:</p>
<pre class="sr-block"><code class="language-sr">+steer(px, py, vx, vy, @next_vx, @next_vy) {
    par i in 0..px.len() {
        force = compute_force(i, px, py, vx, vy)
        next_vx[i] = vx[i] + force.x
        next_vy[i] = vy[i] + force.y
    }
}</code></pre>
<p>At module scope you can declare several bindings on one line with per-name export: <code>+tasks, +next_id, name = [], 1, &quot;Ahmad&quot;</code>. Inside functions, <code>a, b = 1, 2</code> introduces locals and <code>a, b = b, a</code> swaps safely because every right-hand side is evaluated before any store.</p>
<p>Swap the roots after the pass. Multi-assignment evaluates right sides before stores, so <code>vx, next_vx = next_vx, vx</code> has swap semantics.</p>
<h2 id="18-destination-driven-construction-with">18. Destination-driven construction with <code>.{...}</code></h2>
<p>Super constructs an aggregate into the destination selected by context whenever possible. A return type, parameter type, field type, or typed binding can provide that context:</p>
<pre class="sr-block"><code class="language-sr">+User = {
    id: i64
    name: str
    active: bool = true
}

+new_user(id: i64, name: str) : User  =&gt; .{id, name}

+accept(user: User) =&gt; user.id

primary: User = .{id: 1, name: &quot;Amina&quot;}
code = accept(.{id: 2, name: &quot;Noor&quot;})</code></pre>
<p>Field punning binds <code>id</code> to <code>id: id</code>. Defaults fill omitted fields. This is not dynamic dictionary construction: semantic analysis knows the target layout and field offsets. For larger results the hidden return destination lets the callee write directly into caller-owned stack storage.</p>
<p>Use named <code>User{...}</code> when context is weak or the type name communicates domain meaning. Use <code>.{...}</code> at typed boundaries where repeating the name adds no information. If inference reports E0201, add a type annotation at the narrowest boundary rather than annotating every intermediate expression.</p>
<p>Destination construction also clarifies optional fallback:</p>
<pre class="sr-block"><code class="language-sr">user = find_user(id) or .{id: 0, name: &quot;Guest&quot;, active: false}</code></pre>
<p>The successful and fallback branches construct the same concrete type. No boxed union or hash map is introduced.</p>
<h2 id="19-request-frame-lexical-and-task-stacks">19. Request, frame, lexical, and task stacks</h2>
<p>Most backend values die with a request; most game temporaries die with a frame. Super maps those facts to bump-stack marks:</p>
<pre class="sr-block"><code class="language-sr">+handle(req: http.Request) : http.Response! {
    scope {
        body = req.json(CreateOrder) or err {
            return http.bad_request(.{error: &quot;{err}&quot;})
        }
        scratch = [0; 4096]
        order = create_order(body, @scratch)
        return http.ok(.{id: order.id})
    }
}</code></pre>
<p>The request or explicit scope records a cursor. Arrays, temporary strings, decoded records, and intermediate pipeline output advance the cursor. Scope exit rewinds it once. There is no object-by-object free walk and no tracing pause.</p>
<p>That is the design. In 0.1.0 the HTTP request scope is not implemented yet. A server whose routes are closures runs in the boxed runtime, which keeps every per-request value on the heap: 42 allocations and 0 frees per request for the hello server, pinned by <code>crates/super-cli/tests/alloc_probe.rs</code>.</p>
<p>The same shape applies to a frame:</p>
<pre class="sr-block"><code class="language-sr">state world = ecs.World{}

+update(dt: f64) {
    scope {
        contacts = [Contact{}; MAX_CONTACTS]
        count = broad_phase(world, @contacts)
        solve(@world, contacts[0..count], dt)
    }
}</code></pre>
<p><code>world</code> persists across frames and may survive hot reload. <code>contacts</code> does not. The compiler prevents a mutable view into <code>contacts</code> from being stored in <code>world</code>. Persistent state should contain values, stable handles, or data copied into its own stack, never an accidental pointer into request/frame scratch.</p>
<p>A spawned task receives a task-local micro-stack. Structured <code>scope</code> waits for its tasks before reclaiming their storage:</p>
<pre class="sr-block"><code class="language-sr">scope {
    left = spawn fetch(&quot;/left&quot;)
    right = spawn fetch(&quot;/right&quot;)
    merged = merge(await left, await right)
    consume(merged)
}</code></pre>
<p>If data must outlive the task, return it into the awaiting destination, send an owned value through a bounded channel, or explicitly copy it into persistent state. Do not hide lifetime transfer behind a global registry.</p>
<h3 id="choosing-the-right-lifetime">Choosing the right lifetime</h3>
<p>Use a lexical stack for parsing helpers and algorithm scratch. Use a request stack for decoded inputs and response assembly. Use a frame stack for contacts, render commands, and visibility lists. Use <code>state</code> for world data, caches, and hot-reload continuity. Use host resource handles for sockets, files, GPU objects, and database connections whose cleanup cannot be represented by a cursor rewind alone.</p>
<h2 id="20-error-pipelines-with-or-and-err">20. Error pipelines with <code>or</code> and <code>err</code></h2>
<p>Optionals and value errors keep the success path as an ordinary value:</p>
<pre class="sr-block"><code class="language-sr">+load_config(path: str) : Config! {
    text = fs_read_str(path) or err {
        fail &quot;cannot read {path}: {err}&quot;
    }
    toml_decode(text, Config) or err {
        fail &quot;invalid configuration: {err}&quot;
    }
}</code></pre>
<p>At a boundary, convert domain failure into the boundary protocol:</p>
<pre class="sr-block"><code class="language-sr">#post(&quot;/orders&quot;)
handle_orders(req: http.Request) : http.Response! {
    body = req.json(CreateOrder) or err {
        return http.bad_request(.{error: &quot;{err}&quot;})
    }
    order = create_order(body) or err {
        return http.conflict(.{error: &quot;{err}&quot;})
    }
    http.ok(.{order_id: order.id})
}</code></pre>
<p><code>or</code> evaluates its fallback only on failure. <code>or err { ... }</code> binds the payload for logging, mapping, recovery, or early return. A plain fallback is ideal for a true default:</p>
<pre class="sr-block"><code class="language-sr">port = env_port(&quot;PORT&quot;) or 8080</code></pre>
<p>Do not use defaults to erase operational failures. Missing optional metadata can reasonably default; invalid authentication, disk corruption, and database timeouts normally deserve an error path. Keep logs at architectural boundaries so low-level helpers remain reusable and a failure is not logged repeatedly.</p>
<p>For pipelines, handle failure as soon as policy is known:</p>
<pre class="sr-block"><code class="language-sr">token = header(req, &quot;authorization&quot;)
    .strip_prefix(&quot;Bearer &quot;)
    .decode_token()
    or err { return http.unauthorized(.{error: &quot;{err}&quot;}) }</code></pre>
<p>The exact available methods depend on imported modules, but the shape remains: typed transformations on the left, one policy decision at the edge.</p>
<h2 id="21-ecs-systems-and-deterministic-frames">21. ECS systems and deterministic frames</h2>
<p>An ECS world separates identity from component storage. A system should declare the components it reads and mutates:</p>
<pre class="sr-block"><code class="language-sr">state world = ecs.World{}

+movement(dt: f64) {
    #simd
    for @(pos: Pos), (vel: Vel) in world {
        pos.x += vel.vx * dt
        pos.y += vel.vy * dt
    }
}</code></pre>
<p>The leading <code>@</code> applies to the mutable component stream. The velocity stream is read-only. This gives the scheduler and optimizer a usable access set. Systems that mutate disjoint component sets can run in parallel; systems with conflicts need an order or a staged command buffer.</p>
<p>For deterministic simulation:</p>
<ol><li>use a fixed timestep;</li><li>make iteration order stable where order affects results;</li><li>avoid wall-clock reads inside systems;</li><li>seed random generators explicitly;</li><li>queue structural changes until a defined synchronization point;</li><li>checksum authoritative fields in tests and network snapshots.</li></ol>
<pre class="sr-block"><code class="language-sr">+tick(@world, inputs) {
    apply_inputs(@world, inputs)
    movement(TICK_DT)
    broad_phase(@world)
    solve_contacts(@world)
    flush_commands(@world)
}</code></pre>
<p>Use <code>par</code> only where iterations are independent or a supported reduction combines worker-local results. Collision pair generation, damage ordering, and entity creation can be order-sensitive; make their merge rule explicit.</p>
<h2 id="22-router-and-protocol-patterns">22. Router and protocol patterns</h2>
<p>Routes are declarations in the language:</p>
<pre class="sr-block"><code class="language-sr">#get(&quot;/health&quot;)
health(req: http.Request) : http.Response! {
    http.ok(.{status: &quot;ok&quot;})
}

#post(&quot;/users&quot;)
create_user(req: http.Request) : http.Response! {
    input = req.json(CreateUser) or err {
        return http.bad_request(.{error: &quot;{err}&quot;})
    }
    http.created(create_user(input))
}</code></pre>
<p>Keep transport records separate from domain records when validation or privacy differs. A request DTO can accept fields that never enter the stored model; a response DTO can omit private fields. Route handlers should coordinate parsing, authorization, domain calls, and response mapping, not implement storage algorithms inline.</p>
<p>For high-rate services, parse method and path once, use zero-copy slices for parameters, and route through a precomputed tree or table. Bound request bodies, header counts, connection queues, channel capacity, and timeouts. A scoped request stack makes reclamation cheap but does not make unbounded input safe.</p>
<p>For frontend/backend communication, prefer a binary WebSocket protocol with Protobuf schemas for sustained streams, then gRPC with a Tonic host where HTTP/2 RPC fits. Always retain a JSON endpoint or JSON WebSocket message form for browsers, diagnostics, and operational fallback. The core domain function should accept typed values so binary and JSON adapters share behavior.</p>
<pre class="sr-block"><code class="language-sr">+apply_command(@state, command: Command) : Reply! {
    // transport-independent domain logic
}

#post(&quot;/v1/command&quot;)
command(req: http.Request) : http.Response! {
    command = req.json(Command) or err {
        return http.bad_request(.{error: &quot;{err}&quot;})
    }
    http.ok(apply_command(@state, command) or err {
        return http.conflict(.{error: &quot;{err}&quot;})
    })
}</code></pre>
<h2 id="23-compile-time-lookup-tables-with-for">23. Compile-time lookup tables with <code>#for</code></h2>
<p>A lookup table that depends only on constants belongs in compile-time storage:</p>
<pre class="sr-block"><code class="language-sr">+CRC_POLY = 0xEDB88320

+CRC32_LUT = #for i in 0..256 {
    crc = i
    for 0..8 {
        crc = if (crc &amp; 1) == 1 {
            (crc &gt;&gt; 1) ^ CRC_POLY
        } else {
            crc &gt;&gt; 1
        }
    }
    crc &amp; 0xFFFF_FFFF
}</code></pre>
<p>Runtime code becomes one indexed update per byte:</p>
<pre class="sr-block"><code class="language-sr">+crc32(text: str) : i64 {
    crc = 0xFFFF_FFFF
    for i in 0..byte_len(text) {
        slot = (crc ^ ord(text[i])) &amp; 255
        crc = ((crc &gt;&gt; 8) ^ CRC32_LUT[slot]) &amp; 0xFFFF_FFFF
    }
    (crc ^ 0xFFFF_FFFF) &amp; 0xFFFF_FFFF
}</code></pre>
<p>The table is generated by the meta engine, validated during compilation, and emitted into constant storage. There is no startup initializer. The same pattern works for trigonometric samples, parser character classes, quantization curves, small finite-state transitions, and protocol metadata.</p>
<p>Keep generated tables modest and deterministic. If generation depends on a file, use <code>embed</code> so the compiler tracks the input. If the table is huge or deployment-specific, an external generated asset may produce faster builds and smaller debug artifacts.</p>
<h2 id="24-fused-pipelines-and-explicit-reductions">24. Fused pipelines and explicit reductions</h2>
<p>UFCS pipelines describe value flow:</p>
<pre class="sr-block"><code class="language-sr">total = readings
    .keep(it.valid)
    .map(it.value * it.value)
    .sum()</code></pre>
<p>The optimizer may fuse recognized operations into one traversal. Use pipelines for transformations whose ordering and ownership are straightforward. Use an explicit loop when you need multiple outputs, early exits, complex mutation, or detailed diagnostics:</p>
<pre class="sr-block"><code class="language-sr">sum = 0.0
rejected = 0
for reading in readings {
    if reading.valid {
        sum += reading.value * reading.value
    } else {
        rejected += 1
    }
}</code></pre>
<p><code>.sum()</code> and <code>.par_sum()</code> communicate an additive reduction directly:</p>
<pre class="sr-block"><code class="language-sr">energy = velocities.map(it * it).sum()
checksum = (0..n).map(hash(values[it])).par_sum()</code></pre>
<p>A parallel reduction requires an associative operation in practice. Floating addition is not mathematically associative, so a parallel grouping can differ in low bits from a serial grouping. Use fixed grouping, integer/fixed-point accumulation, or an accepted tolerance when reproducibility matters.</p>
<h2 id="25-pattern-review-checklist">25. Pattern review checklist</h2>
<p>Before optimizing a subsystem, answer:</p>
<ul><li>What owns the data: lexical scope, request, frame, task, persistent state, or host resource?</li><li>Is the hot layout AoS or SoA, and does it match actual access?</li><li>Which roots mutate, and are all mutation boundaries marked with <code>@</code>?</li><li>Can output be built directly in its final destination?</li><li>Are failure values handled where policy becomes known?</li><li>Does parallel work write disjoint destinations or use a supported reduction?</li><li>Can a compile-time table replace repeated pure runtime setup?</li><li>Is a JSON fallback preserved beside any binary transport?</li><li>Do tests verify failure paths, deterministic checksums, and optimized/scalar equivalence?</li></ul>
<h1 id="part-iv--algorithms-ten-benchmark-deep-dives">Part IV — Algorithms: Ten Benchmark Deep Dives</h1>
<p>The benchmark contract matters: every language implementation must compute the same checksum, the harness performs two warmups and five timed runs, and a checksum mismatch fails the gate. Read the Super source at <code>benchmarks/*/algo.sr</code>; do not infer performance from synthetic snippets alone.</p>
<h2 id="17-01_nbody">17. 01_nbody</h2>
<p>Source: <code>benchmarks/01_nbody/algo.sr</code></p>
<p>N-body integrates interacting particles and stresses floating-point arithmetic, nested loops, and mutable arrays. The Super version keeps body state in dense arrays and uses explicit <code>@</code> at mutation boundaries. The useful lesson is layout: positions, velocities, and masses should be arranged for repeated linear traversal rather than wrapped in dynamic objects.</p>
<p>When modifying the kernel:</p>
<ol><li>preserve update order and checksum;</li><li>keep hot values local;</li><li>avoid changing force accumulation precision;</li><li>compare both AOT and JIT, since specialization shapes differ.</li></ol>
<p>the latest verified results mean times were C 4.852 ms, Super AOT 4.703 ms, and Super JIT 5.460 ms.</p>
<h2 id="18-02_mandelbrot_parallel">18. 02_mandelbrot_parallel</h2>
<p>Source: <code>benchmarks/02_mandelbrot_parallel/algo.sr</code></p>
<p>Mandelbrot is a naturally independent row kernel. <code>par y in 0..H</code> exposes that independence, while every worker computes a local row result. The C23 emitter recognizes the pure-double row shape and can generate OpenMP with a reduction or independent stores.</p>
<p>The deep lesson is to parallelize at a granularity that amortizes scheduling. Pixels are too small as tasks; rows are useful partitions. the latest verified results mean times were C 3.237 ms, AOT 1.423 ms, and JIT 3.652 ms, with substantial variance visible in short runs.</p>
<h2 id="19-03_astar_pathfinding">19. 03_astar_pathfinding</h2>
<p>Source: <code>benchmarks/03_astar_pathfinding/algo.sr</code></p>
<p>A* combines dense fixed-capacity arrays, a binary heap, irregular branches, and sentinel values. <code>[1; 4096]</code>, <code>[INF; 4096]</code>, and related repeat arrays avoid per-node object allocation. This benchmark demonstrates that scoped dense storage is useful even when SIMD is not the main win.</p>
<p>Keep heap invariants explicit and use multi-assignment carefully: right sides are all evaluated before stores, making swaps reliable. the latest verified results means were C 0.663 ms, AOT 0.624 ms, and JIT 1.173 ms.</p>
<h2 id="20-04_json_parity">20. 04_json_parity</h2>
<p>Source: <code>benchmarks/04_json_serde/algo.sr</code></p>
<p>The JSON benchmark measures parsing/encoding work under a checksum gate. Super’s standard JSON design favors structural validation and path extraction rather than building a general heap tree. This is representative of backend APIs that decode known request shapes.</p>
<p>Avoid comparing only source line count: validation coverage and escaping rules must match. the latest verified results means were C 0.380 ms, AOT 0.346 ms, and JIT 0.723 ms.</p>
<h2 id="21-05_parallel_mapreduce">21. 05_parallel_mapreduce</h2>
<p>Source: <code>benchmarks/05_parallel_mapreduce/algo.sr</code></p>
<p>This workload generates a 64 KiB corpus, hashes words and prefixes, and accumulates into fixed buckets. It exercises radix constants, array-repeat allocation, loops, mutation-capable helper parameters, and reduction-like scans.</p>
<p>The current benchmark uses explicit open addressing so all implementations perform comparable work. It is a good study in keeping the hot table contiguous and avoiding general map overhead. the latest verified results means were C 2.509 ms, AOT 0.638 ms, and JIT 1.121 ms.</p>
<h2 id="22-06_mergesort_radix">22. 06_mergesort_radix</h2>
<p>Source: <code>benchmarks/06_mergesort_radix/algo.sr</code></p>
<p>Sorting exposes memory traffic, branch behavior, and temporary buffers. Dense arrays and scoped scratch storage fit the algorithm’s phase lifetime. Radix work rewards predictable passes; merge work rewards sequential accesses.</p>
<p>Preserve stability and checksum when experimenting with pass width. the latest verified results means were C 5.834 ms, AOT 5.431 ms, and JIT 6.342 ms.</p>
<h2 id="23-07_crc32_lz4_compress">23. 07_crc32_lz4_compress</h2>
<p>Source: <code>benchmarks/07_crc32_lz4_compress/algo.sr</code></p>
<p>Compression combines byte-oriented scanning, hash/checksum arithmetic, and irregular match decisions. It tests whether host/string/byte operations remain cheap after lowering. The standard library’s compression domain includes LZ4 block compression, Adler-32, and CRC-32 contracts.</p>
<p>This benchmark is sensitive to exact byte semantics. Check checksum agreement before trusting time. the latest verified results means were C 0.924 ms, AOT 1.097 ms, and JIT 1.550 ms.</p>
<h2 id="24-08_ecs_simulation">24. 08_ecs_simulation</h2>
<p>Source: <code>benchmarks/08_ecs_simulation/algo.sr</code></p>
<p>The ECS workload uses 2,000 entities for 20 ticks, structure-of-arrays position/velocity/health data, a 64×64 spatial grid, and collision probes. <code>integrate(@px, @py, @vx, @vy)</code> is the canonical restrict-friendly kernel. Grid rebuild and collision remain branchier.</p>
<p>This split is instructive: vectorize integration, keep spatial indexing explicit, and do not demand that one optimization fit the whole frame. the latest verified results means were C 3.800 ms, AOT 3.922 ms, and JIT 4.095 ms.</p>
<h2 id="25-09_matmul_tiled">25. 09_matmul_tiled</h2>
<p>Source: <code>benchmarks/09_matmul_tiled/algo.sr</code></p>
<p>Tiled matrix multiplication tests cache locality and repeated floating multiply-add work. Tile size is a compile-time tuning decision; arrays remain dense and indexing arithmetic should stay simple enough for strength reduction and bounds-check elimination.</p>
<p>Use <code>#unroll</code> or <code>#simd</code> only after preserving the scalar checksum. the latest verified results means were C 1.336 ms, AOT 1.288 ms, and JIT 1.846 ms.</p>
<h2 id="26-10_sha256_merkle_tree">26. 10_sha256_merkle_tree</h2>
<p>Source: <code>benchmarks/10_sha256_merkle_tree/algo.sr</code></p>
<p>SHA-256 plus Merkle construction stresses shifts, rotates, masks, fixed rounds, and repeated block hashing. Hex constants and explicit shift operators map cleanly through C23 <code>&lt;stdbit.h&gt;</code>-capable lowering. The Merkle phase tests compact intermediate arrays.</p>
<p>Cryptographic benchmark code must prioritize exactness: do not substitute non-equivalent “fast hashes.” the latest verified results means were C 0.999 ms, AOT 0.976 ms, and JIT 1.456 ms.</p>
<h2 id="27-source-atlas-the-ten-kernels">27. Source atlas: the ten kernels</h2>
<p>The following excerpts are from the shipped <code>algo.sr</code> files. They are short on purpose: use them to locate the hot shape, then read the complete source for initialization, checksum, and edge conditions.</p>
<h3 id="271-n-body-nested-force-accumulation">27.1 N-body: nested force accumulation</h3>
<p>From <code>benchmarks/01_nbody/algo.sr</code>:</p>
<pre class="sr-block"><code class="language-sr">for i in 0..n {
    ax, ay = 0.0, 0.0
    for j in 0..n {
        if i != j {
            dx = x[j] - x[i]
            dy = y[j] - y[i]
            inv = 1.0 / sqrt(dx * dx + dy * dy + 0.01)
            inv3 = inv * inv * inv
            ax += m[j] * dx * inv3
            ay += m[j] * dy * inv3
        }
    }
    vx[i] += ax * dt
    vy[i] += ay * dt
}</code></pre>
<p>The source uses separate <code>x</code>, <code>y</code>, <code>vx</code>, <code>vy</code>, and <code>m</code> arrays. Force accumulation stays local until the velocity store. The later position pass is a simple dense traversal. This split makes update order explicit and avoids allocating body objects.</p>
<h3 id="272-mandelbrot-row-level-par">27.2 Mandelbrot: row-level <code>par</code></h3>
<p>From <code>benchmarks/02_mandelbrot_parallel/algo.sr</code>:</p>
<pre class="sr-block"><code class="language-sr">rows = [0; 256]
par y in 0..H {
    cy, local = (y * 1.0 / H) * 2.0 - 1.0, 0
    for x in 0..W {
        cx, zx, zy, iter = (x * 1.0 / W) * 3.5 - 2.5, 0.0, 0.0, 0
        for 0..MAXIT {
            zx2, zy2 = zx * zx, zy * zy
            break if zx2 + zy2 &gt; 4.0
            zy, zx = 2.0 * zx * zy + cy, zx2 - zy2 + cx
            iter += 1
        }
        local += iter
    }
    rows[y] = local
}</code></pre>
<p>Rows are independent destinations, so no lock is needed. Multi-assignment preserves the old <code>zx</code> and <code>zy</code> values while calculating the next iteration. Only the final row sum crosses worker boundaries.</p>
<h3 id="273-a-reusable-dense-state-and-inline-heap">27.3 A*: reusable dense state and inline heap</h3>
<p>From <code>benchmarks/03_astar_pathfinding/algo.sr</code>:</p>
<pre class="sr-block"><code class="language-sr">#inline
+astar(@gscore, @came, @heap, @hprio, @hn, walkable, start: i64, goal: i64): i64 {
    for 0..CELLS {
        gscore[it] = INF
        came[it] = -1
    }
    hn[0] = 0
    gscore[start] = 0
    heap_push(@heap, @hprio, @hn, start, heur(start, goal))
    while hn[0] &gt; 0 {
        cur = heap_pop(@heap, @hprio, @hn)
        // neighbor relaxation follows
    }
    -1
}</code></pre>
<p>The caller allocates all buffers once and reuses them for 40 queries. A single-element <code>hn</code> array is an explicit mutable scalar root. Compile-time direction tables remove per-query setup.</p>
<h3 id="274-json-parity-deterministic-field-workload">27.4 JSON parity: deterministic field workload</h3>
<p>From <code>benchmarks/04_json_serde/algo.sr</code>:</p>
<pre class="sr-block"><code class="language-sr">for 0..N {
    rng = (rng * 1103515245 + 12345) % 2147483648
    id = rng % 1000000
    rng = (rng * 1103515245 + 12345) % 2147483648
    total_cents = rng % 100000
    rng = (rng * 1103515245 + 12345) % 2147483648
    cs = cs + 2 * (id + total_cents + (rng % 8 + 1) * 17)
}</code></pre>
<p>The current parity kernel is intentionally compact and deterministic. It exercises the same generated field values and checksum as peer implementations; it should not be described as a complete general JSON parser benchmark. Use the <code>std.fmt.json</code> tests for parser correctness and a separately specified corpus when measuring full encode/decode behavior.</p>
<h3 id="275-mapreduce-open-addressed-fixed-buckets">27.5 Map/reduce: open-addressed fixed buckets</h3>
<p>From <code>benchmarks/05_parallel_mapreduce/algo.sr</code>:</p>
<pre class="sr-block"><code class="language-sr">+bump(@keys, @counts, key: i64, add: i64) : i64 {
    i = key % BUCKETS
    for probe in 0..BUCKETS {
        j = (i + probe) % BUCKETS
        if counts[j] == 0 {
            keys = arr_set(keys, j, key)
            counts = arr_set(counts, j, add)
            return add
        }
        if keys[j] == key {
            n = counts[j] + add
            counts = arr_set(counts, j, n)
            return n
        }
    }
    0
}</code></pre>
<p>The table has explicit capacity and linear probing. It avoids a generic map so peer implementations perform comparable hashing and collision work. Despite the directory name, the current source's counting pass is explicit and primarily serial; benchmark descriptions should not claim parallel execution that is not present in the file.</p>
<h3 id="276-mergesort-one-scoped-scratch-buffer">27.6 Mergesort: one scoped scratch buffer</h3>
<p>From <code>benchmarks/06_mergesort_radix/algo.sr</code>:</p>
<pre class="sr-block"><code class="language-sr">+mergesort(a, tmp, lo: i64, hi: i64) {
    if hi - lo &lt;= 1 { return a }
    mid = lo + (hi - lo) / 2
    a = mergesort(a, tmp, lo, mid)
    a = mergesort(a, tmp, mid, hi)
    merge(a, tmp, lo, mid, hi)
}</code></pre>
<p><code>main</code> allocates <code>a</code> and <code>tmp</code> once at length 50,000. Recursive calls pass the same storage rather than allocating subarrays. The checksum also verifies sorted order, preventing a fast but invalid no-op.</p>
<h3 id="277-crc-32-library-table-path">27.7 CRC-32: library table path</h3>
<p>From <code>benchmarks/07_crc32_lz4_compress/algo.sr</code>:</p>
<pre class="sr-block"><code class="language-sr">use std.fmt.compress

payload = block
for 0..8 { payload = payload + block }
cs = crc32_str(payload) + crc32_str(block) + acc % 1000</code></pre>
<p>The benchmark builds deterministic bytes, then calls the standard compression module's CRC path. The security hash module demonstrates the modern table pattern with <code>CRC32_LUT = #for i in 0..256 { ... }</code>. Always verify exact unsigned 32-bit semantics when changing this kernel.</p>
<h3 id="278-ecs-soa-and-exclusive-integration-roots">27.8 ECS: SoA and exclusive integration roots</h3>
<p>From <code>benchmarks/08_ecs_simulation/algo.sr</code>:</p>
<pre class="sr-block"><code class="language-sr">+integrate(@px, @py, @vx, @vy) {
    for 0..N {
        npx, npy = px[it] + vx[it], py[it] + vy[it]
        nvx, nvy = vx[it], vy[it]
        nvx = -nvx if npx &lt; 0.0 || npx &gt; 600.0
        nvy = -nvy if npy &lt; 0.0 || npy &gt; 600.0
        px[it], py[it], vx[it], vy[it] = npx, npy, nvx, nvy
    }
}</code></pre>
<p>The signature communicates four non-overlapping mutable streams. Grid rebuild uses <code>heads</code> and <code>next</code>; collision probes read the grid and update health. Those phases have different access patterns and remain separate.</p>
<h3 id="279-matrix-multiplication-loop-order-and-dense-buffers">27.9 Matrix multiplication: loop order and dense buffers</h3>
<p>From <code>benchmarks/09_matmul_tiled/algo.sr</code>:</p>
<pre class="sr-block"><code class="language-sr">+matmul(@a, @b, @c, n: i64) {
    for i in 0..n {
        for k in 0..n {
            aik = a[i * n + k]
            for j in 0..n {
                c[i * n + j] += aik * b[k * n + j]
            }
        }
    }
}</code></pre>
<p>The <code>i-k-j</code> order reuses <code>aik</code> and traverses contiguous rows of <code>b</code> and <code>c</code>. The AOT backend may recognize and tile this named kernel. Three explicit mutable roots expose alias facts; the checksum is the scaled Frobenius norm.</p>
<h3 id="2710-sha-256-merkle-tree-compact-level-construction">27.10 SHA-256 Merkle tree: compact level construction</h3>
<p>From <code>benchmarks/10_sha256_merkle_tree/algo.sr</code>:</p>
<pre class="sr-block"><code class="language-sr">+hash_pair(a, b) : _ {
    cat = [0; 64]
    for 0..32 {
        cat[it], cat[it + 32] = a[it], b[it]
    }
    sha2_256(cat)
}</code></pre>
<p>Leaves are 64-byte deterministic messages. Parent input is a fixed 64-byte stack array, and the level shrinks until one root remains. An odd final node is paired with itself. The final byte fold checks the actual digest path.</p>
<h1 id="part-v--standard-library">Part V — Standard Library</h1>
<p>The standard library is organized by responsibility rather than by a single prelude. Import the modules a source file uses. Tooling reads <code>stdlib/mod.sr</code>, whose exported <code>STDLIB_VERSION</code> is <code>&quot;0.1.0&quot;</code> and whose manifest lists 57 modules across nine layers.</p>
<pre class="sr-block"><code class="language-sr">use std.core.str
use std.fmt.json
use std.net.http
use std.diag.metrics</code></pre>
<p>Short aliases such as <code>std.str</code>, <code>std.json</code>, <code>std.http</code>, <code>std.ecs</code>, and <code>std.diag.metrics</code> are supported where documented. Layered names are preferable in libraries because they expose the dependency boundary.</p>
<h2 id="28-core-values-bytes-and-collections">28. <code>core</code>: values, bytes, and collections</h2>
<p>The core layer contains:</p>
<ul><li><code>core.str</code>: slicing, searching, case conversion, padding, splitting, and numeric parsing;</li><li><code>core.bytes</code>: byte arrays, hexadecimal conversion, endian reads/writes, and bit helpers;</li><li><code>core.collections</code>: stack, queue, ring buffer, set, priority queue, and array-backed map helpers;</li><li><code>core.regex</code>: compiled backtracking regular expressions;</li><li><code>core.uuid</code>: UUID v4/v7 generation, parsing, and formatting.</li></ul>
<p>Core operations preserve explicit ownership. Searches that can miss return optionals. Parsing does not invent sentinel values unless the API explicitly documents one:</p>
<pre class="sr-block"><code class="language-sr">use std.core.str

port = to_i64(&quot;8080&quot;) or err {
    print(&quot;invalid port: {err}&quot;)
    return
}
parts = split(&quot;api/v1/users&quot;, &quot;/&quot;)</code></pre>
<p>Use slices for views into an existing string or byte buffer. Copy into an owning destination before the backing request/frame stack is released. Collection types are value-oriented and expose <code>@</code> mutation:</p>
<pre class="sr-block"><code class="language-sr">use std.core.collections

queue = RingBuf.new(128)
@queue.push(event)
next = @queue.pop() or none</code></pre>
<p>Fixed arrays remain the lowest-overhead choice when capacity is naturally bounded and known at compile time.</p>
<h2 id="29-fmt-json-text-formats-binary-data-compression">29. <code>fmt</code>: JSON, text formats, binary data, compression</h2>
<p>The format layer contains JSON, CSV, TOML, binary packing, and compression:</p>
<ul><li><code>fmt.json</code> validates, decodes, encodes, and extracts known paths without requiring a general heap tree;</li><li><code>fmt.csv</code> handles RFC 4180 quoting, commas, and line endings;</li><li><code>fmt.toml</code> parses the supported TOML subset used by package/configuration workflows;</li><li><code>fmt.bin</code> packs and unpacks explicit binary representations;</li><li><code>fmt.compress</code> supplies LZ4 blocks, Adler-32, and CRC-32.</li></ul>
<p>At an HTTP boundary:</p>
<pre class="sr-block"><code class="language-sr">#post(&quot;/events&quot;)
events(req: http.Request) : http.Response! {
    event = req.json(Event) or err {
        return http.bad_request(.{error: &quot;{err}&quot;})
    }
    http.ok(.{accepted: true, id: event.id})
}</code></pre>
<p>JSON is the interoperability fallback even when a service also exposes Protobuf over binary WebSocket or gRPC through a host. Binary codecs must define endianness, field widths, length limits, and versioning. Never decode a claimed length without comparing it to the remaining buffer and configured maximum.</p>
<p>Compression works best on owned byte blocks. Bound decompressed size before allocation. Checksums detect accidental corruption; they are not authentication. Use <code>sec</code> primitives for signatures or message authentication.</p>
<h2 id="30-sys-operating-system-boundaries">30. <code>sys</code>: operating-system boundaries</h2>
<p>The system layer includes path, CLI, environment, filesystem, process, time, epoll, and io_uring modules:</p>
<ul><li><code>sys.path</code>, <code>sys.cli</code>, <code>sys.env</code>, <code>sys.fs</code>, <code>sys.proc</code>, <code>sys.time</code>;</li><li><code>sys.epoll</code> for Linux non-blocking descriptor readiness;</li><li><code>sys.io_uring</code> for Linux submission/completion integration, including the shipped NOP and socket-readiness bridge.</li></ul>
<p>System APIs wrap narrow host primitives. File descriptors, processes, sockets, and mappings are resources, not ordinary scoped bytes. Their wrappers track ownership and explicit close behavior.</p>
<p>An epoll-style server separates readiness from protocol work:</p>
<pre class="sr-block"><code class="language-sr">poll = epoll.open() or err { fail &quot;epoll: {err}&quot; }
@poll.add(listener, EPOLL_IN) or err { fail &quot;register: {err}&quot; }

loop {
    ready = @poll.wait(256, 50.ms) or err {
        log.error(&quot;poll: {err}&quot;)
        continue
    }
    for event in ready {
        handle_ready(event)
    }
}</code></pre>
<p>The exact names in a module should be checked with completion or source docs; the architecture is stable: readiness returns bounded events, protocol state lives with the connection, and request scratch dies after dispatch.</p>
<p>io_uring is not automatically faster for every workload. Submission batching, registered resources, kernel version, and operation mix matter. Keep an epoll path where portability or simpler readiness semantics are valuable.</p>
<h2 id="31-net-http-websocket-polling-tcp-and-udp">31. <code>net</code>: HTTP, WebSocket, polling, TCP, and UDP</h2>
<p>The network layer contains:</p>
<ul><li><code>net.http</code>: requests, responses, routers, cookies, and middleware;</li><li><code>net.ws</code>: handshake and frame encoding/decoding;</li><li><code>net.poll</code>: a higher-level TCP readiness event loop;</li><li><code>net.tcp</code>, <code>net.udp</code>, <code>net.dns</code>, <code>net.ip</code>, <code>net.url</code>;</li><li><code>net.udp_netcode</code>: packet helpers for game networking;</li><li><code>net.sse</code>: server-sent event framing;</li><li><code>net.retry</code>: exponential backoff and circuit breaking.</li></ul>
<p>HTTP helpers return typed responses:</p>
<pre class="sr-block"><code class="language-sr">#get(&quot;/health&quot;)
health(req: http.Request) : http.Response! {
    http.ok(.{version: &quot;0.1.0&quot;, healthy: true})
}</code></pre>
<p>WebSocket framing distinguishes text, binary, ping/pong, and close:</p>
<pre class="sr-block"><code class="language-sr">wire = ws_encode(ws_binary(snapshot), 0) or err {
    fail &quot;snapshot encode: {err}&quot;
}
frame = ws_decode(wire) or ws_close(1002, &quot;invalid frame&quot;)</code></pre>
<p>For live game or telemetry streams, use binary WebSocket plus Protobuf as the primary contract and offer JSON text messages as fallback. gRPC with a Tonic host is a strong service-to-service alternative. MsgPack is useful when schema tooling is less important, but version fields and limits remain mandatory.</p>
<p>TCP is a byte stream: applications must frame messages. A length prefix should use a fixed endian representation, reject oversized values, and cope with partial reads/writes. UDP preserves datagrams but may lose, duplicate, or reorder them; the netcode layer's sequence/ack design must match the game.</p>
<h2 id="32-game-ecs-physics-app-loops-assets-and-ai">32. <code>game</code>: ECS, physics, app loops, assets, and AI</h2>
<p>The game layer supplies:</p>
<ul><li><code>game.ecs</code>: sparse-set entity/component storage;</li><li><code>game.physics2d</code>: circles, boxes, broad-phase helpers, and impulses;</li><li><code>game.ai</code>: A*, steering, finite-state machines, utility scores, and behavior trees;</li><li><code>game.app</code>: fixed timestep, scenes, input actions, and audio-facing helpers;</li><li><code>game.assets</code>: hot-reload cache plus PNG/WAV loaders;</li><li><code>game.tilemap</code>: CSV/JSON tilemaps integrated with pathfinding;</li><li><code>game.fx</code>: <code>Sprite</code> (declares <code>Widget</code>), <code>Clip</code>, <code>Cue</code>, and <code>Mixer</code>. Paint and sound are display-list lines.</li></ul>
<p>A fixed update loop should accumulate wall time but advance authoritative state in constant steps:</p>
<pre class="sr-block"><code class="language-sr">clock = [0.0]

+frame(frame_dt: f64) {
    clock[0] += frame_dt
    for 0..MAX_CATCHUP {
        break if clock[0] &lt; TICK_DT
        update(TICK_DT)
        clock[0] -= TICK_DT
    }
    render(clock[0] / TICK_DT)
}</code></pre>
<p>Separate broad phase, narrow phase, and solver passes. Broad phase should reduce candidate pairs; it must not change collision meaning. Use stable pair ordering when replay/network determinism matters. Assets persist beyond one frame, while decoded scratch and draw-command lists fit frame stacks.</p>
<h2 id="33-sec-hashes-authentication-encoding-and-post-quantum-tools">33. <code>sec</code>: hashes, authentication, encoding, and post-quantum tools</h2>
<p>The security layer distinguishes representation, checksums, hashes, and authentication:</p>
<ul><li><code>sec.encoding</code>: base64/base32/hex and URL encoding;</li><li><code>sec.crypto</code>: hashes, HMAC, PBKDF2, and TOTP;</li><li><code>sec.jwt</code>: HS256 JWT helpers;</li><li><code>sec.sha2</code>: pure-Super SHA-256 and HMAC-SHA256;</li><li><code>sec.hash</code>: BLAKE2/3, SHA-3, SHAKE, Keccak, CRC-32, FNV, and DJB2;</li><li><code>sec.pq</code>: ML-KEM, ML-DSA, hybrid helpers, and hash-based signatures.</li></ul>
<p>The CRC implementation demonstrates compile-time table generation:</p>
<pre class="sr-block"><code class="language-sr">+CRC32_LUT = #for i in 0..256 {
    crc = i
    for 0..8 {
        crc = if (crc &amp; 1) == 1 {
            (crc &gt;&gt; 1) ^ HASH_CRC_POLY
        } else {
            crc &gt;&gt; 1
        }
    }
    crc &amp; 0xFFFF_FFFF
}</code></pre>
<p>CRC-32 and FNV are not cryptographic. Use HMAC for message authentication. Validate JWT algorithm, expiry, issuer/audience policy, and key rotation outside the compact token primitive. Password derivation needs a deliberately expensive KDF and deployment-specific parameters.</p>
<p>Post-quantum primitives are easy to compose incorrectly. Treat the module as building blocks, preserve domain separation and transcript binding, and use reviewed protocols rather than inventing a handshake.</p>
<h2 id="34-math-scalar-through-geometric-tools">34. <code>math</code>: scalar through geometric tools</h2>
<p>The math layer contains scalar functions, linear algebra, random generation, noise, complex values, quaternions, statistics, and geometry:</p>
<pre class="sr-block"><code class="language-sr">use std.math.linalg
use std.math.rand
use std.math.geom

velocity = v2(3.0, 4.0)
speed = v2_len(velocity)</code></pre>
<p>Random generators are deterministic under an explicit seed, which is essential for tests and replay. Do not use simulation PRNGs for secrets. Linear algebra types favor flat layout and dense traversal. Quaternions avoid many Euler-angle composition problems but still require normalization policy. Statistics APIs should be fed bounded, validated samples; percentile methods differ in interpolation and must be chosen consistently.</p>
<h2 id="35-diag-tests-logs-and-metrics">35. <code>diag</code>: tests, logs, and metrics</h2>
<p>The diagnostic layer includes:</p>
<ul><li><code>diag.test</code>: assertions and soft reports;</li><li><code>diag.log</code>: leveled structured logs with redaction;</li><li><code>diag.metrics</code>: counters, gauges, histograms, and summary helpers.</li></ul>
<p>Tests are language items, not runtime callbacks:</p>
<pre class="sr-block"><code class="language-sr">test &quot;parser rejects an oversized frame&quot; {
    result = decode_frame(make_header(MAX_FRAME + 1))
    #assert(result.is_error())
}</code></pre>
<p>Logs should carry stable field names and avoid secrets. Metrics should have bounded label cardinality: status class and route template are useful; user IDs, raw paths, and request IDs are not. Histograms need bounds chosen for the service-level objective, preferably generated at compile time:</p>
<pre class="sr-block"><code class="language-sr">#LATENCY_MS = #for i in 0..8 { i * 25 }</code></pre>
<h2 id="36-standard-library-selection-guide">36. Standard-library selection guide</h2>
<p>Start at the lowest layer that expresses the problem:</p>
<ol><li>use <code>core</code> for in-memory values and containers;</li><li>use <code>fmt</code> to cross a representation boundary;</li><li>use <code>sys</code> for operating-system resources;</li><li>use <code>net</code>, <code>db</code>, <code>game</code>, or <code>sec</code> for domain protocols;</li><li>instrument behavior with <code>diag</code>.</li></ol>
<p>Do not bypass a typed module for a host primitive merely to save a line. The module supplies validation, ownership, diagnostics, and a place for optimized lowering. Reach for the host boundary only when implementing or extending the module itself.</p>
<h1 id="part-vi--real-world-projects">Part VI — Real-World Projects</h1>
<h2 id="28-01_space_shooter">28. 01_space_shooter</h2>
<p>Path: <code>examples/01_space_shooter/</code></p>
<p>This project introduces a frame loop, ECS-style position/velocity/collider/enemy data, wave spawning, collision math, score, health, and tests. Study it after the array and mutation chapters. The key mapping is frame lifetime to scoped-stack lifetime: temporary collision candidates die together while persistent world state remains under <code>state</code>.</p>
<p>For voxel meshing, 3D integer coordinates, ray visibility, and dense chunk-style indexing, continue with <code>examples/11_voxel_raycast/</code>.</p>
<h2 id="29-02_task_api">29. 02_task_api</h2>
<p>Path: <code>examples/02_task_api/</code></p>
<p>The API is split into <code>main.sr</code>, <code>models.sr</code>, <code>auth.sr</code>, and <code>routes.sr</code>. It demonstrates <code>use</code>, exported/private declarations, typed records, route declarations, JSON handling, authentication, and endpoint tests. This is the best starting point for a multi-module backend.</p>
<p>Production work should add a concrete persistent database adapter, migration policy, TLS termination contract, and operational limits. Keep JSON as a supported fallback even if a deployment adds a binary WebSocket/Protobuf or gRPC transport.</p>
<h2 id="30-03_log_analyzer">30. 03_log_analyzer</h2>
<p>Path: <code>examples/03_log_analyzer/</code></p>
<p>The analyzer demonstrates high-volume parsing, filtering, grouping, sums, and compact status representation. It maps stream/batch lifetime to a scoped stack and makes reductions explicit. It is a useful template for telemetry preprocessing and offline data tools.</p>
<p>Extension gap: add bounded streaming input, backpressure, rotated-file handling, and malformed-record metrics without turning every record into a heap object.</p>
<h2 id="31-04_roguelike_dungeon">31. 04_roguelike_dungeon</h2>
<p>Path: <code>examples/04_roguelike_dungeon/</code></p>
<p>This project applies grids, procedural generation, pathfinding, turns, and compact entities. It reinforces array-repeat maps, deterministic random input, and AI helpers. Tests should pin generation seeds and verify reachability rather than snapshots of incidental formatting.</p>
<p>Chunked 3D cells, face-style sampling, and deterministic grid indexing are demonstrated in <code>examples/11_voxel_raycast/</code>.</p>
<h2 id="32-05_tower_defense">32. 05_tower_defense</h2>
<p>Path: <code>examples/05_tower_defense/</code></p>
<p>Tower defense combines path following, waves, targeting, cooldowns, and damage. It is a natural data-oriented workload: separate arrays for positions, target IDs, cooldowns, and health make hot updates direct. <code>.sum()</code> pipelines are appropriate for economy and damage summaries.</p>
<p>Swarm movement with neighborhood queries, separation/alignment/cohesion, and double-buffered steering is demonstrated in <code>examples/12_boids_flocking/</code>.</p>
<h2 id="33-06_physics_sandbox">33. 06_physics_sandbox</h2>
<p>Path: <code>examples/06_physics_sandbox/</code></p>
<p>The sandbox exercises integration, collision detection, impulses, and the <code>std.physics2d</code> contract. Separate broad phase from narrow phase and solver iterations. <code>@</code> parameters make mutation sets reviewable and provide alias facts for integration kernels.</p>
<p>Extension gap: continuous collision detection, joints, sleeping islands, and deterministic cross-platform replay are not implied by a small 2D example.</p>
<h2 id="34-07_realtime_chat_broker">34. 07_realtime_chat_broker</h2>
<p>Path: <code>examples/07_realtime_chat_broker/</code></p>
<p>The broker maps naturally to structured concurrency: listeners create tasks, channels transfer messages, and scopes own connection work. The network library provides HTTP/WebSocket framing and Linux epoll-backed readiness primitives.</p>
<p>Chat broadcast is not game-state replication. Continue with the authoritative fixed-timestep loop in <code>examples/13_multiplayer_tick/</code> for snapshots, sequence numbers, and deterministic tick checksums. Prefer a binary WebSocket + Protobuf interface for live state, with JSON fallback for browsers/tools.</p>
<h2 id="35-08_kv_cache_engine">35. 08_kv_cache_engine</h2>
<p>Path: <code>examples/08_kv_cache_engine/</code></p>
<p>The cache engine demonstrates maps/tables, expiry, metrics, and bounded storage behavior. It is useful for thinking about persistent versus request-scoped state. Define eviction and time semantics explicitly; a benchmark that never reaches capacity does not test a cache policy.</p>
<p>Extension gap: persistence snapshots, replication, sharding, admission policy, and crash recovery need separate contracts and tests.</p>
<h2 id="36-09_http_load_tester">36. 09_http_load_tester</h2>
<p>Path: <code>examples/09_http_load_tester/</code></p>
<p>This example combines <code>scope</code>, <code>spawn</code>, <code>chan[Sample]</code>, <code>shared</code> counters, compile-time histogram bounds, HTTP simulation, and metrics. It is the clearest concurrency tutorial in the examples.</p>
<p>The collector owns the stream of samples; workers send values rather than sharing an output array. That ownership shape scales better and avoids E0302. A production extension needs real sockets, connection reuse, rate shaping, coordinated omission correction, TLS, and percentile calculation over an actual histogram.</p>
<h2 id="37-10_mapreduce_kv">37. 10_mapreduce_kv</h2>
<p>Path: <code>examples/10_mapreduce_kv/</code></p>
<p>The project connects partitioned mapping, grouped/reduced values, and key/value state. It complements benchmark 05 by emphasizing architecture rather than only a timed kernel. Use bounded channels between stages to make backpressure visible.</p>
<p>Extension gap — multiplayer/cluster: distributed operation needs partition ownership, retry identity, idempotence, failure detection, and a wire protocol. Binary Protobuf over WebSocket or gRPC/Tonic hosts are suitable primary transports; preserve JSON for debugging and frontend compatibility.</p>
<h2 id="38-11_voxel_raycast">38. 11_voxel_raycast</h2>
<p>Path: <code>examples/11_voxel_raycast/</code></p>
<p>Software DDA raycasting over a fixed <code>MAP³</code> voxel grid: dense indexing, 3D integer sampling along a ray, parallel scanline rendering, and solid-floor plus obstacle hits. It closes the voxel extension paths referenced from the 2D shooter and roguelike chapters—use it when learning chunk-local arrays, visibility probes, and the stepping blocks that precede face extraction or streaming chunk ownership.</p>
<h2 id="39-12_boids_flocking">39. 12_boids_flocking</h2>
<p>Path: <code>examples/12_boids_flocking/</code></p>
<p>Data-oriented flocking with fixed-capacity arrays for positions and velocities, perception-radius neighbor scans, separation/alignment/cohesion accumulation, and post-integration speed clamping. Steering forces are accumulated in separate buffers before mutating velocities, which keeps <code>@</code> updates from overlapping within a tick—matching the boids extension sketched in the tower-defense walkthrough.</p>
<h2 id="40-13_multiplayer_tick">40. 13_multiplayer_tick</h2>
<p>Path: <code>examples/13_multiplayer_tick/</code></p>
<p>Authoritative 60Hz fixed-timestep simulation with stack-resident player SoA arrays, tick accumulation, and a deterministic <code>CHECKSUM</code> over replicated state. Pair with <code>07_realtime_chat_broker</code> for transport; prefer binary WebSocket + Protobuf (or gRPC) on the wire with JSON for debugging and browser tools.</p>
<h2 id="41-complete-project-field-guide">41. Complete project field guide</h2>
<p>All projects are runnable modules with executable tests. From the repository root, use:</p>
<pre><code class="language-bash">super check examples/NN_name/src/main.sr
super test examples/NN_name/
super run examples/NN_name/src/main.sr</code></pre>
<p>The first ten projects are the original application suite. Projects 11–20 form a second, complete workload set: three data-oriented games, a binary TCP broker, a radix HTTP core, a RESP/LRU cache, a log pipeline, a rigid-body simulation, a multi-room WebSocket core, and an authoritative synchronized game server.</p>
<h3 id="411-examples01_space_shooter">41.1 <code>examples/01_space_shooter</code></h3>
<p>Purpose: a compact arcade simulation with ECS components, collision tests, parallel physics intent, compile-time trigonometric tables, channels, and hot-reloadable world state.</p>
<p>Key features: <code>state</code>, named value structs, <code>@self</code> methods, ECS component queries, <code>#for</code>, <code>#simd</code>, <code>for parallel</code>, channels, scoped tasks, and tests.</p>
<pre class="sr-block"><code class="language-sr">state world = ecs.World{}

+update(dt: f64) {
    #simd
    for parallel @(pos: Pos), (vel: Vel) in world {
        pos.x += vel.vx * dt
        pos.y += vel.vy * dt
    }
}</code></pre>
<p>Run <code>super test examples/01_space_shooter/</code>. Extend it by separating collision candidate generation from damage commands so parallel movement cannot race structural world changes.</p>
<h3 id="412-examples02_task_api">41.2 <code>examples/02_task_api</code></h3>
<p>Purpose: a multi-module backend skeleton with models, authentication, routes, concurrent cache/database work, an audit channel, HTTP helpers, JWTs, logging, hashes, and post-quantum key examples.</p>
<p>Key features: selective <code>use</code>, typed DTOs, <code>scope</code>/<code>spawn</code>, bounded channels, JSON responses, value errors, and module-private tests.</p>
<pre class="sr-block"><code class="language-sr">scope {
    t_user = spawn fetch_user(id)
    t_cache = spawn fetch_cache(id)
    user = await t_user
    cached = await t_cache
    total = user.id + cached
}</code></pre>
<p>Run <code>super test examples/02_task_api/</code>. For production, keep JSON routes as the fallback and add Protobuf over binary WebSocket or a gRPC/Tonic host for the primary typed transport.</p>
<h3 id="413-examples03_log_analyzer">41.3 <code>examples/03_log_analyzer</code></h3>
<p>Purpose: high-volume log parsing and aggregation with embedded fixtures, compile-time character classes, slices, regex, metrics, pipelines, and chunk distribution.</p>
<p>Key features: <code>embed</code>, sum types, <code>#for</code>, UFCS fusion, string slices, shared counters, and a million-record scale test.</p>
<pre class="sr-block"><code class="language-sr">+analyze(scores) : i64 {
    scores.keep(it &gt;= 25).map(it * 2).sum()
}</code></pre>
<p>Run <code>super test examples/03_log_analyzer/</code>. A production reader should add bounded input chunks, malformed-line counters, backpressure, and rotation handling without retaining whole files.</p>
<h3 id="414-examples04_roguelike_dungeon">41.4 <code>examples/04_roguelike_dungeon</code></h3>
<p>Purpose: BSP-flavored room counting, grid movement, A* integration, FOV-style visibility, ASCII dimensions, deterministic direction tables, and AI work queues.</p>
<p>Key features: sum types, compile-time direction arrays, channels, shared values, and <code>std.game.ai</code>.</p>
<pre class="sr-block"><code class="language-sr">#DIR_DX = [0, 1, 0, -1]
#DIR_DY = [-1, 0, 1, 0]

+neighbor_steps() : i64 {
    (0..4).map(#DIR_DX[it] * #DIR_DX[it] + #DIR_DY[it] * #DIR_DY[it]).sum()
}</code></pre>
<p>Run <code>super test examples/04_roguelike_dungeon/</code>. Pin random seeds and test connectivity invariants instead of snapshots of decorative output.</p>
<h3 id="415-examples05_tower_defense">41.5 <code>examples/05_tower_defense</code></h3>
<p>Purpose: a dense tower-defense workload with 50,000-creep scale tests, spatial cell helpers, targeting, projectiles, economy, priority queues, and deterministic noise.</p>
<p>Key features: dense records, <code>for parallel</code>, <code>#simd</code>, compile-time neighbor offsets, channels, collections, and noise.</p>
<pre class="sr-block"><code class="language-sr">+retarget(creeps: i64, towers: i64) : i64 {
    #simd
    for parallel 0..creeps {
        inspect_target_band(it, towers)
    }
    creeps * towers
}</code></pre>
<p>Run <code>super test examples/05_tower_defense/</code>. Real targeting should partition space and write one target slot per creep, avoiding a shared counter in the hot parallel pass.</p>
<h3 id="416-examples06_physics_sandbox">41.6 <code>examples/06_physics_sandbox</code></h3>
<p>Purpose: 2D rigid bodies, AABB and circle overlap, Euler integration, broad-phase sizing, unrolled solver passes, contact channels, and <code>std.game.physics2d</code>.</p>
<p>Key features: struct methods, <code>@self</code>, <code>for parallel</code>, <code>#unroll</code>, sum variants, and deterministic collision tests.</p>
<pre class="sr-block"><code class="language-sr">+RigidBody = {
    x: f64
    y: f64
    vx: f64
    vy: f64

    integrate_euler(@self, dt: f64) {
        self.x += self.vx * dt
        self.y += self.vy * dt
    }
}

@body.integrate_euler(0.016)</code></pre>
<p>Run <code>super test examples/06_physics_sandbox/</code>. Keep integration, candidate generation, contact construction, and iterative solving as separate phases.</p>
<h3 id="417-examples07_realtime_chat_broker">41.7 <code>examples/07_realtime_chat_broker</code></h3>
<p>Purpose: a multi-room message broker with bounded channels, heartbeat timeouts, shared delivery counters, parallel room work, compile-time wire metadata, and WebSocket frame tests.</p>
<p>Key features: <code>when</code> multiplexing, <code>after</code>, structured concurrency, sums, and <code>std.net.ws</code>.</p>
<pre class="sr-block"><code class="language-sr">when {
    stop.recv() =&gt; return
    after 1.ms =&gt; send_ping()
}</code></pre>
<p>Run <code>super test examples/07_realtime_chat_broker/</code>. Add per-connection output limits so a slow client cannot retain unbounded messages.</p>
<h3 id="418-examples08_kv_cache_engine">41.8 <code>examples/08_kv_cache_engine</code></h3>
<p>Purpose: a sharded in-memory key/value core with command channels, TTL sweeps, compile-time shard masks/seeds, Redis-like behavior, and collection tests.</p>
<p>Key features: <code>#{...}</code>, <code>#for</code>, <code>#inline</code>, shared counters, channels, and <code>std.db.redis</code>.</p>
<pre class="sr-block"><code class="language-sr">#SHARD_BITS = 2
#SHARD_MASK = #{ 4 - 1 }

+shard(key: i64) : i64  =&gt; key &amp; #SHARD_MASK</code></pre>
<p>Run <code>super test examples/08_kv_cache_engine/</code>. Production cache design also needs capacity pressure, eviction policy, snapshot/replication semantics, and crash tests.</p>
<h3 id="419-examples09_http_load_tester">41.9 <code>examples/09_http_load_tester</code></h3>
<p>Purpose: concurrent synthetic HTTP generation, sample streaming, histogram bounds, percentile indexes, and metrics.</p>
<p>Key features: spawned workers, bounded sample channels, shared counters, compile-time histogram tables, HTTP simulation, and metrics.</p>
<pre class="sr-block"><code class="language-sr">#HIST_BOUNDS = #for i in 0..6 { i * 200 }

for bound in #HIST_BOUNDS {
    if latency_us &gt;= bound { bucket += 1 }
}</code></pre>
<p>Run <code>super test examples/09_http_load_tester/</code>. Real load generation needs connection reuse, rate shaping, TLS, timeout policy, and correction for coordinated omission.</p>
<h3 id="4110-examples10_mapreduce_kv">41.10 <code>examples/10_mapreduce_kv</code></h3>
<p>Purpose: partitioned mapping, bounded batches, a reducer task, n-gram counting, ring buffers, and stage logging.</p>
<p>Key features: channels as ownership transfer, shared reduction state, compile-time constants, collections, and deterministic tests.</p>
<pre class="sr-block"><code class="language-sr">scope {
    reducer = spawn reduce_batches(batches, @reduced)
    for partition in partitions {
        batches.send(map_partition(partition))
    }
    batches.close()
    await reducer
}</code></pre>
<p>Run <code>super test examples/10_mapreduce_kv/</code>. Distributed evolution requires partition identity, retry identity, idempotence, explicit wire schemas, and backpressure.</p>
<h3 id="4111-examples11_voxel_raycast">41.11 <code>examples/11_voxel_raycast</code></h3>
<p>Purpose: software grid ray marching over a fixed 16³ voxel field with parallel scanlines and a checksum.</p>
<p>What Super does well: <code>[0; 4096]</code> creates dense map storage, <code>sample</code> performs bounded integer indexing, <code>par y</code> gives workers independent row destinations, and <code>rows.sum()</code> closes the workload with a deterministic gate.</p>
<pre class="sr-block"><code class="language-sr">rows = [0; H]
par y in 0..H {
    local = 0
    for x in 0..W {
        local += cast(map, 8.0, 3.0, 8.0, ray_x(x), -0.02, ray_z(y))
    }
    rows[y] = local
}</code></pre>
<p>Run:</p>
<pre><code class="language-bash">super run examples/11_voxel_raycast/src/main.sr
super test examples/11_voxel_raycast/</code></pre>
<p>The next step is chunk ownership: keep chunk voxels persistent, allocate visible faces per frame, and schedule scanlines or tiles with disjoint outputs.</p>
<h3 id="4112-examples12_boids_flocking">41.12 <code>examples/12_boids_flocking</code></h3>
<p>Purpose: fixed-capacity flocking with SoA positions/velocities, neighborhood scans, steering accumulation, speed limits, wraparound, and deterministic seeds.</p>
<p>What Super does well: acceleration arrays are frame scratch; old position and velocity arrays remain read-only during neighbor scans; updates happen in a separate pass through exclusive roots.</p>
<pre class="sr-block"><code class="language-sr">+step(@px, @py, @vx, @vy) {
    ax, ay = [0.0; N], [0.0; N]
    for i in 0..N { accumulate_neighbors(i, px, py, vx, vy, @ax, @ay) }
    for i in 0..N {
        vx[i] += ax[i]
        vy[i] += ay[i]
    }
}</code></pre>
<p>Run <code>super run examples/12_boids_flocking/src/main.sr</code> and <code>super test examples/12_boids_flocking/</code>. A spatial grid can reduce the quadratic neighbor scan while preserving the double-buffered update pattern.</p>
<h3 id="4113-examples13_multiplayer_tick">41.13 <code>examples/13_multiplayer_tick</code></h3>
<p>Purpose: authoritative fixed-60-Hz simulation, accumulated fractional frame time, bounded catch-up, SoA player state, and deterministic snapshot checksums.</p>
<p>What Super does well: one-element array state makes mutation explicit, fixed steps separate simulation from frame arrival, and checksum output makes replay tests cheap.</p>
<pre class="sr-block"><code class="language-sr">+simulate_tick(@x, @y, @vx, @vy) {
    for player in 0..PLAYER_CAPACITY {
        x[player] += vx[player] * TICK_DT
        y[player] += vy[player] * TICK_DT
    }
}</code></pre>
<p>Run <code>super run examples/13_multiplayer_tick/src/main.sr</code> and <code>super test examples/13_multiplayer_tick/</code>. Pair it with binary WebSocket plus Protobuf snapshots and retain JSON messages for debugging and browser fallback.</p>
<h3 id="4114-examples14_tcp_message_broker">41.14 <code>examples/14_tcp_message_broker</code></h3>
<p>Purpose: length-prefixed binary frames, topic subscriptions, flat membership tables, fanout, deterministic sequence checks, and 2,000-message workload output.</p>
<p>What Super does well: wire buffers have fixed capacity; <code>encode_frame</code> mutates only its destination; subscriptions flatten to <code>subscriber * TOPIC_COUNT + topic</code>; fanout writes explicit mailbox roots.</p>
<pre class="sr-block"><code class="language-sr">+fanout(subscriptions, topic: i64, payload_sum: i64, @mailboxes) : i64 {
    delivered = 0
    for subscriber in 0..SUBSCRIBER_COUNT {
        if subscriptions[subscriber * TOPIC_COUNT + topic] == 1 {
            mailboxes[subscriber] += payload_sum
            delivered += 1
        }
    }
    delivered
}</code></pre>
<p>Run <code>super run examples/14_tcp_message_broker/src/main.sr</code> and <code>super test examples/14_tcp_message_broker/</code>. A socket adapter must handle partial frames, enforce <code>MAX_FRAME</code>, and apply per-subscriber backpressure.</p>
<h3 id="4115-examples15_http_radix_framework">41.15 <code>examples/15_http_radix_framework</code></h3>
<p>Purpose: method-aware radix-style routing, zero-copy path parameter slices, typed route matches, static and parameter routes, and a 10,000-request checksum.</p>
<p>What Super does well: early returns keep matching linear to read, slices avoid allocating path segments, and a flat handler ID supports direct dispatch.</p>
<pre class="sr-block"><code class="language-sr">if method == METHOD_GET &amp;&amp; starts_with(path, &quot;/users/&quot;) {
    return RouteMatch{
        handler: 4,
        parameter: parse_decimal(path[7..byte_len(path)]),
        matched: true,
    }
}</code></pre>
<p>Run <code>super run examples/15_http_radix_framework/src/main.sr</code> and <code>super test examples/15_http_radix_framework/</code>. A generated radix tree can move route-shape decisions to compile time while leaving parameter validation in the request stack.</p>
<h3 id="4116-examples16_resp_lru_cache">41.16 <code>examples/16_resp_lru_cache</code></h3>
<p>Purpose: RESP-like command parsing, fixed-capacity O(1)-style table slots, LRU stamps, TTL expiry, deletion, deterministic pressure, and 5,000 operations.</p>
<p>What Super does well: keys, values, expiries, and stamps are SoA arrays; all capacity is explicit; cache mutations name their roots; no node allocation is required.</p>
<pre class="sr-block"><code class="language-sr">+cache_set(@keys, @values, @expires, @stamps, key: i64, value: i64, now: i64) {
    slot = find_slot(keys, key)
    if slot &lt; 0 { slot = choose_slot(keys, stamps) }
    keys[slot], values[slot] = key, value
    expires[slot], stamps[slot] = 0, now
}</code></pre>
<p>Run <code>super run examples/16_resp_lru_cache/src/main.sr</code> and <code>super test examples/16_resp_lru_cache/</code>. A production implementation should replace scans with an indexed table/list while retaining fixed bounds and expiry tests.</p>
<h3 id="4117-examples17_log_ingest_pipeline">41.17 <code>examples/17_log_ingest_pipeline</code></h3>
<p>Purpose: 200,000 synthetic records, chunked ingestion, service/status/latency aggregates, sum variants, fused filtering/mapping, and deterministic byte totals.</p>
<p>What Super does well: records are generated and consumed inside the chunk lifetime; aggregate arrays persist; only compact counters cross chunk boundaries; UFCS expresses a secondary pipeline.</p>
<pre class="sr-block"><code class="language-sr">+ingest_chunk(start: i64, end: i64, @services, @statuses, @latencies) : i64 {
    bytes = 0
    for index in start..end {
        record = make_record(index)
        services[record.service] += 1
        statuses[record.status] += 1
        latencies[classify_latency(record.latency_ms)] += 1
        bytes += record.bytes
    }
    bytes
}</code></pre>
<p>Run <code>super run examples/17_log_ingest_pipeline/src/main.sr</code> and <code>super test examples/17_log_ingest_pipeline/</code>. Parallel chunks should produce worker-local counters, followed by a bounded merge.</p>
<h3 id="4118-examples18_rigidbody_sandbox">41.18 <code>examples/18_rigidbody_sandbox</code></h3>
<p>Purpose: 96-body fixed-step integration, gravity, boundary restitution, pairwise circle contacts, two solver iterations, and 180-tick checksums.</p>
<p>What Super does well: SoA roots expose mutation and layout, radius is read-only, solver passes are compile-time-unrolled, and deterministic tests isolate gravity and contact behavior.</p>
<pre class="sr-block"><code class="language-sr">+step(@x, @y, @vx, @vy, radius) : i64 {
    integrate(@x, @y, @vx, @vy, radius, DT)
    contacts = 0
    #unroll(2)
    for 0..2 {
        contacts += solve_contacts(@x, @y, @vx, @vy, radius)
    }
    contacts
}</code></pre>
<p>Run <code>super run examples/18_rigidbody_sandbox/src/main.sr</code> and <code>super test examples/18_rigidbody_sandbox/</code>. Add broad-phase cells before raising body count; preserve a scalar reference for solver verification.</p>
<h3 id="4119-examples19_ws_multiroom_chat">41.19 <code>examples/19_ws_multiroom_chat</code></h3>
<p>Purpose: WebSocket-shaped frames, multi-room membership, sender exclusion, fanout into client inboxes, standard WebSocket round trips, and 4,000 messages.</p>
<p>What Super does well: membership is a flat bit table, frame decoding produces a small value record, and publish mutates one explicit inbox root.</p>
<pre class="sr-block"><code class="language-sr">+publish(memberships, frame: ChatFrame, @inboxes) : i64 {
    delivered = 0
    for client in 0..CLIENT_COUNT {
        if client != frame.sender &amp;&amp; memberships[client * ROOM_COUNT + frame.room] == 1 {
            inboxes[client] += frame.payload_sum + frame.sequence
            delivered += 1
        }
    }
    delivered
}</code></pre>
<p>Run <code>super run examples/19_ws_multiroom_chat/src/main.sr</code> and <code>super test examples/19_ws_multiroom_chat/</code>. Use Protobuf in binary frames for the primary contract and JSON text frames as a fallback.</p>
<h3 id="4120-examples20_game_server_ws_sync">41.20 <code>examples/20_game_server_ws_sync</code></h3>
<p>Purpose: a 60-Hz authoritative server, 48-player SoA state, input application, world wrapping, quantized snapshots, WebSocket binary framing, and periodic state synchronization.</p>
<p>What Super does well: input records are values, velocity and position mutation are separate, snapshot output is destination-driven into a fixed packet, and checksums cover synchronization output.</p>
<pre class="sr-block"><code class="language-sr">+encode_snapshot(@packet, tick: i64, x, y) : i64 {
    packet[0] = 130
    cursor = 4
    for player in 0..PLAYER_CAPACITY {
        write_quantized(@packet, cursor, player, x[player], y[player], tick)
        cursor += 6
    }
    cursor
}</code></pre>
<p>Run <code>super run examples/20_game_server_ws_sync/src/main.sr</code> and <code>super test examples/20_game_server_ws_sync/</code>. Evolve this toward delta snapshots, acknowledgements, input sequence validation, interest management, and JSON diagnostics.</p>
<h3 id="4127-examples27_gui_todo">41.27 <code>examples/27_gui_todo</code></h3>
<p>Purpose: a todo desk whose rows live in <code>std.db.sqlite</code> and whose window declares <code>Widget</code>. Add refuses an empty title, toggle flips <code>done</code>, and the filter is <code>all</code>, <code>open</code>, or <code>done</code>.</p>
<p>What Super does well: <code>+TodoApp: Widget</code> is checked at compile time. <code>paint</code> builds a <code>std.gui</code> tree (title bar, field, badge, toast) and returns the display list. Tests read that string and the sqlite rows. Running the program opens the platform window: GTK 4 on Linux (Wayland, then X11), Win32 on Windows, Cocoa on macOS. <code>SUPER_GUI=win32</code> or <code>SUPER_GUI=cocoa</code> selects those providers; <code>SUPER_GUI=gtk</code> keeps GTK, including on Windows and macOS.</p>
<p>Run <code>super test examples/27_gui_todo</code>. Nineteen assertions cover insert, validation, filter, delete, hit testing, and measure.</p>
<h3 id="4128-examples28_gui_breakout">41.28 <code>examples/28_gui_breakout</code></h3>
<p>Purpose: a paddle, a sprite ball, a row of bricks, and bounce or break cues.</p>
<p>What Super does well: <code>Breakout</code> declares <code>Widget</code>. <code>tick</code> mutates the ball. <code>paint</code> asks <code>Sprite</code> and <code>Cue</code> for display-list lines (<code>sprite …</code>, <code>sound bounce 80</code>).</p>
<p>Run <code>super test examples/28_gui_breakout</code>.</p>
<h3 id="4129-examples29_gui_invaders">41.29 <code>examples/29_gui_invaders</code></h3>
<p>Purpose: a player sprite, an alien row whose width tracks how many remain, one shot, and <code>shot</code> / <code>hit</code> cues.</p>
<p>What Super does well: the same <code>Widget</code> header as the desk, with <code>std.game.fx</code> for the cels. <code>fire</code> and <code>tick</code> are ordinary methods beside <code>paint</code>.</p>
<p>Run <code>super test examples/29_gui_invaders</code>.</p>
<h3 id="4130-examples30_gui_snake">41.30 <code>examples/30_gui_snake</code></h3>
<p>Purpose: a head that moves, food that grows <code>body</code>, and a wall that ends the run.</p>
<p>What Super does well: the length field is <code>body</code>, because <code>len</code> is the array intrinsic. Eating writes <code>sound eat 60</code>. A later tick on a dead snake returns <code>dead</code> and does not move.</p>
<p>Run <code>super test examples/30_gui_snake</code>.</p>
<h2 id="42-a-staged-learning-project">42. A staged learning project</h2>
<p>Build a small telemetry service:</p>
<ol><li>Define <code>Event</code> as a struct and <code>Severity</code> as a sum.</li><li>Accept a JSON route with <code>or err</code>.</li><li>Send events over <code>chan[Event]</code> to a collector task.</li><li>Aggregate counts with dense arrays and <code>.sum()</code>.</li><li>Publish metrics and a JSON health endpoint.</li><li>Add a binary transport in the host while retaining JSON.</li><li>Compile with C23 and compare VM/JIT/AOT checksums.</li></ol>
<p>Skeleton:</p>
<pre class="sr-block"><code class="language-sr">use std.net.http
use std.diag.metrics

+Event = {
    service: str
    latency_us: i64
    ok: bool = true
}

state metrics = Metrics.new()
state events = chan[Event](1024)

#post(&quot;/event&quot;)
event(req: http.Request) : http.Response! {
    event = req.json(Event) or err {
        return http.bad_request(.{error: &quot;{err}&quot;})
    }
    events.send(event)
    http.ok(.{accepted: true})
}

+collector() {
    for event in events {
        @metrics.inc(&quot;events&quot;, 1)
        @metrics.observe(&quot;latency_us&quot;, event.latency_us)
    }
}

test &quot;event route rejects malformed JSON&quot; {
    response = http.simulate_post(&quot;/event&quot;, &quot;{bad&quot;)
    assert_eq(response.status, 400)
}</code></pre>
<h2 id="42-engineering-checklist">42. Engineering checklist</h2>
<p>Before calling a Super program complete:</p>
<ul><li>run <code>super check</code> on every entry module;</li><li>run <code>super test</code> at the package or repository root;</li><li>treat E0303–E0305 as design feedback, not noise;</li><li>verify every <code>@</code> parameter is truly mutation-capable and non-escaping;</li><li>checksum optimized kernels against a simple reference;</li><li>test optional/error failure paths;</li><li>bound channels, arrays, request sizes, and resource counts;</li><li>separate persistent <code>state</code> from request/frame/task temporaries;</li><li>test both scalar fallback and optimized C23/JIT paths;</li><li>record benchmark machine, compiler, warmups, runs, and variance;</li><li>preserve JSON transport fallbacks when adding binary protocols;</li><li>commit <code>Pod.lock</code> for reproducible applications.</li></ul>
<h1 id="part-vii--tooling--performance">Part VII — Tooling &amp; Performance</h1>
<p>Super is a toolchain, not only a parser. The CLI, diagnostics, language server, editor grammars, benchmark harness, VM/JIT, and C23 backend are designed to agree on one program model.</p>
<h2 id="43-cli-workflow">43. CLI workflow</h2>
<p>The primary commands are:</p>
<pre><code class="language-bash">super check path/to/main.sr
super r path/to/main.sr
super test path/or/package
super b -d path/to/main.sr
super b -r --c23 path/to/main.sr -o dist/app
super b --profile size --c23 -Oz path/to/main.sr -o dist/app
super install
super which
super list
super uninstall
super clean -d
super clean -r
super clean
super help
super lsp
super mcp</code></pre>
<p><code>check</code> lexes, parses, resolves, infers, validates mutation/lifetimes, and emits diagnostics without running the program. Use it as the fastest feedback loop. <code>super r</code> is <code>super run</code>. <code>super b</code> is <code>super build</code>. Either command with no file uses the Pod's <code>Pod.toml</code> and the entry for its <code>kind</code> (<code>src/main.sr</code>, <code>src/lib.sr</code>, or <code>src/macros.sr</code>). <code>-d</code> is the dev profile (<code>-O0 -g</code>, tests kept). <code>-r</code> is the release profile, and it is the fastest setting the toolchain has: <code>-O3</code>, fat LTO, <code>-march=native</code>, <code>-ffast-math</code>, unrolling, and direct calls, with tests stripped. <code>--profile test</code> is <code>-O2 -g</code> with tests kept (this is what <code>super test</code> selects). <code>--profile size</code> is the small binary. <code>--profile fast</code> matches release. <code>-O0</code> through <code>-O3</code>, <code>-Os</code>, <code>-Oz</code>, <code>-Og</code>, and <code>-Ofast</code> override just the optimization level. A flag on the command line wins over the Pod manifest. The default backend is <code>c23</code> and the default mode is <code>aot</code>, so <code>super b -r &lt;pod&gt;</code> writes the native executable at <code>target/release/&lt;pod&gt;</code> for bin Pods. <code>--c23</code> still selects that stack explicitly. <code>--mode bytecode</code> writes a portable <code>.superb</code> module instead. While the build runs, the CLI prints each step. The current step name is cyan-green, with the step's text and progress beside it. The last line is <code>Finished</code> in cyan-green, <code>Failed</code> in cyan-red, or <code>Finished with warnings</code> in cyan-yellow.</p>
<p><code>super install</code> (<code>super i</code>) is the ship step for <strong>bin</strong> Pods. From the Pod directory, or any subdirectory, it builds <code>src/main.sr</code> as a native C23 binary at the maximum release profile and copies it onto <code>PATH</code>. The executable name is the Pod name. Lib and macro Pods are skipped. The directory is <code>--bin-dir</code>, <code>SUPER_INSTALL_DIR</code>, a user bin already on <code>PATH</code> (<code>~/.local/bin</code>, <code>~/bin</code>, or <code>~/.cargo/bin</code>), or <code>~/.local/bin</code> after Super adds that directory to the shell startup file. On Windows the same directories live under the user profile and Super updates the user <code>Path</code>. <code>super uninstall</code> (<code>super u</code>) removes the recorded binary and leaves <code>PATH</code> in place. <code>super list</code> shows every receipt. <code>super which</code> prints one path.</p>
<p><code>super clean -d</code> removes debug binaries (<code>target/dev</code>). <code>super clean -r</code> removes release binaries (<code>target/release</code>). <code>super clean</code> and <code>super clean -a</code> remove the whole <code>target/</code> directory. A workspace is a directory of Pods, or a manifest with <code>[workspace] members</code>; install and clean act on every member. <code>super help</code> lists every command, and <code>super help install</code> prints one.</p>
<p>Pod commands resolve <code>Pod.toml</code>, honor <code>Pod.lock</code> / <code>Pod.lock</code>, and can vendor source. Applications should commit the lockfile. Library and macro Pods should document supported version ranges and test their public modules through a small consumer Pod.</p>
<p>A productive loop is:</p>
<pre><code class="language-bash">super check examples/17_log_ingest_pipeline/src/main.sr
super test examples/17_log_ingest_pipeline/
super run examples/17_log_ingest_pipeline/src/main.sr
super build --release --c23 examples/17_log_ingest_pipeline/src/main.sr</code></pre>
<p>Do not skip <code>check</code> merely because one runtime path succeeds. Static diagnostics are part of the contract, and release lowering may exercise stricter layout or alias information.</p>
<h2 id="44-diagnostics-as-design-feedback">44. Diagnostics as design feedback</h2>
<p>Stable semantic codes distinguish categories:</p>
<ul><li>E0101–E0104: names, modules, members, and visibility;</li><li>E0201–E0203: type unification, unhandled optional/error values, and trait conformance;</li><li>E0301–E0302: missing mutation borrow and unsafe concurrent capture;</li><li>E0303–E0305: unused local, parameter, or import;</li><li>E0401: an escaping mutable root or scoped value.</li></ul>
<p>Fix the primary span first. A missing <code>@</code> is not solved by suppressing a warning; it asks whether the call really mutates and whether exclusive access is available. An escape error asks which lifetime should own the result. An unused import often means a refactor is incomplete or a module boundary is too broad.</p>
<p>CI should run <code>super check</code> and <code>super test</code> over package roots, then build the release target used in deployment. Optimized kernels should have checksum or reference tests that run through scalar and optimized paths.</p>
<h2 id="45-language-server">45. Language server</h2>
<p>Start the language server with <code>super lsp</code> or the dedicated <code>super-lsp</code> binary. It speaks JSON-RPC 2.0 over standard input/output and supports:</p>
<ul><li>open/change synchronization and live diagnostics;</li><li>inferred-type, layout, offset, stack-behavior, and trait-signature hover;</li><li>definition and reference navigation through interned symbols/spans;</li><li>completion for visible fields, imports, implicit <code>it</code>/<code>index</code>, and UFCS methods valid for the receiver type;</li><li>semantic tokens that distinguish exported declarations, private fields, mutable borrows, types, and implicit names.</li></ul>
<p>Editor clients must not write protocol logs to standard output because that corrupts JSON-RPC framing. Use standard error or a configured log file.</p>
<p>Incremental updates re-run the smallest practical frontend region, but semantic truth remains the same as <code>super check</code>. If editor output disagrees with the CLI, save the file and treat CLI diagnostics as authoritative while reporting the mismatch with a minimal source case.</p>
<h2 id="451-agent-mcp-server">45.1 Agent MCP server</h2>
<p>Coding agents should use the embedded MCP knowledge server rather than scraping docs by hand:</p>
<pre><code class="language-bash">super mcp
# or the shim:
super-mcp</code></pre>
<p>It speaks Model Context Protocol over stdio (newline-delimited JSON-RPC 2.0; protocol <code>2024-11-05</code>). Point an MCP client at the <code>super mcp</code> command. Tools cover:</p>
<ul><li>language features and cheatsheets (<code>super_get_feature</code>, <code>super_cheatsheet</code>);</li><li>algorithm sketches in Super (<code>super_get_algorithm</code>);</li><li><code>examples/</code> project index (<code>super_list_examples</code>, <code>super_get_example</code>);</li><li>stdlib coverage and live module source (<code>super_list_stdlib</code>, <code>super_get_stdlib</code>);</li><li>unified search (<code>super_search</code>).</li></ul>
<p>Resources use <code>super://feature/…</code>, <code>super://algorithm/…</code>, <code>super://example/…</code>, and <code>super://stdlib/…</code>. Prompts <code>learn-super</code>, <code>write-super</code>, and <code>explore-stdlib</code> give agents a guided workflow. Set <code>SUPER_STDLIB</code> / <code>SUPER_STD</code> (or install via <code>make install</code> → <code>~/.super/lib/stdlib</code>) so stdlib tools can open the <code>.sr</code> files next to the compiler.</p>
<h2 id="46-editor-support">46. Editor support</h2>
<p>The repository ships:</p>
<ul><li>a VS Code TextMate grammar and language configuration under <code>editors/vscode/</code>;</li><li>a Tree-sitter grammar under <code>editors/tree-sitter-super/</code>;</li><li>Zed integration under <code>editors/zed/</code>.</li></ul>
<p>Syntax highlighting recognizes exports, private fields, mutation sigils, compile-time forms, tests, routes, the <code>trait</code> keyword, interpolation, and control-flow keywords. Semantic tokens from the LSP improve on lexical highlighting because they know whether a name is a type, field, mutable root, or implicit binding.</p>
<p>Editor grammars are not the language specification. A highlighted construct can still be invalid, and a temporarily unhighlighted valid construct remains valid. When syntax evolves, update lexer, parser, semantic analysis, formatter/tooling, TextMate, Tree-sitter, Zed, tests, the specification, and this book together.</p>
<h2 id="47-benchmark-methodology">47. Benchmark methodology</h2>
<p>Performance numbers without a contract are stories. A credible Super benchmark records:</p>
<ol><li>exact source revision and Super version;</li><li>OS, CPU model, core count, memory, and power policy;</li><li>host C compiler and flags for C23 AOT;</li><li>VM, JIT, and AOT modes measured separately;</li><li>warmup count, timed run count, and full distribution;</li><li>input size and deterministic seed;</li><li>checksum or output validation against a reference;</li><li>excluded workloads and the reason for exclusion.</li></ol>
<p>The repository benchmark suite uses ten algorithms and checksum gates. Run the harness from a quiet machine, allow warmups, and compare medians plus variance. For very short programs, process startup and timer resolution can dominate; increase work per process or use an in-process harness while retaining the same algorithm and checksum.</p>
<p>Report ratios in an unambiguous direction, such as “Super time / C time.” A value below 1 then means Super was faster for that workload. A geometric mean is useful across ratios but must list exclusions. It does not replace per-workload results; a strong Mandelbrot result cannot explain an A* regression.</p>
<h3 id="avoiding-benchmark-traps">Avoiding benchmark traps</h3>
<ul><li>Do not remove required work because a checksum happens not to observe it.</li><li>Do not compare a host intrinsic in one language with a source implementation in another without labeling the difference.</li><li>Do not time file generation, compilation, or startup unless the workload is specifically about those costs.</li><li>Do not use a parallel implementation against a forced single-thread baseline without reporting CPU utilization and both serial versions.</li><li>Do not assume source brevity implies speed.</li><li>Do not tune only the checksum path.</li></ul>
<p>Use profilers after correctness gates pass. Inspect allocation/stack growth, branch misses, cache behavior, worker balance, generated C, and final assembly.</p>
<h2 id="48-c23-aot">48. C23 AOT</h2>
<p>C23 is Super's broad optimizing backend. The frontend lowers checked source to typed IR/bytecode, then emits a self-contained C translation unit. The host compiler handles mature scalar optimization, vectorization, register allocation, and linking.</p>
<p>The emitter can use:</p>
<ul><li><code>static constexpr</code> data for compile-time values and lookup tables;</li><li><code>#embed</code> as a representation option for large embedded assets;</li><li><code>&lt;stdbit.h&gt;</code> operations for recognized bit primitives;</li><li><code>[[unsequenced]]</code> for eligible pure helpers;</li><li>typed structs and compact tag/payload sums;</li><li><code>restrict</code> on proven exclusive mutable arrays/components;</li><li>OpenMP for recognized independent <code>par</code> kernels and reductions.</li></ul>
<p>Generated C is an implementation artifact, but reading it is valuable. Verify that hot loops have simple bounds, roots carry the intended qualifiers, compile-time tables are static, and no unexpected runtime constructor appears. Then inspect compiler optimization remarks or assembly.</p>
<p>C compatibility does not expose arbitrary C undefined behavior to Super source. The frontend must establish bounds, aliasing, initialization, and lifetime facts before emitting aggressive forms. Unsupported patterns lower to correct scalar code.</p>
<h3 id="interoperability-notes">Interoperability notes</h3>
<p>Named value structs have deterministic C-compatible layout. Validate size and offset assumptions with compile-time assertions and host-side static assertions. Do not pass stack-relative pointers beyond their lifetime. Strings and arrays need explicit ABI descriptors or host binding types, not assumptions about a C NUL-terminated pointer.</p>
<h2 id="49-vm-and-jit-measurement">49. VM and JIT measurement</h2>
<p>The register-oriented VM is the universal execution model and correctness fallback. The focused JIT recognizes profitable dense numeric kernels on x86-64 and AArch64, emits packed operations plus scalar tails, and protects executable memory through an audited RAII buffer.</p>
<p>Measure VM and JIT separately. A JIT run may include compilation overhead unless the harness warms it. A kernel can be too small to repay compilation. A shape the JIT does not recognize should remain correct through general bytecode.</p>
<p>When diagnosing a JIT/AOT difference:</p>
<ol><li>compare checksums;</li><li>reduce to the smallest kernel;</li><li>confirm static types and <code>@</code> alias facts;</li><li>inspect bounds and scalar tail behavior;</li><li>test lengths around vector width: 0, 1, width−1, width, width+1;</li><li>compare NaN, infinity, overflow, and signed shift cases where relevant.</li></ol>
<p>Never “fix” a mismatch by weakening the checksum. Optimized and scalar paths are the same language.</p>
<h2 id="50-release-checklist">50. Release checklist</h2>
<p>Before shipping a Super 0.1.0 application:</p>
<ul><li>run <code>super check</code> for every entry module;</li><li>run package and integration tests, including failure paths;</li><li>build the deployment target in release mode;</li><li>verify lockfile and vendored dependency policy;</li><li>bound requests, frames, channels, arrays, and resource counts;</li><li>ensure request/frame data cannot escape into persistent state;</li><li>validate binary protocols and keep a JSON fallback;</li><li>test reconnect, timeout, cancellation, and partial I/O behavior;</li><li>compare optimized kernels against scalar references;</li><li>record performance methodology and checksums;</li><li>verify logs redact secrets and metric labels are bounded;</li><li>validate C ABI layout at every FFI boundary;</li><li>exercise clean shutdown and resource closure;</li><li>retain deterministic seeds and replay fixtures for game/network simulations.</li></ul>
<h2 id="51-where-to-continue">51. Where to continue</h2>
<p>Use <a href="SPECIFICATION.md">SPECIFICATION.md</a> for normative grammar and diagnostics. Read <code>stdlib/mod.sr</code> for the shipped module catalog, <code>benchmarks/the latest verified results/results.json</code> for measurement detail, <code>docs/the latest verified results</code> for syntax refinements, and the example tests for executable usage.</p>
<p>Super 0.1.0 is intentionally compact, but it is not magical. Its performance comes from visible layout, bounded lifetime, static specialization, and giving the backend honest alias and parallelism facts. Write the clearest value-oriented program first; then use checksums, diagnostics, and profiles to decide which loops deserve sharper hints.</p><p><strong>Next:</strong> <a href="syntax.html">Language</a></p>
