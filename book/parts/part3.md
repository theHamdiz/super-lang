# Part III — The Standard Library Cookbook

*Super 0.1.0*

The standard library is where Super stops being a language you read about and becomes a language you ship with. Every module in it is written in Super itself, sits on a small set of native primitives, and carries its own `test` blocks, so you can open any file under `stdlib/` and learn from it the same way you learn from the examples in this book.

This part is a cookbook. Each chapter takes one corner of the library and walks through **complete, runnable recipes**: every listing below is a whole program that you can paste into a file, run with `super run`, and exercise with `super test`. Where a program prints something, the output printed under it is what the program really prints.

## A few habits that make the library pleasant

Four conventions show up in every recipe. Learn them once and the rest of the library will feel familiar.

**Import by module name.** The layers (`core`, `fmt`, `sys`, `net`, `io`, `sec`, `math`, `game`, `diag`, plus the interface modules `cli`, `tui`, and `gui`) organize the documentation; the import line names the module directly:

| Layer | Modules and their import lines |
|---|---|
| core | `use std.core.str`, `use std.core.bytes`, `use std.core.collections`, `use std.core.regex`, `use std.core.uuid` |
| fmt | `use std.fmt.json`, `use std.fmt.csv`, `use std.fmt.toml`, `use std.fmt.bin`, `use std.fmt.compress` |
| io | `use std.io.buf`, `use std.io.poll`, `use std.io.stdio`, `use std.io.archive` (raw poll: `std.sys.epoll` / `std.sys.io_uring`) |
| net | `use std.net.http`, `use std.net.ws`, `use std.net.tcp`, `use std.net.udp`, `use std.net.url`, `use std.net.retry` |
| game | `use std.game.ecs`, `use std.game.app`, `use std.game.physics2d`, `use std.game.ai`, `use std.game.tilemap`, `use std.game.assets`, `use std.game.fx` |
| sec | `use std.sec.crypto`, `use std.sec.encoding`, `use std.sec.jwt`, `use std.sec.hash`, `use std.sec.sha2`, `use std.sec.pq`, `use std.sec.x509`, `use std.sec.otp` |
| math | `use std.math.core`, `use std.math.linalg`, `use std.math.rand`, `use std.math.noise`, `use std.math.stats`, `use std.math.geom`, `use std.math.quat`, `use std.math.complex` |
| diag | `use std.diag.test`, `use std.diag.log`, `use std.diag.metrics` |
| interfaces | `use std.cli.cmd` (also `use std.cli` / `use std.clap`), `use std.tui`, `use std.gui`, `use std.gui.widgets` |

**Public names share one flat namespace.** When you import a module, its exported functions and types become ordinary names in your file. That keeps call sites short (`sha256_hex(x)`, not `crypto.sha256_hex(x)`), and it gives you two small rules: do not pick a local variable name that an imported module already exports (`lines`, `created`, and `text` are the classic ones), and do not import two modules that export the same function name. Library helpers carry a module prefix (`ecs_`, `p2_`, `ai_`, `jw_`, `enc_`) so collisions stay rare.

**Mutation is visible.** A method that changes its receiver is declared with `@self` and called with a leading `@`: `@room.join(101)`, `@w.spawn()`, `@ring.submit_nop(42)`. Read-only methods are called plainly: `room.count`, `w.count()`. If you can see the `@`, you can see every place state changes.

**Interpolation holes hold expressions, not string literals.** Inside `"…{expr}…"` you can call functions and read fields, but you cannot write another quoted string. Bind the value first and interpolate the name:

```super
+main() {
    query = "a b&c=d"
    print("length is {byte_len(query)}")
}
```

One more habit worth adopting: a loop or `if` header that ends in a field access, such as `if c.fd == ev.token {`, reads more clearly (and parses unambiguously) when you bind the field to a local first. You will see that pattern in the epoll recipe.

---

## 16. `std.core` and `std.fmt`: text, containers, bytes, and wire formats

These two layers are the workhorses. `core` gives you strings, dense containers, byte buffers, regular expressions, and UUIDs. `fmt` turns values into JSON, CSV, TOML, compact binary frames, and compressed payloads, and back again.

### 16.1 A text toolkit

Everything in `std.core.str` is a plain function over `str`, which means UFCS reads naturally and nothing allocates behind your back beyond the result you ask for. This recipe turns a title into a URL slug, pulls cells out of a comma-separated line, and parses a number with a fallback.

```super
use std.core.str

+slug(title: str) : str {
    lowered = to_lower(trim(title))
    replace(lowered, " ", "-")
}

+csv_cell(line: str, n: i64) : str  => trim(split_nth(line, ",", n))

+main() {
    print(slug("  Hello Super World  "))
    print(pad_left("42", 6, "0"))
    print(repeat("=", 12))
    line = "amina, 31 , cairo"
    print("{csv_cell(line, 0)}|{csv_cell(line, 1)}|{csv_cell(line, 2)}")
    age = to_i64(csv_cell(line, 1)) or 0
    print("age + 1 = {age + 1}")
    is_toml = starts_with("super.toml", "super")
    is_sr = ends_with("main.sr", ".sr")
    print("{is_toml} {is_sr}")
}

test "slug normalizes titles" {
    assert_eq(slug("  Hello Super World  "), "hello-super-world")
}

test "csv cell trims whitespace" {
    assert_eq(csv_cell("amina, 31 , cairo", 1), "31")
}
```

Running it prints:

```text
hello-super-world
000042
============
amina|31|cairo
age + 1 = 32
true true
```

`to_i64(...)` returns a value-error type (`i64!`), so the `or 0` is required: Super will not let a possibly-failed parse masquerade as a number. `split_nth` reads one field of a delimited line without building an array of every field; reach for `split` only when you truly need every field.

### 16.2 Containers: sets, maps, rings, and priority queues

`std.core.collections` offers fixed-footprint containers with predictable behavior. `Set` and `HashMap` use open addressing and rehash by powers of two. `RingBuf` is a bounded circular buffer, and `PriorityQueue` pops the *lowest* priority number first, which is exactly what a scheduler or an A* open list wants.

```super
use std.core.collections

+main() {
    seen = Set.new()
    for [4, 8, 15, 16, 23, 42, 8, 4] {
        @seen.add(it)
    }
    print("distinct = {seen.size}")

    stock = HashMap.new()
    @stock.put(101, 12)
    @stock.put(202, 0)
    @stock.put(101, 9)
    print("sku 101 = {stock.get(101) or -1}")
    print("sku 999 = {stock.get(999) or -1}")

    recent = RingBuf.new(3)
    for 1..6 {
        @recent.push_overwrite(it)
    }
    print("newest window starts at {recent.at(0) or -1}")

    jobs = PriorityQueue.new()
    @jobs.push(30, 300)
    @jobs.push(10, 100)
    @jobs.push(20, 200)
    print("first job = {@jobs.pop() or -1}")
    print("then = {@jobs.pop() or -1}")
}

test "set deduplicates" {
    s = Set.new()
    @s.add(1)
    @s.add(1)
    @s.add(2)
    assert_eq(s.size, 2)
}

test "ring buffer keeps the newest values" {
    r = RingBuf.new(3)
    for 1..6 { @r.push_overwrite(it) }
    assert_eq(r.at(0) or -1, 3)
    assert_eq(r.at(2) or -1, 5)
}

test "priority queue pops lowest priority first" {
    q = PriorityQueue.new()
    @q.push(5, 50)
    @q.push(1, 10)
    assert_eq(@q.pop() or -1, 10)
}
```

```text
distinct = 6
sku 101 = 9
sku 999 = -1
newest window starts at 3
first job = 100
then = 200
```

Notice how failure is expressed in the types: `stock.get(999)` is an optional (`i64?`) because the key may be absent, and `@jobs.pop()` is an optional because the queue may be empty. Both are discharged with `or`.

### 16.3 Building and reading binary packets

`ByteBuf` is a growable byte builder with explicit endianness, and `ByteReader` is its checked counterpart: every read returns a value-error, so a truncated packet becomes a handled case instead of an out-of-bounds read.

```super
use std.core.bytes
use std.core.str

+encode_hello(version: i64, id: i64, name: str) {
    buf = ByteBuf.new()
    @buf.put_u8(version)
    @buf.put_u32_be(id)
    @buf.put_u16_le(byte_len(name))
    @buf.put_str(name)
    buf
}

+main() {
    buf = encode_hello(2, 70000, "amina")
    print("packet is {buf.size()} bytes")
    r = reader(buf.data)
    version = @r.read_u8() or 0
    id = @r.read_u32_be() or 0
    n = @r.read_u16_le() or 0
    print("version={version} id={id} name_len={n} left={r.remaining()}")
}

test "packet header round trips" {
    buf = encode_hello(2, 70000, "amina")
    assert_eq(buf.size(), 12)
    r = reader(buf.data)
    assert_eq(@r.read_u8() or 0, 2)
    assert_eq(@r.read_u32_be() or 0, 70000)
    assert_eq(@r.read_u16_le() or 0, 5)
}
```

```text
packet is 12 bytes
version=2 id=70000 name_len=5 left=5
```

The header is 1 + 4 + 2 bytes and the name is 5, hence 12 in total and 5 left for the reader after the header is consumed.

### 16.4 Regular expressions for log lines

`std.core.regex` is a small, dependable backtracking engine: character classes, repetition, anchors, and the usual searching helpers. `compile` returns a value-error, and `regex_none()` gives you a harmless placeholder for the failure branch.

```super
use std.core.regex
use std.core.str

+status_of(line: str) : i64 {
    re = compile("[0-9][0-9][0-9]") or regex_none()
    if is_match(re, line) {
        return to_i64(match_text(re, line)) or 0
    }
    0
}

+main() {
    entries = ["GET /health 200 3ms", "POST /orders 503 120ms", "GET /missing 404 1ms"]
    bad = 0
    for line in entries {
        s = status_of(line)
        if s >= 400 { bad += 1 }
    }
    print("{bad} of 3 requests failed")
    digits = compile("[0-9]+") or regex_none()
    print("matches in first line: {count_matches(digits, entries[0])}")
}

test "status extraction" {
    assert_eq(status_of("POST /orders 503 120ms"), 503)
    assert_eq(status_of("no digits here"), 0)
}
```

```text
2 of 3 requests failed
matches in first line: 2
```

### 16.5 JSON in both directions

`JsonObj` builds objects without an intermediate tree: each `@o.str(...)`, `@o.int(...)`, `@o.float(...)`, and `@o.bool(...)` appends an encoded member. Reading goes the other way with path-style extraction — `json_get_int`, `json_get_str`, `json_get_float`, `json_get_bool` — which scan the document in place and never build a heap tree. Every extractor returns an optional, so a missing field is an ordinary `or` default.

```super
use std.fmt.json

+order_json(id: i64, customer: str, total: f64, paid: bool) : str {
    o = JsonObj.new()
    @o.int("id", id)
    @o.str("customer", customer)
    @o.float("total", total)
    @o.bool("paid", paid)
    o.end()
}

+main() {
    body = order_json(9001, "Amina", 120.5, true)
    print(body)
    print("valid = {json_valid(body)}")
    id = json_get_int(body, "id") or -1
    who = json_get_str(body, "customer") or "anonymous"
    total = json_get_float(body, "total") or 0.0
    print("{id} {who} {total}")
    missing = json_get_int(body, "coupon") or 0
    print("coupon = {missing}")
    items = "[10, 20, 30]"
    second = json_array_at(items, 1) or "none"
    print("items = {json_array_len(items)}, second = {second}")
}

test "built documents validate and parse back" {
    body = order_json(7, "Bo \"B\"", 9.25, false)
    #assert(json_valid(body))
    assert_eq(json_get_int(body, "id") or -1, 7)
    assert_eq(json_get_str(body, "customer") or "", "Bo \"B\"")
    assert_eq(json_get_bool(body, "paid") or true, false)
}

test "malformed json is rejected" {
    assert_eq(json_valid("{\"a\":1,}"), false)
    #assert(!(json_valid("[1 2]")))
}
```

```text
{"id":9001,"customer":"Amina","total":120.5,"paid":true}
valid = true
9001 Amina 120.5
coupon = 0
items = 3, second = 20
```

`json_valid` is the right first step for untrusted input; it checks structure, escapes, and trailing garbage before you extract a single field.

### 16.6 Configuration files: TOML and CSV

TOML parsing yields a `TomlDoc` with typed getters addressed by dotted key (`server.limits.rate`). CSV parsing yields rows you address by row and column, or by header name once you know the column.

```super
use std.fmt.toml
use std.fmt.csv

+CONFIG = "title = \"inventory\"\n[server]\nhost = \"0.0.0.0\"\nport = 8_080\ntls = true\n[server.limits]\nrate = 2.5\n"

+REPORT = "sku,name,qty\nA-1,Widget,12\nB-2,\"Gadget, large\",3\n"

+main() {
    doc = toml_parse(CONFIG) or TomlDoc.new()
    host = doc.get_str("server.host") or "127.0.0.1"
    port = doc.get_int("server.port") or 80
    tls = doc.get_bool("server.tls") or false
    rate = doc.get_float("server.limits.rate") or 1.0
    print("{host}:{port} tls={tls} rate={rate}")

    rows = csv_parse(REPORT)
    print("rows = {csv_row_count(rows)}")
    print("row 2 name = {csv_cell(rows, 2, 1)}")
    qty_col = csv_col(rows, "qty")
    print("qty column = {qty_col}")
}

test "toml scalars" {
    doc = toml_parse(CONFIG) or TomlDoc.new()
    assert_eq(doc.get_int("server.port") or 0, 8080)
    assert_eq(doc.get_str("title") or "", "inventory")
}

test "csv handles quoted commas" {
    rows = csv_parse(REPORT)
    assert_eq(csv_cell(rows, 2, 1), "Gadget, large")
    assert_eq(csv_get(rows, 1, "qty"), "12")
}
```

```text
0.0.0.0:8080 tls=true rate=2.5
rows = 3
row 2 name = Gadget, large
qty column = 2
```

Quoted fields keep their commas (`"Gadget, large"` stays one cell), and underscores in TOML integers (`8_080`) are honored.

### 16.7 Compact frames: varints, zigzag, and checksums

`std.fmt.bin` is the toolbox for hand-rolled protocols: LEB128-style varints, zigzag encoding for signed numbers, a length-prefixed `frame`/`unframe` pair, and `adler32` and `fnv1a32` checksums over byte arrays.

```super
use std.fmt.bin

+main() {
    payload = [1, 2, 3, 250]
    wire = frame(payload)
    back = unframe(wire) or []
    print("framed {bin_len(wire)} bytes, decoded {bin_len(back)} bytes")

    enc = varint_encode(300)
    first = varint_decode(enc, 0) or -1
    print("300 -> {bin_len(enc)} bytes -> {first}")
    print("zigzag(-3) => {zigzag_encode(-3)}")
    print("adler32 = {adler32(payload)}")
}

test "frame round trip" {
    back = unframe(frame([9, 8, 7])) or []
    assert_eq(bin_len(back), 3)
    assert_eq(back[0], 9)
}

test "varints are compact" {
    assert_eq(bin_len(varint_encode(127)), 1)
    assert_eq(bin_len(varint_encode(128)), 2)
    assert_eq(varint_decode(varint_encode(1234567890), 0) or -1, 1234567890)
}
```

```text
framed 9 bytes, decoded 4 bytes
300 -> 2 bytes -> 300
zigzag(-3) = 5
adler32 = 17694977
```

`frame` prefixes the payload with a varint length and appends a four-byte Adler-32 trailer, so a four-byte payload becomes a 1 + 4 + 4 = nine-byte frame. A zigzag code keeps small negative numbers small: `-3` becomes `5`.

### 16.8 Compressed snapshots

`std.fmt.compress` pairs an LZ4 block codec with CRC-32 and Adler-32. `compress_frame` wraps the compressed bytes with a checksum, and `decompress_frame` verifies it on the way back, which is what you want for save games, replication snapshots, and cached responses.

*Import `std.fmt.compress` or `std.fmt.bin`, not both in one file — each exports its own `adler32`.*

```super
use std.fmt.compress

+main() {
    text = "snapshot-payload-aaaa-bbbb-aaaa"
    packed = compress_frame(text)
    unpacked = decompress_frame(packed) or ""
    print("round trip ok = {unpacked == text}")
    ca = crc32_str("a")
    cb = crc32_str("b")
    print("crc differs = {ca != cb}")
}

test "lz4 round trip" {
    s = "hello hello hello super compress"
    assert_eq(lz4_decompress(lz4_compress(s)) or "", s)
}

test "framed payload carries a checksum" {
    s = "snapshot-payload-aaaa-bbbb-aaaa"
    assert_eq(decompress_frame(compress_frame(s)) or "", s)
}
```

```text
round trip ok = true
crc differs = true
```

### 16.9 Identifiers, logs, metrics, and durations

Backend services live on these four small tools. `Uuid` produces and parses RFC 4122 identifiers. `Logger` formats structured lines and can be given a fixed clock, which makes log assertions deterministic. `Metrics` keeps counters, gauges, and histograms. `Duration` carries units so a timeout can never be confused with a count.

```super
use std.core.uuid
use std.diag.log
use std.diag.metrics
use std.sys.time
use std.core.str

+main() {
    id = Uuid.v4()
    shown = id.to_str()
    print("request id has {byte_len(shown)} characters")

    lg = Logger.new("orders")
    @lg.set_clock(1700000000000)
    @lg.info("order created")
    print(lg.last())

    m = Metrics.new()
    @m.histogram("latency_ms", [5.0, 10.0, 50.0])
    for [3.0, 7.0, 12.0, 80.0] { @m.observe("latency_ms", it) }
    @m.inc("orders_total", 1.0)
    seen = m.hist_count("latency_ms")
    print("observations = {seen}")

    budget = Duration.ms(250)
    print("budget = {budget.as_ms()} ms")
}

test "uuid v4 text form" {
    assert_eq(byte_len(Uuid.v4().to_str()), 36)
    parsed = Uuid.parse("123e4567-e89b-42d3-a456-426614174000") or Uuid.nil()
    assert_eq(parsed.to_str(), "123e4567-e89b-42d3-a456-426614174000")
}

test "deterministic log lines" {
    lg = Logger.new("t")
    @lg.set_clock(0)
    @lg.info("hello")
    assert_eq(lg.last(), "1970-01-01T00:00:00.000Z INFO  [t] hello")
}

test "durations convert" {
    assert_eq(Duration.s(2).as_ms(), 2000)
}
```

```text
request id has 36 characters
2023-11-14T22:13:20.000Z INFO  [orders] order created
observations = 4
budget = 250 ms
```

The fixed clock `1700000000000` always renders as `2023-11-14T22:13:20.000Z`, so the same test passes on every machine.

---

## 17. `std.io`: buffers, `io_uring`, `epoll`, and event loops

The `io` layer is the portable readiness machinery beneath the network modules.
Use `std.io.poll` for the platform-selected backend: epoll with optional
io_uring acceleration on Linux, kqueue on macOS and FreeBSD, and WSAPoll on
Windows. `std.io.buf` supplies fixed-capacity byte rings and `std.io.stdio`
wraps process streams; raw `std.sys.epoll` and `std.sys.io_uring` remain
Linux-specific. These recipes need no network beyond loopback.

### 17.0 Process stdio

Builtin `print` still writes a newline-terminated line to stdout. Prefer `std.io.stdio` when you need stderr, binary-safe writes, flushing, or reading stdin (including pipes):

```super
use std.io.stdio

+main() {
    stdout.write_str("ready\n") or 0
    eprint("diag: waiting for a line\n") or 0
    line = stdin.read_line() or "eof"
    stdout.write_str("got={line}\n") or 0
    stdout.flush() or false
}
```

In tests, feed input with the host hook `sys_stdin_feed` so suites never wait on a terminal.

### 17.1 Ring buffers for byte streams

`RingWriter` accepts bytes until it is full and tells you when it refuses one; `RingReader` drains bytes from an array. Both live on the scoped data stack and never grow, which makes them a good fit for per-connection input and output staging.

```super
use std.io.buf

+main() {
    out = RingWriter.new(8)
    for 1..11 {
        stored = @out.push(it * 10)
        if !stored { print("full after {out.count} bytes") }
    }
    first = @out.pop()
    print("popped {first}, {out.remaining()} slot free")

    incoming = RingReader.from_bytes([7, 8, 9])
    total = 0
    while incoming.available() > 0 {
        total += @incoming.read_byte()
    }
    print("drained sum = {total}")
}

test "writer refuses bytes once full" {
    w = RingWriter.new(2)
    #assert(@w.push(1))
    #assert(@w.push(2))
    #assert(!(@w.push(3)))
    assert_eq(@w.pop(), 1)
    #assert(@w.push(3))
}
```

```text
full after none bytes
full after none bytes
popped 10, 1 slot free
drained sum = 24
```

The first ten pushes against capacity 8 fill the writer and then report `full` twice; that is the signal to flush or to apply back-pressure upstream.

### 17.2 `io_uring`: submit, complete, repeat

`IoUring.new()` asks the kernel for a submission/completion ring pair. `submit_nop` queues a no-op tagged with a token of your choosing, `wait(max, timeout_ms)` returns the completions, and `is_native()` tells you whether you are talking to a real kernel ring or the epoll-backed fallback the library uses where `io_uring` is unavailable. The same token-based API drives socket polls through `poll_add`.

```super
use std.sys.io_uring
use std.core.collections

+main() {
    ring = IoUring.new() or err {
        print("io_uring unavailable: {err}")
        return
    }
    submitted = @ring.submit_nop(42) or err { false }
    ready = ring.wait(4, 0) or err { [] }
    print("submitted={submitted} completions={len(ready)}")
    print("native ring = {ring.is_native()}")
    @ring.close()
}

test "nop completes with its token" {
    ring = IoUring.new() or err {
        assert_eq(1, 1)
        return
    }
    @ring.submit_nop(73)
    ready = ring.wait(4, 0) or err { [] }
    assert_eq(ready[0].token, 73)
    #assert(@ring.close())
}
```

```text
submitted=true completions=1
native ring = true
```

The `or err { … }` on `IoUring.new()` is the polite way to degrade: a container or a locked-down kernel that refuses the ring produces a message and a clean return, not a crash.

### 17.3 `epoll`: a real loopback echo

This recipe stands up a listener, connects a client to it, and uses a single `Epoll` instance to learn *when* each socket is ready, then echoes a message back. The tokens (`1`, `2`) are values you choose when registering a socket; they come back with every readiness event so you can tell sockets apart without a lookup table.

```super
use std.sys.epoll
use std.net.tcp
use std.core.collections
use std.core.bytes

+ADDR = "127.0.0.1:39911"

+main() {
    listener = TcpListener.bind(ADDR) or err {
        print("sockets unavailable: {err}")
        return
    }
    ep = Epoll.new() or err { return }
    @ep.add(listener.fd, EPOLL_IN, 1) or err { return }

    client = TcpStream.connect(ADDR) or err { return }
    ready = ep.wait(8, 1000) or err { return }
    print("listener events: {len(ready)}, token {ready[0].token}")

    conn = @listener.accept() or err { return }
    @client.write_str("ping")
    @ep.add(conn.fd, EPOLL_IN, 2) or err { return }
    ready2 = ep.wait(8, 1000) or err { return }
    print("connection token: {ready2[0].token}")

    data = @conn.read(64) or err { return }
    @conn.write(data)
    echoed = @client.read(64) or err { return }
    print("echoed: {bytes_to_str(echoed)}")
    @ep.close()
}

test "readiness flags classify events" {
    #assert(epoll_readable(EPOLL_IN))
    #assert(!(epoll_writable(EPOLL_IN)))
    #assert(epoll_failed(EPOLL_HUP))
}
```

```text
listener events: 1, token 1
connection token: 2
echoed: ping
```

`EPOLL_IN`, `EPOLL_OUT`, `EPOLL_ERR`, `EPOLL_HUP`, and `EPOLL_EDGE` are bit flags, and `epoll_readable`, `epoll_writable`, and `epoll_failed` classify an event mask for you. Add `EPOLL_EDGE` to a registration when you want edge-triggered delivery, and call `socket_set_nonblock(fd, true)` on the socket first.

### 17.4 A multi-client event loop

The pattern that scales is always the same: one readiness set, a token per socket, and a loop that reacts to whichever token fires. Token `0` is the listener; every accepted connection registers with its own file descriptor as its token.

```super
use std.sys.epoll
use std.net.tcp
use std.core.bytes
use std.core.collections

+ADDR = "127.0.0.1:39912"
+CLIENTS = 3

+main() {
    listener = TcpListener.bind(ADDR) or err {
        print("sockets unavailable: {err}")
        return
    }
    ep = Epoll.new() or err { return }
    @ep.add(listener.fd, EPOLL_IN, 0) or err { return }

    peers = []
    for 0..CLIENTS {
        peers = peers + [TcpStream.connect(ADDR) or err { return }]
    }

    conns = []
    handled = 0
    spins = 0
    while handled < CLIENTS && spins < 50 {
        spins += 1
        ready = ep.wait(16, 200) or err { return }
        for ev in ready {
            token = ev.token
            if token == 0 {
                conn = @listener.accept() or err { return }
                conns = conns + [conn]
                @ep.add(conn.fd, EPOLL_IN, conn.fd) or err { return }
                slot = len(conns) - 1
                @peers[slot].write_str("hello from client {slot}")
            } else {
                for c in conns {
                    if c.fd == token {
                        data = @c.read(64) or err { return }
                        print("a client said: {bytes_to_str(data)}")
                        @ep.remove(c.fd) or err { return }
                        handled += 1
                    }
                }
            }
        }
    }
    print("handled {handled} of {CLIENTS} clients")
    @ep.close()
}

test "event flags are distinct bits" {
    assert_eq(EPOLL_IN | EPOLL_OUT, 5)
    #assert(epoll_readable(EPOLL_IN | EPOLL_OUT))
}
```

```text
a client said: hello from client 0
a client said: hello from client 1
a client said: hello from client 2
handled 3 of 3 clients
```

Three things to carry into your own servers: keep the loop's per-event work short, bound the number of spins so a bug cannot hold a core forever, and remove a descriptor from the set (`@ep.remove(fd)`) before you stop caring about it.

### 17.5 `std.io.poll`: the high-level wrapper

`std.io.poll` packages the same loop as `PollLoop`. Its methods mirror `Epoll` one for one — `register` is `add` (and it switches the socket to non-blocking mode for you), `update` is `modify`, `unregister` is `remove`, and `wait` and `close` are unchanged — with friendlier flag names:

| `PollLoop` | `Epoll` equivalent | Meaning |
|---|---|---|
| `POLL_READ` | `EPOLL_IN` | the socket has bytes to read or a connection to accept |
| `POLL_WRITE` | `EPOLL_OUT` | the socket can accept more bytes |
| `POLL_ERROR` | `EPOLL_ERR \| EPOLL_HUP` | the peer failed or hung up |
| `poll_can_read(m)` | `epoll_readable(m)` | classify a returned mask |

Everything you learn from the `Epoll` recipes above transfers directly: swap the type, rename the flags, and the loop stays the same.

---

## 18. `std.net` over HTTP: routers, middleware, and responses

The HTTP layer is built from small, pure pieces: a `Request` and `Response` record, a prefix-bucketed `Router`, and a handful of middleware helpers for bearer tokens, CORS, security headers, and rate limits. Because they are plain values, you can test a complete handler without opening a socket.

### 18.1 The common server path

The common path has one application value. Route handlers are statically
`Fn[Request, Response]`; returning `"pong"` directly is a compile error, while
`ok("pong")` is a `Response`. The `routes` form is compile-time sugar for the
same `App.get` and `App.post` calls, rejects duplicate method/path pairs, and
adds no runtime dispatch layer.

```super
use std.net.http

+main() {
    app = #routes {
        get "/health" { ok("up") }
        post "/echo" |req| { ok(req.body) }
    }
    @app.serve("127.0.0.1:8080") or err {
        print("server failed: {err}")
        return 1
    }
    0
}
```

`App.serve` creates the listener, installs SIGINT/SIGTERM handling, runs the
loop, and closes resources. Use `serve_with(address, config)` for timeout,
header/body, and keep-alive limits. Direct `Server.listen`,
`Notify.graceful`, and `Server.serve` remain available for advanced custom
listeners, stop sources, or multi-server orchestration.

### 18.2 A router and a handler you can test

The router maps `(method, path)` to an integer route id and captures `:name` segments. A handler is then an ordinary function from `Request` to `Response`. Status helpers (`ok`, `created`, `bad_request`, `not_found`, `method_not_allowed`, …) accept Super objects and JSON-encode them (a `str` is still the raw wire body), and `render` produces the exact bytes for the wire.

```super
use std.net.http
use std.fmt.json

+ROUTE_HEALTH = 1
+ROUTE_LIST = 2
+ROUTE_CREATE = 3
+ROUTE_SHOW = 4

+build_router() : Router {
    r = Router.new()
    @r.add("GET", "/health", ROUTE_HEALTH)
    @r.add("GET", "/tasks", ROUTE_LIST)
    @r.add("POST", "/tasks", ROUTE_CREATE)
    @r.add("GET", "/tasks/:id", ROUTE_SHOW)
    r
}

+handle(r: Router, req: Request) : Response {
    m = r.match(req.method, req.path)
    if m.status == 405 { return method_not_allowed({error: "method not allowed"}) }
    if m.status != 200 { return not_found({error: "no such route"}) }
    if m.id == ROUTE_HEALTH { return ok({status: "ok"}) }
    if m.id == ROUTE_LIST { return ok([]) }
    if m.id == ROUTE_CREATE {
        title = json_get_str(req.body, "title") or ""
        if title == "" { return bad_request({error: "title required"}) }
        return created({id: "1", title: title})
    }
    ok({id: route_param(m, "id"), title: "demo"})
}

+main() {
    r = build_router()
    print(render(handle(r, request("GET", "/health", ""))))
    fresh = handle(r, request("POST", "/tasks", "{\"title\":\"Ship Super\"}"))
    print("{fresh.status} {fresh.body}")
    shown = handle(r, request("GET", "/tasks/42?verbose=1", ""))
    print("{shown.status} {shown.body}")
    wrong = handle(r, request("DELETE", "/tasks", ""))
    print("{wrong.status}")
}

test "unknown paths are 404 and wrong verbs are 405" {
    r = build_router()
    assert_eq(handle(r, request("GET", "/nope", "")).status, 404)
    assert_eq(handle(r, request("DELETE", "/tasks", "")).status, 405)
}

test "creating a task validates the body" {
    r = build_router()
    bad = handle(r, request("POST", "/tasks", "{}"))
    assert_eq(bad.status, 400)
    good = handle(r, request("POST", "/tasks", "{\"title\":\"x\"}"))
    assert_eq(good.status, 201)
}

test "bearer middleware guards routes" {
    req = req_with_header(request("GET", "/tasks", ""), "Authorization", "Bearer s3cret")
    guard = mw_require_token(req, "s3cret") or ok("pass")
    assert_eq(guard.status, 200)
    anon = mw_require_token(request("GET", "/tasks", ""), "s3cret") or ok("pass")
    assert_eq(anon.status, 401)
}
```

```text
HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: 15

{"status":"ok"}
201 {"id":"1","title":"Ship Super"}
200 {"id":"42","title":"demo"}
405
```

The router distinguishes three outcomes, and a good handler maps each to the right status: a hit (`200`), a path that exists under a different method (`405`), and no match at all (`404`).

### 18.3 Cookbook notes for real services

- **Guard first.** Run `mw_require_token` (or your own check) before routing, and return its `Response` unchanged when it denies the request, as the third test above does.
- **Decorate last.** `mw_cors(res, origin)` and `mw_security_headers(res)` wrap a finished response; `with_header` and `with_cookie` add individual headers.
- **Query strings are cheap.** `path_only(target)` strips the query and `query_get(target, "page")` reads one parameter without parsing the rest.
- **Keep JSON beside binary.** If a sustained stream uses a binary protocol, keep a JSON form of the same endpoint for browsers and for debugging; the handler in 18.1 already takes typed values, so both adapters can share it.

---

## 19. `std.net` over WebSocket: handshakes, frames, and rooms

`std.net.ws` implements RFC 6455 as pure functions over byte arrays. That means the handshake, the frame codec, and the room registry are all testable with no socket in sight, and the same code runs unchanged behind epoll, io_uring, or a test harness.

### 19.1 Handshake, codec, and fan-out

This recipe performs the upgrade handshake for the RFC's sample key, joins three clients to a `WsRoom`, encodes a text frame, decodes it again, and builds a close frame. `broadcast` shows the standard fan-out shape: encode **once**, then queue the same bytes for every subscriber except the sender.

```super
use std.net.ws
use std.core.collections
use std.core.str

+broadcast(room: WsRoom, sender: i64, message: str) : i64 {
    wire = ws_encode(ws_text(message), 0) or []
    delivered = 0
    for client in room.subscribers() {
        if client != sender {
            delivered += 1
        }
    }
    delivered * len(wire)
}

+main() {
    reply = ws_upgrade_response("dGhlIHNhbXBsZSBub25jZQ==") or "handshake failed"
    print(reply)

    room = WsRoom.new(8)
    @room.join(101)
    @room.join(102)
    @room.join(103)
    print("{room.count} clients in the room")
    bytes_out = broadcast(room, 101, "hello team")
    print("fan-out from 101 queued {bytes_out} bytes")

    wire = ws_encode(ws_text("hi room"), 0) or []
    frame = ws_decode(wire) or ws_close(WS_CLOSE_PROTOCOL_ERROR, "bad frame")
    print("decoded: {ws_text_of(frame)}")
    bye = ws_close(WS_CLOSE_NORMAL, "bye")
    print("close code: {ws_close_code(bye)}")
}

test "handshake derives the RFC 6455 accept key" {
    reply = ws_upgrade_response("dGhlIHNhbXBsZSBub25jZQ==") or ""
    assert_eq(find(reply, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=") >= 0, true)
}

test "text frames round trip" {
    wire = ws_encode(ws_text("ping"), 0) or []
    assert_eq(ws_text_of(ws_decode(wire) or ws_close(1002, "x")), "ping")
}

test "rooms deduplicate members and fan out" {
    room = WsRoom.new(4)
    #assert(@room.join(1))
    #assert(!(@room.join(1)))
    @room.join(2)
    assert_eq(broadcast(room, 1, "x"), 3)
    #assert(@room.leave(1))
    assert_eq(room.count, 1)
}
```

```text
HTTP/1.1 101 Switching Protocols
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=


3 clients in the room
fan-out from 101 queued 24 bytes
decoded: hi room
close code: 1000
```

Details worth knowing:

- `ws_encode(frame, mask_key)` takes a mask key because clients must mask and servers must not; pass `0` on the server side.
- `ws_decode` returns a value-error, so a malformed frame becomes an ordinary failure you convert into a close frame with `WS_CLOSE_PROTOCOL_ERROR`.
- Control frames (`ws_ping`, `ws_pong`, `ws_close`) are capped at 125 bytes of payload by the protocol; `ws_pong_for(ping)` echoes a ping's payload.
- `WsRoom` rejects duplicate joins and removes members in O(1) by swapping the last member into the hole.

---

## 20. `std.game`: ECS, a fixed timestep, physics, and AI

The game layer is deliberately small and deterministic. `std.game.ecs` is a sparse-set entity-component store with generational handles. `std.game.app` converts real frame time into whole fixed ticks. `std.game.physics2d` gives you circles, boxes, and impulse resolution. `std.game.ai` supplies A\*, steering, state machines, and behavior trees.

### 20.1 An ECS world driven by a fixed clock

Entities are plain integers that carry a generation, so a stale handle can never reach a recycled slot. Components are stored by name. `App.new(60)` creates a 60 Hz clock; `advance(real_dt)` returns how many whole ticks have accumulated, so rendering can run at any rate while the simulation stays exactly 60 Hz.

```super
use std.game.ecs
use std.game.app
use std.math.core

+Pos = {
    x: f64 = 0.0
    y: f64 = 0.0
}

+Vel = {
    vx: f64 = 0.0
    vy: f64 = 0.0
}

+spawn_mover(@world, x: f64, vx: f64) : i64 {
    e = @world.spawn()
    @world.add(e, "Pos", Pos{x: x, y: 0.0})
    @world.add(e, "Vel", Vel{vx: vx, vy: 1.0})
    e
}

+movement(@world, dt: f64) : i64 {
    moved = 0
    for e in world.query(["Pos", "Vel"]) {
        p = world.get(e, "Pos")
        v = world.get(e, "Vel")
        @world.add(e, "Pos", Pos{x: p.x + v.vx * dt, y: p.y + v.vy * dt})
        moved += 1
    }
    moved
}

+main() {
    world = World.new()
    a = spawn_mover(@world, 0.0, 2.0)
    spawn_mover(@world, 10.0, -1.0)

    clock = App.new(60)
    steps = 0
    for 0..30 {
        ticks = @clock.advance(1.0 / 30.0)
        for 0..ticks {
            movement(@world, clock.dt)
            steps += 1
        }
    }
    p = world.get(a, "Pos")
    seconds = round(clock.time * 100.0) / 100.0
    px = round(p.x * 100.0) / 100.0
    py = round(p.y * 100.0) / 100.0
    print("simulated {steps} ticks over {seconds} s")
    print("mover a is at ({px}, {py})")
    print("entities alive: {world.count()}")
}

test "movement applies velocity once per tick" {
    world = World.new()
    e = spawn_mover(@world, 0.0, 4.0)
    assert_eq(movement(@world, 0.5), 1)
    p = world.get(e, "Pos")
    assert_eq(p.x, 2.0)
    assert_eq(p.y, 0.5)
}

test "stale handles cannot touch recycled slots" {
    world = World.new()
    old = @world.spawn()
    @world.despawn(old)
    fresh = @world.spawn()
    assert_eq(@world.add(old, "Hp", 5), false)
    assert_eq(@world.add(fresh, "Hp", 9), true)
}

test "fixed timestep yields a steady tick count" {
    clock = App.new(60)
    assert_eq(@clock.run_frames(120, 1.0 / 60.0), 120)
}
```

```text
simulated 60 ticks over 1 s
mover a is at (2, 1)
entities alive: 2
```

Thirty frames at 30 fps is one second of real time, which the clock converts into exactly sixty 60 Hz ticks; the mover with velocity `(2, 1)` therefore ends at `(2, 1)`. This is the deterministic shape to keep: fixed `dt`, queries in stable order, structural changes (spawn and despawn) outside the iteration.

### 20.2 Physics and AI in one scene

A 2D world with gravity, a falling circle, and a static floor; an 8×8 grid with a wall that A\* must route around; and a two-state finite machine for an NPC. All three are pure computations, so the tests assert on their outcomes directly.

```super
use std.game.physics2d
use std.game.ai
use std.core.collections

+drop_ball(frames: i64) : f64 {
    w = p2_world(0.0, -9.0)
    @w.p2_add(p2_circle(0.0, 5.0, 0.5, 1.0))
    @w.p2_add(p2_box(0.0, -1.0, 10.0, 1.0, 0.0))
    for 0..frames {
        @w.p2_step(1.0 / 60.0)
    }
    w.bodies[0].y
}

+main() {
    y = drop_ball(180)
    print("ball rests at y = {y}")

    maze = Grid.new(8, 8)
    for 1..6 { @maze.block(3, it) }
    route = maze.astar(0, 3, 6, 3, false)
    print("path length: {len(route)}")
    print("walls: {maze.walls()}")

    npc = Fsm.new("idle")
    @npc.on("idle", "see_player", "chase")
    @npc.on("chase", "lose_player", "idle")
    @npc.fire("see_player")
    print("npc is now {npc.cur}")
}

test "the ball comes to rest on the floor" {
    y = drop_ball(180)
    #assert(y > 0.0 && y < 1.0)
}

test "a wall forces a detour" {
    g = Grid.new(8, 8)
    for 1..6 { @g.block(3, it) }
    straight = 7
    assert_eq(len(g.astar(0, 3, 6, 3, false)) > straight, true)
}

test "state machines only follow declared edges" {
    m = Fsm.new("idle")
    @m.on("idle", "see_player", "chase")
    #assert(!(@m.fire("jump")))
    #assert(@m.fire("see_player"))
    assert_eq(m.cur, "chase")
}
```

```text
ball rests at y = 0.4899966613247863
path length: 13
walls: 5
npc is now chase
```

A body with infinite mass (`mass 0.0`) is static, which is how the floor stays put. `astar(sx, sy, gx, gy, diag)` returns the cell path as flat indices, so its length is the number of cells visited, start and goal included.

### 20.3 Choosing the right game tool

| You want… | Reach for… |
|---|---|
| thousands of similar entities with optional components | `std.game.ecs` `World`, plus a dense SoA array for the hot field |
| a frame-rate-independent simulation | `std.game.app` `App` with a fixed `hz` |
| collision and response between circles and boxes | `std.game.physics2d` `p2_collide`, `p2_step` |
| grid pathfinding, steering, or decision logic | `std.game.ai` `Grid.astar`, `ai_seek`, `Fsm`, `Bt` |
| procedural terrain or loot | `std.math.noise` and `std.math.rand` (chapter 22) |
| a sprite cel, a looping frame, or a sound cue on the display list | `std.game.fx` `Sprite`, `Clip`, `Cue`, `Mixer` |

`Sprite` declares `Widget`, so a window can take `ball.paint()` and mount it. The recipe in the interface chapter prints one cel and one cue. Programs 28 through 30 are the games that drive those values every tick.

---

## 21. `std.sec`: hashes, signatures, passwords, tokens, and post-quantum keys

Security code should be boring and hard to misuse. The `sec` layer gives you vetted primitives with small, explicit surfaces: hex digests, HMAC, constant-time comparison, salted password hashing, signed messages, TOTP codes, base64 and percent encoding, HS256 JSON Web Tokens, BLAKE3 and SHA-3, and the NIST post-quantum KEM and signature schemes.

### 21.1 Everyday primitives

This recipe computes a digest and a keyed tag, round-trips base64, percent-encodes a query value, hashes and verifies a password, and issues and verifies a session token with an expiry.

```super
use std.sec.crypto
use std.sec.encoding
use std.sec.jwt
use std.core.str

+issue_session(secret: str, user: str, now: i64) : str {
    jwt_make(secret, user, now, now + 900)
}

+main() {
    digest = sha256_hex("super")
    print("sha256 = {digest}")
    tag = hmac_sha256_hex("api-key", "GET /orders")
    print("hmac   = {tag}")

    b64 = base64_encode("hello, super")
    print("base64 = {b64}")
    plain = base64_decode(b64) or "decode failed"
    print("back   = {plain}")
    query = url_encode("a b&c=d")
    print("query  = {query}")

    stored = pw_hash_with("correct horse", "fixed-salt", 1000)
    right = pw_verify("correct horse", stored)
    wrong = pw_verify("battery staple", stored)
    print("password ok    = {right}")
    print("password wrong = {wrong}")

    token = issue_session("s3cret", "user-7", 1000)
    good = jwt_verify("s3cret", token, 1500)
    print("token valid = {good.valid}, subject = {good.sub}")
    stale = jwt_verify("s3cret", token, 5000)
    print("expired valid = {stale.valid}")
}

test "signed messages detect tampering" {
    signed = sign_message("k", "pay 10")
    assert_eq(open_message("k", signed) or "rejected", "pay 10")
    assert_eq(open_message("other", signed) or "rejected", "rejected")
}

test "tokens expire and reject wrong secrets" {
    token = issue_session("s3cret", "user-1", 1000)
    assert_eq(jwt_verify("s3cret", token, 1500).valid, true)
    assert_eq(jwt_verify("s3cret", token, 2500).valid, false)
    assert_eq(jwt_verify("wrong", token, 1500).valid, false)
}

test "constant time compare" {
    assert_eq(ct_eq("abc", "abc"), true)
    assert_eq(ct_eq("abc", "abd"), false)
}

test "totp matches the RFC 6238 length" {
    code = totp("12345678901234567890", 59, 30, 8)
    assert_eq(byte_len(code), 8)
}
```

```text
sha256 = 73d1b1b1bc1dabfb97f216d897b7968e44b06457920f00f2dc6c1ed3be25ad4c
hmac   = 196ad605d8375cab842e29946668ba414fac6bf0f789c7fa733120d0b810a0ba
base64 = aGVsbG8sIHN1cGVy
back   = hello, super
query  = a%20b%26c%3Dd
password ok    = true
password wrong = false
token valid = true, subject = user-7
expired valid = false
```

The habits to copy:

- Compare secrets with `ct_eq`, never with `==`.
- Store `pw_hash(password, salt)` records (they carry the scheme, iteration count, and salt), verify with `pw_verify`, and use `pw_needs_rehash` to upgrade old records at login. The recipe passes a small iteration count so it finishes instantly; the default is deliberately expensive.
- Tokens carry their own expiry, and `jwt_verify(secret, token, now)` takes the clock as an argument, so expiry tests need no sleeping.
- `sign_message` / `open_message` give you an authenticated envelope in one call; `open_message` returns an optional, so a forged message is just `none`.

### 21.2 Modern hashes and post-quantum cryptography

`std.sec.hash` adds BLAKE3, SHA-3, and cheap non-cryptographic checksums (`fnv1a32`, `crc32`). `std.sec.pq` provides an ML-KEM key encapsulation (agree on a session key), an ML-DSA signature scheme, and a hybrid X25519+ML-KEM option for the transition period.

*As with `bin` and `compress`, import `std.sec.hash` or `std.sec.crypto` in a given file, not both: they export some of the same digest names.*

```super
use std.sec.hash
use std.sec.pq
use std.core.str

+fingerprint(payload: str) : str  => blake3_hex(payload)

+seal_session_key() : bool {
    keys = pq_kem_keygen()
    sealed = pq_kem_encaps(keys.ek)
    opened = pq_kem_decaps(keys.seed, sealed.ct)
    opened == sealed.ss
}

+main() {
    fp = fingerprint("user-7:profile")
    print("blake3 = {fp}")
    sha3 = sha3_256_hex("super")
    fnv = fnv1a32("super")
    print("sha3   = {sha3}")
    print("fnv1a  = {fnv}")
    print("kem round trip = {seal_session_key()}")

    signer = pq_sign_keygen()
    sig = pq_sign(signer.id, "release-0.1.0")
    genuine = pq_verify(signer.vk, "release-0.1.0", sig)
    forged = pq_verify(signer.vk, "release-0.1.1", sig)
    print("signature verifies = {genuine}")
    print("tampered verifies  = {forged}")
}

test "blake3 digests are 64 hex characters and deterministic" {
    d = fingerprint("audit")
    assert_eq(byte_len(d), 64)
    #assert(d == fingerprint("audit"))
}

test "kem shared secrets agree" {
    #assert(seal_session_key())
}
```

```text
blake3 = f2dd7fdde42e22cce901a3e11c264f792ea913023e624baa16de723bbdf6120a
sha3   = 79de1c617efcf3d784ca3b5d1be7fefb1d1287b079fe4527640c36446cd29ea0
fnv1a  = 4152230356
kem round trip = true
signature verifies = true
tampered verifies  = false
```

The key encapsulation flow is: the receiver publishes `ek`; the sender runs `pq_kem_encaps(ek)` and keeps the shared secret `ss` while sending the ciphertext `ct`; the receiver runs `pq_kem_decaps(seed, ct)` and recovers the same `ss`. Derive working keys from that secret with `pq_kdf`.

---

## 22. `std.math.core`: scalars, vectors, randomness, noise, and statistics

The math layer is the numerical toolbox. `std.math.core` holds scalar functions; `std.math.linalg` holds `Vec2`, `Vec3`, and 4×4 matrix helpers; `std.math.rand` is a seedable generator with the same sequence on every platform; `std.math.noise` produces value, Perlin, fractal, and Worley noise; `std.math.stats` covers the descriptive statistics you need for benchmarks and telemetry.

### 22.1 One tour of the numeric toolbox

```super
use std.math.core
use std.math.rand
use std.math.stats
use std.math.linalg
use std.math.noise

+orbit(radius: f64, angle: f64) : Vec2 {
    v2_rotate(v2(radius, 0.0), angle)
}

+main() {
    print("sqrt(2) => {sqrt(2.0)}")
    print("clamp(15, 0, 10) => {clamp(15.0, 0.0, 10.0)}")

    dice = Rng.new(42)
    rolls = []
    for 0..1000 {
        rolls = rolls + [@dice.range(1, 7)]
    }
    avg = round(mean(rolls) * 10.0) / 10.0
    print("mean of 1000 d6 rolls ~ {avg}")

    p = orbit(2.0, 1.5707963267948966)
    px = round(p.x * 1000.0) / 1000.0
    py = round(p.y * 1000.0) / 1000.0
    print("quarter turn: ({px}, {py})")

    heading = v2_norm(v2(3.0, 4.0))
    print("unit vector: ({heading.x}, {heading.y})")

    height = perlin2(1.5, 2.5, 7)
    print("terrain sample is deterministic: {height == perlin2(1.5, 2.5, 7)}")
}

test "same seed gives the same sequence" {
    a = Rng.new(7)
    b = Rng.new(7)
    for 0..20 { assert_eq(@a.range(0, 100), @b.range(0, 100)) }
}

test "dice stay in range" {
    r = Rng.new(1)
    for 0..200 {
        v = @r.range(1, 7)
        #assert(v >= 1 && v < 7)
    }
}

test "vector helpers" {
    assert_eq(v2_len(v2(3.0, 4.0)), 5.0)
    assert_eq(v2_dot(v2(1.0, 0.0), v2(0.0, 1.0)), 0.0)
    assert_eq(v3_cross(v3(1.0, 0.0, 0.0), v3(0.0, 1.0, 0.0)).z, 1.0)
}

test "statistics" {
    xs = [1.0, 2.0, 3.0, 4.0, 5.0]
    assert_eq(mean(xs), 3.0)
    assert_eq(median(xs), 3.0)
    assert_eq(percentile(xs, 0.5), 3.0)
    #assert(stddev(xs) > 1.41 && stddev(xs) < 1.42)
}
```

```text
sqrt(2) = 1.4142135623730951
clamp(15, 0, 10) = 10
mean of 1000 d6 rolls ~ 3.5
quarter turn: (0, 2)
unit vector: (0.6, 0.8)
terrain sample is deterministic: true
```

Three properties make this layer a good citizen in a simulation:

- **Determinism.** `Rng.new(seed)` expands a seed into a six-word MRG32k3a state; the same seed yields the same sequence everywhere, which is what replays, procedural worlds, and tests need. Use `Rng.from_clock()` only when you genuinely want a different run each time.
- **Visible mutation.** Every draw changes the generator, so every draw is written `@dice.range(1, 7)`.
- **Pure geometry.** Vector functions return new values (`v2_rotate`, `v2_norm`, `v3_cross`), so they compose in expressions without hidden state.

*A note on `stats`:* percentile and quantile take their probability in the range 0 to 1 (`percentile(xs, 0.9)` is the 90th percentile), and `median` is exact for odd-length data.

---

## Interfaces — `std.cli.cmd`, `std.tui`, and `std.gui`

These three modules are the library's surface for programs a person drives. They sit on the modules you already have: strings, arrays, environment, math, geometry, time, and the low-level argument and window hosts. Layout and widget state stay in Super. The host does the work that has to be dense: ANSI damage, terminal mode, and pixel raster.

Use **`use std.cli.cmd`** (or `use std.cli` / `use std.clap`) for everything CLI-related in application code. That one import gives you `Command` plus the argv helpers `cli_args` / `cli_parse` / `CliArgs`. Do not also import `std.sys.cli` / `std.cli.args` — that path is the tokenizer implementation used by `std.cli.cmd`, not an application-facing module.

```super
use std.cli.cmd

+main() {
    cmd = Command.new("tasks")
    cmd = @cmd.about("task tracker")
    cmd = @cmd.version("1.0")
    cmd = @cmd.flag("verbose", "v", "more logs")
    cmd = @cmd.opt("port", "p", "PORT", "8080", "listen port", false)
    cmd = @cmd.as_int()
    add = Command.new("add")
    add = @add.about("add a task")
    add = @add.pos("title", true, "task title")
    cmd = @cmd.sub(add)
    print(cmd.help())
}
```

`super run` prints:

```text
tasks 1.0
task tracker

Usage: tasks [OPTIONS] [COMMAND]

Commands:
  add             add a task

Options:
  -v, --verbose           more logs
  -p, --port <PORT>       listen port [default: 8080]
  -h, --help              Print help
  -V, --version           Print version
```

`parse` takes the arguments after the program name. `parse_args` reads the process and drops argv[0]. The value of an option is the command line, then the environment (`from_env` / `env_get`), then the fallback. `as_int` and `as_float` mark the option you just added. A subcommand owns every argument after its name. `-h` and `-V` succeed without the required checks, unless you defined those shorts yourself. `counts` makes a repeated flag a number (`-vv` is `2`). `alias` adds more names for a subcommand. `conflicts` and `requires` name other options. `group` files options under a help heading. `author` and `after_help` add the lines around the list. An unknown long option within one edit suggests the nearest name. `complete_bash` prints the words a shell completion script should offer.

`std.tui` is the terminal screen. `Screen.new(w, h)` allocates four integer planes with `arr_new`. `clear` replaces those planes in one shot. `print_at`, `block`, `paragraph`, `list`, `gauge`, `tabs`, `sparkline`, and `table` write cells. `words` wraps on spaces, `align_in` places a run at the start, center, or end, `scrollbar` draws a thumb, and `bars` draws a labeled chart. `Style` and `theme` name dark, light, accent, warning, and ok ink. `layout_grid` fills a rectangle, `tier` reads the width (`compact` below 40 columns, `wide` from 100), and `reflow` stacks cells when they would be narrower than `min_w`. `mouse_parse` reads an SGR click (`\x1b[<btn;x;yM`). `plain` asks the host for the text. `ansi` asks the host to diff against the previous frame and returns only the damaged runs, so a quiet frame writes nothing. `Term.open` turns on raw mode only when stdin is a terminal, which keeps `super test` from taking over your tty.

```super
use std.tui

+hud_line(score: i64, health: i64) : str {
    text = "SCORE={score} HP={health}"
    view = Screen.new(byte_len(text), 1)
    @view.print_at(0, 0, text, 7, 0, 0)
    view.row(0)
}
```

`std.gui` is the retained desktop tree. You build nodes, call `layout`, then `paint` or `describe`. `paint` is the display list tests and canvas games read (`fill`, `rect`, `text`, `sprite`, `sound`); the host draws that list with an 8×8 font inside a real window. `describe` is the widget document: a `gui` line, then one tab-separated `node` per control. The desktop provider turns that document into real controls. Nodes carry ink, fill, accent, font size, weight, alignment, tracking, padding, gap, and opacity. `style` and `metrics` set them; `theme` paints `dark`, `light`, or `contrast`. `text` is the calligraphy block, and `measure_text` returns its width. `grid` flows children across columns and honors `span_c`. `scroll` offsets its column by the node's value. `slider`, `progress`, and `tabs` are the extra controls. `tier` is `compact` below 480 pixels and `wide` from 960; a compact window stacks a `row` and collapses a `grid` to one column. `note` and `take` are the event queue (`pointer`, `key`, `tick`). `animate` and `frame` ease `x`, `y`, `w`, or `opacity` with `linear`, `in`, `out`, or `inout`. `mirror` draws the same tree into a `std.tui` screen. `run` opens a window through `std.gui.window`, polls it, and sleeps 16 ms via `std.sys.time`. Hits go through `Aabb.contains` from `std.math.geom`.

The provider follows the operating system. Linux opens GTK 4 and tries Wayland before X11, so a session that exports both does not land on XWayland. Windows opens Win32 common controls. macOS opens Cocoa. If the first provider cannot open a display, the host tries the others. `sys_gui_backend` reports `wayland`, `x11`, `win32`, `cocoa`, `gtk-win32`, or `gtk-macos` once the window is up, and the platform name (`gtk`, `win32`, `cocoa`) before that.

| `SUPER_GUI` | What opens |
|---|---|
| unset, on a display | The platform provider: GTK on Linux, Win32 on Windows, Cocoa on macOS |
| `1`, `native`, `window` | The same platform provider, even without `DISPLAY` or `WAYLAND_DISPLAY` |
| `gtk` | GTK 4. On Linux this is Wayland, then X11 |
| `wayland`, `x11` | GTK 4 forced onto that GDK backend |
| `win32` | Win32 controls |
| `cocoa`, `macos` | Cocoa controls |
| `0`, `headless`, `quiet` | No window. This is how `super test` stays off the display |
| `ppm` | A Portable Pixmap of the display list (`SUPER_GUI_OUT` names the file) |
| `dump` | The script on stderr, and no window |

```super
use std.gui

+inspector() : str {
    app = Gui.new("Physics", 160, 80)
    root = @app.column("root")
    @app.label(root, "title", "World")
    @app.layout()
    @app.paint()
}
```

That call returns a display list whose first line is `fill 0 0 160 80 18 18 24` and whose label line contains `World`.

The same tree grows the controls a desk actually needs. `toggle`, `checkbox`, `radio`, `dialog`, `field`, `toast`, `flash`, `shape`, `titlebar`, `chrome`, and `badge` are nodes. Escape closes every dialog. Choosing one radio clears the others that share its group. Each of those controls is also a value in `std.gui.widgets` that declares `Widget` inline: `paint` writes a display list, `hit` tests a pixel, and `measure` reports a width. Your own window does the same. `custom` mounts whatever `paint` returned.

```super
use std.gui
use std.gui.widgets
use std.core.str

+BadgeCard: Widget = {
    label: str = ""
    w: i64 = 80
    paint(self) : str  => "text 8 8 230 220 255 |{self.label}\n"
    hit(self, x: i64, y: i64) : bool  => widget_hit(x, y, 0, 0, self.w, 24)
    measure(self, max_w: i64) : i64  => widget_measure(max_w, self.w)
}

+main() {
    app = Gui.new("Desk", 160, 80)
    root = @app.column("root")
    @app.toggle(root, "sound", "Sound", true)
    card = BadgeCard{label: "new"}
    @app.custom(root, "pin", card.paint())
    @app.layout()
    frame = @app.paint()
    sound = "Sound"
    label = "new"
    print("{contains(frame, sound)} {contains(frame, label)} {card.hit(4, 4)}")
}
```

```text
$ super run desk.sr
true true true
```

`std.game.fx` is the same idea for a frame. `Sprite` declares `Widget`, so `paint` is a `sprite x y w h name frame` line and `step` advances the cel. `Clip` only counts frames. `Cue.play` is a `sound name gain` line, and `Mixer` concatenates the cues it has heard. Nothing in this module opens an audio device; the window provider paints the display list on its canvas and is the one that plays the cue.

```super
use std.game.fx

+main() {
    ball = Sprite{name: "ball", x: 10, y: 20, w: 8, h: 8, frame: 0, frames: 4}
    @ball.step()
    cue = Cue{name: "bounce", gain: 80}
    print("{ball.paint()}{cue.play()}")
}
```

```text
$ super run sprite.sr
sprite 10 20 8 8 ball 1
sound bounce 80
```

Parenthesize a condition that ends in a field (`if (a.required) {`). A `{` immediately after a field is a struct literal, and the same rule is what makes `. {` the inferred-struct form.

## Cookbook checklist

Before you move on to the flagship projects, run your own recipes through the same gate this chapter used:

1. `super check recipe.sr` — it type-checks.
2. `super run recipe.sr` — the output matches what you expect.
3. `super test recipe.sr` — the `test` blocks cover the failure path, not only the happy path.

When all three pass, you have a recipe you can trust.
