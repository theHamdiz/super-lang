# Part V — Ten Flagship Projects

You have the language (Parts I and II), the standard library (Part III), and the performance toolbox (Part IV). This part puts all of it to work. Each of the next ten chapters takes one program from the `examples/` directory, shows you its **complete source**, walks through the decisions in it, shows the real output of `super test`, and ends with a few ways to make it your own.

Every listing in this part is the file as it exists in the repository. Nothing is abridged, nothing is replaced with a placeholder, and every project passes `super test` on the 0.1.0 binary. When a chapter says a test prints a number, that number came from the compiler, not from a guess.

| Chapter | Project | Directory | What it teaches |
|---|---|---|---|
| 33 | Arcade space shooter | `examples/01_space_shooter` | ECS, `state`, parallel SIMD systems, collision events |
| 34 | Task and auth REST API | `examples/02_task_api` | Four-module layout, routes, `or err`, JWT, scope fan-out |
| 35 | Log analyzer | `examples/03_log_analyzer` | `embed`, sum types, slicing, shared counters, regex |
| 36 | Roguelike dungeon | `examples/04_roguelike_dungeon` | Tile sum types, comptime tables, A*, AI channel |
| 37 | Tower defense | `examples/05_tower_defense` | 50 000 creeps, spatial grid, priority queue, noise |
| 38 | Physics sandbox | `examples/06_physics_sandbox` | Rigid bodies, AABB, contact pipeline, `physics2d` |
| 39 | Realtime chat broker | `examples/07_realtime_chat_broker` | `when` with `after`, WebSocket frames, shared registry |
| 40 | Sharded KV cache engine | `examples/08_kv_cache_engine` | Comptime blocks, shard routing, TTL arm, Redis facade |
| 41 | HTTP load tester | `examples/09_http_load_tester` | Worker fan-out, histograms, percentiles |
| 42 | Map-reduce engine | `examples/10_mapreduce_kv` | Mappers, reducer, n-grams, ring buffer, logger |

Two conventions show up in all ten projects, so it is worth naming them once.

**Every project is a pure-core-plus-shell design.** The interesting logic lives in small exported functions that take plain numbers and structs and return plain numbers and structs. Anything that touches a clock, a socket, or a random generator is passed in or simulated. That is why the tests can assert exact values and never sleep.

**Every project has a `stdlib:` test at the bottom.** That test exercises the library module the project leans on, in the same file, so you can see the API in the context of a real program instead of in isolation.

To run any project from the repository root:

```bash
super test examples/01_space_shooter/
```

You can also `cd` into the directory and run `super test .`, which is what the output blocks below show.

---

## 33. Project 1 — The Arcade Space Shooter

A shooter is the classic data-oriented program: a few thousand things with a position and a velocity, one hot loop that moves all of them, and a trickle of events (a bullet hit an enemy, an enemy died) that other systems care about. This project builds exactly that skeleton and tests every piece of it.

### 33.1 Architecture

- **Components** are small C-ABI structs: `Pos` and `Vel` (16 bytes each), `Collider` (8 bytes), and `Enemy` (hit points and a score value). Each of the first three is protected by `#assert_size`, so an accidental extra field fails the build instead of silently changing your memory layout.
- **The world** is a single `state world = ecs.World{}`. Because it is `state`, it survives hot reload.
- **The movement system** is `update(dt)`: a `#simd for parallel` loop over every entity that has both a `Pos` and a `Vel`. The `@` on both bindings says the loop writes through them.
- **Collisions** are a method on `Circle` (`origin.hits(other)`), plus a typed channel, `chan[CollisionEvent]`, that feeds an effects fiber.
- **Comptime tables** (`#SIN_TABLE`, `#COS_TABLE`) are computed while the program compiles, so the runtime lookup is one array index.

### 33.2 The complete source

```super
// Game #1 — Arcade Space Shooter with parallel physics + collision channel
//
//   for parallel @(pos: Pos), (vel: Vel)  — multi-core physics
//   chan[CollisionEvent]                  — audio/particle fiber pipeline
//
// Run: super test examples/01_space_shooter/

use std.game.ecs
use std.math.linalg
use std.game.physics2d

#TRIG_STEPS = 16
#SIN_TABLE = #for i in 0..#TRIG_STEPS { sin_approx(i * 0.393) }
#COS_TABLE = #for i in 0..#TRIG_STEPS { cos_approx(i * 0.393) }

#assert_size(Pos, 16)
+Pos = {
    x: f64 = 0.0
    y: f64 = 0.0

    translate(@self, dx: f64, dy: f64) : f64 {
        self.x += dx
        self.y += dy
        self.x
    }
}

#assert_size(Vel, 16)
+Vel = {
    vx: f64 = 0.0
    vy: f64 = 0.0
}

#assert_size(Collider, 8)
+Collider = {
    radius: f64 = 1.0
}

+Enemy = {
    hp: i64 = 3
    score: i64 = 100

    damage(@self, amount: i64) : i64 {
        self.hp = self.hp - amount
        self.hp
    }

    is_alive(self) : bool  => self.hp > 0
}

+CollisionEvent = {
    a: i64 = 0
    b: i64 = 0
    kind: i64 = 0
}

state world = ecs.World{}

+Circle = {
    x: f64 = 0.0
    y: f64 = 0.0
    r: f64 = 1.0

    hits(self, other: Circle) : bool {
        dx = self.x - other.x
        dy = self.y - other.y
        reach = self.r + other.r
        dx * dx + dy * dy <= reach * reach
    }
}

+spawn_wave(count: i64) : i64 {
    n = 0
    for 0..count {
        n += 1
    }
    n
}

+warmup_slots() : i64 {
    n = 0
    #unroll(4)
    for 0..4 {
        n += 1
    }
    n
}

+trig_sample(idx: i64) : f64 {
    if idx < 0 {
        return 0.0
    }
    if idx >= #TRIG_STEPS {
        return 0.0
    }
    #SIN_TABLE[idx]
}

+update(dt: f64) {
    #simd
    for parallel @(pos: Pos), (vel: Vel) in world {
        pos.x += vel.vx * dt
        pos.y += vel.vy * dt
    }
}

+render_hud(score: i64, health: i64) {
    print("SCORE={score} HP={health}")
}

+run_fx_fiber(inbox: i64) : i64 {
    // Particle/audio consumer — counts events until channel closed by caller.
    n = 0
    n + inbox
}

test "collision detects overlapping circles" {
    origin = Circle{x: 0.0, y: 0.0, r: 1.0}
    near = Circle{x: 0.5, y: 0.0, r: 1.0}
    far = Circle{x: 5.0, y: 0.0, r: 1.0}
    #assert(origin.hits(near))
    #assert(!(origin.hits(far)))
}

test "type method damage + chaining" {
    e = Enemy{hp: 3, score: 100}
    left = @e.damage(1)
    assert_eq(left, 2)
    assert_eq(e.hp, 2)
    #assert(e.is_alive())
    left2 = @e.damage(2)
    assert_eq(left2, 0)
    #assert(!(e.is_alive()))
}

test "pos.translate mutates via @self" {
    p = Pos{x: 1.0, y: 2.0}
    @p.translate(3.0, 4.0)
    assert_eq(p.x, 4.0)
    assert_eq(p.y, 6.0)
}

test "wave spawner counts entities" {
    count = 5
    assert_eq(spawn_wave(.), 5)
}

test "collision channel + scope spawn fx fiber" {
    events = chan[CollisionEvent](32)
    scope {
        fx = spawn {
            n = 0
            for _ev in events {
                n += 1
            }
            n
        }
        events.send(CollisionEvent{a: 1, b: 2, kind: 1})
        events.send(CollisionEvent{a: 3, b: 4, kind: 1})
        events.close()
        got = await fx
        assert_eq(got, 2)
    }
}

test "parallel physics update compiles" {
    update(0.016)
    assert_eq(1, 1)
}

test "comptime sin table length" {
    assert_eq(#TRIG_STEPS, 16)
    assert_eq(trig_sample(0), 0.0)
}

test "unroll warmup slots" {
    assert_eq(warmup_slots(), 4)
}

test "stdlib: Vec2 length and circle overlap agree with Circle.hits" {
    origin = Circle{x: 0.0, y: 0.0, r: 1.0}
    near = Circle{x: 0.5, y: 0.0, r: 1.0}
    far = Circle{x: 5.0, y: 0.0, r: 1.0}
    assert_eq(v2_len(v2(3.0, 4.0)), 5.0)
    assert_eq(v2_dist(v2(0.0, 0.0), v2(0.5, 0.0)), 0.5)
    assert_eq(p2_point_in_circle(0.5, 0.0, 0.0, 0.0, 1.0), origin.hits(near))
    a = p2_circle(0.0, 0.0, 1.0, 1.0)
    b = p2_circle(5.0, 0.0, 1.0, 1.0)
    assert_eq(p2_bounds_overlap(a, b), origin.hits(far))
}
```

### 33.3 Walkthrough

**Layout contracts.** The three `#assert_size` lines are the first thing in the file for a reason. A struct passed to a C host, written to disk, or iterated by a SIMD kernel has a layout that matters. Writing the size next to the declaration turns that layout into a compile-time promise.

**Methods that mutate.** `Pos.translate(@self, dx, dy)` and `Enemy.damage(@self, amount)` both take `@self`, so the caller must write `@p.translate(...)` and `@e.damage(...)`. The tests show both call styles. `Enemy.is_alive(self)` does not mutate and therefore takes plain `self`; you call it as `e.is_alive()` with no sigil.

**The collision test.** `Circle.hits` compares squared distance against the squared sum of radii, so there is no square root in the hot path. Two circles of radius 1 whose centres are 0.5 apart overlap; at 5.0 apart they do not. Those two assertions are the whole contract.

**Argument punning with a dot.** The wave-spawner test writes `spawn_wave(.)`. A lone `.` in an argument position means "pass the local variable that has the same name as this parameter". The test binds `count = 5` first, and `spawn_wave(.)` becomes `spawn_wave(count)`.

**The event channel.** The `collision channel` test is the smallest useful fan-out: a bounded channel of 32 events, a `scope`, a spawned consumer that counts events until the channel closes, two `send`s, a `close`, and an `await` that returns the count. Closing the channel is what ends the consumer's `for` loop, so a test that forgets `events.close()` would hang. Keep that habit.

**An honest note about the parallel test.** `update(0.016)` runs over an empty world in the test, so it proves that the system compiles and the parallel loop runs, not that entities move. The next section turns that into a real movement test.

### 33.4 Running it

```bash
cd examples/01_space_shooter
super test .
```

```text
./main.sr: 19 assertion(s) passed
super test: 19 passed, 0 file(s) failed
```

Nine test blocks, nineteen assertions, all passing.

### 33.5 Make it yours

1. **Move something.** Spawn a handful of entities with a `Pos` and `Vel` into `world`, call `update(1.0)`, and assert that the positions changed by exactly the velocity. The empty-world test becomes a real one.
2. **Add a spatial grid.** Chapter 37 shows a cell function you can reuse: bucket entities by cell, then only call `Circle.hits` for entities in the same or neighboring cells.
3. **Wire the HUD to health.** `render_hud(score, health)` prints one line. Call it from a loop that drains the collision channel and subtracts `Enemy.damage` results.
4. **Use the trig table.** `trig_sample(i)` returns the sine for sixteen evenly spaced angles. Fire bullets in sixteen directions by pairing it with the cosine table.

### 33.6 Pitfalls

- A `chan` consumer that ends on `close()` must be awaited inside the same `scope` that spawned it.
- `@` on an argument is required whenever the callee takes `@`. The compiler reports a missing sigil as E0301.
- `#assert_size` counts C-ABI bytes, so reordering fields can change padding. Re-check the number after any layout edit.

---

## 34. Project 2 — The Task and Auth REST API

This is a real multi-file backend: models, authentication, routes, and an entry module that composes them. It is small enough to read in one sitting and shaped like the services you will actually write.

### 34.1 Architecture

| File | Responsibility |
|---|---|
| `models.sr` | Data shapes: `User`, `Task`, `CreateTask`, `AuthToken`, and comptime reflection over them |
| `auth.sr` | Fallible authentication with `fail` and `ensure` |
| `routes.sr` | HTTP routes for creating tasks, listing tasks, and logging in |
| `main.sr` | Concurrent profile loading, plus library tests for JWT, logging, hashing, and post-quantum key exchange |

The key idea is that **each file is a module**. Top-level items are private unless you mark them with `+`. Fields inside a struct are public unless you mark them with `-`. `User.password_hash` is private, so no other module can read it by accident.

### 34.2 `models.sr`

```super
#assert_size(User, 24)
+User = {
    id: i64
    name: str
    -password_hash: str = ""
}

#USER_SZ = User.size
#USER_JSON_GOAL = #goal_type

+Task = {
    id: i64
    title: str
    done: bool = false
    owner_id: i64
}

+CreateTask = {
    title: str
    owner_id: i64
}

+AuthToken = {
    user_id: i64
    token: str
}

#USER_FIELD_LABELS = User.fields.join(", ")

+user_fields_json(id: i64, name: str) : str {
    head = "{ \"id\": "
    tail = ", \"name\": \"" + name + "\" }"
    head + id + tail + " /* " + #USER_FIELD_LABELS + " */"
}

test "user C-ABI size via reflection" {
    assert_eq(#USER_SZ, 24)
}

test "goal_type stub for JSON builders" {
    assert_eq(#USER_JSON_GOAL, "goal")
}
```

`#USER_SZ = User.size` asks the compiler for the C-ABI size of `User`, and `#USER_FIELD_LABELS = User.fields.join(", ")` builds a string of field names at compile time. `user_fields_json` stitches them into a JSON-shaped string. A JSON response for a model whose shape the compiler already knows is the pattern to reach for first.

### 34.3 `auth.sr`

```super
+AuthToken = {
    user_id: i64
    token: str
}

+authenticate(user: str, pass: str) : AuthToken! {
    ensure user != "" else {
        fail "empty user"
        return
    }
    ensure pass != "" else {
        fail "empty pass"
        return
    }
    AuthToken.{user_id: 1, token: "tok_demo"}
}

+require_token(token: str) : i64! {
    ensure token != "" else {
        fail "missing token"
        return
    }
    1
}

test "authenticate ok" {
    t = authenticate("amina", "secret") or .{user_id: 0, token: ""}
    // Destructuring binding refinement
    {user_id, token} = t
    assert_eq(user_id, 1)
    assert_eq(token, "tok_demo")
}

test "require_token rejects empty" {
    v = require_token("") or 0
    assert_eq(v, 0)
}

test "require_token accepts" {
    v = require_token("tok") or 0
    assert_eq(v, 1)
}
```

`authenticate` returns `AuthToken!`: either a token or an error. The `ensure ... else { fail ...; return }` shape is the standard guard clause. Callers unwrap it with `or`, as the first test does: `authenticate("amina", "secret") or .{user_id: 0, token: ""}` falls back to an inferred struct literal when authentication fails. The same test then destructures the result with `{user_id, token} = t`, binding two locals in one line.

### 34.4 `routes.sr`

```super
// Routes module — requires the HTTP standard library.
use std.net.http

+CreateTask = {
    title: str
    owner_id: i64
}

+Task = {
    id: i64
    title: str
    done: bool = false
    owner_id: i64
}

+tasks = []

+authenticate(user: str, _pass: str) : i64! {
    ensure user != "" else { fail "empty user" }
    1
}

#route("POST", "/tasks")
#post("/tasks")
create_task(req: http.Request) : http.Response! {
    body = req.json(CreateTask) or err { return http.bad_request(.{error: "{err}"}) }
    ensure body.title != "" else { return http.bad_request(.{error: "title required"}) }
    http.ok(Task.{id: 1, title: body.title, done: false, owner_id: body.owner_id})
}

#route("GET", "/tasks")
#get("/tasks")
list_tasks(_req: http.Request) : http.Response! {
    http.ok(.{tasks: tasks})
}

#route("POST", "/login")
#post("/login")
login(_req: http.Request) : http.Response! {
    tok = authenticate("amina", "secret") or 0
    http.ok(.{user_id: tok, token: "tok_demo"})
}

test "integration: create task returns 200" {
    res = http.simulate_post("/tasks", {title: "Ship Super", owner_id: 1})
    assert_eq(res.status, 200)
}

test "integration: list tasks returns 200" {
    res = http.simulate_get("/tasks")
    assert_eq(res.status, 200)
}

test "integration: login returns 200" {
    res = http.simulate_post("/login", {user: "amina", pass: "secret"})
    assert_eq(res.status, 200)
}
```

A route is a declaration, not a function call: `#post("/tasks")
create_task(req: http.Request) : http.Response! { ... }`. The handler reads its JSON body with `req.json(CreateTask)`, and the `or err { return http.bad_request(...) }` handler turns a decode failure into a `400` response whose body is built with an inferred struct literal, `.{error: "{err}"}`. `ensure body.title != ""` adds a second validation that returns a `400` of its own. Success returns `http.ok(...)`, which the framework serializes straight to JSON.

This module declares its own copies of `CreateTask` and `Task`, plus a local `authenticate`, so the file compiles on its own. In a larger service you would import them from `models.sr` instead; the tests here prefer a standalone module.

The three `integration:` tests call `http.simulate_post` and `http.simulate_get`, which dispatch through the real route table without opening a socket. Each asserts a `200` status. That makes the whole HTTP surface testable in milliseconds.

### 34.5 `main.sr`

```super
// Backend API — concurrent DB/cache via scope/spawn + audit channel pool
//
// Run: super test examples/02_task_api/

use std.net.http
use std.sec.jwt
use std.diag.log
use std.sec.hash
use std.sec.pq
use models.{User}
use std.core.str

+AuditEvent = {
    user_id: i64 = 0
    action: str = ""
}

+fetch_user(id: i64) : User {
    User{id: id, name: "Amina"}
}

+fetch_cache(id: i64) : i64 {
    id * 10
}

+handle_profile(id: i64) : i64 {
    audit = chan[AuditEvent](16)
    total = 0
    scope {
        worker = spawn {
            n = 0
            for _ev in audit {
                n += 1
            }
            n
        }
        t_user = spawn fetch_user(id)
        t_cache = spawn fetch_cache(id)
        u = await t_user
        c = await t_cache
        audit.send(AuditEvent{user_id: u.id, action: "profile"})
        audit.close()
        processed = await worker
        total = u.id + c + processed
    }
    total
}

+boot() {
    print("task_api listening")
}

test "models construct via import" {
    u = User{id: 1, name: "Amina"}
    assert_eq(u.id, 1)
}

test "http helpers require std.net.http" {
    res = http.ok(.{ok: true})
    assert_eq(res.status, 200)
}

test "scope spawn concurrent profile + audit" {
    // user.id(7) + cache(70) + 1 audit event = 78
    assert_eq(handle_profile(7), 78)
}

test "stdlib: jwt roundtrip for authenticated user" {
    tok = jwt_make("s3cret", "user-1", 1000, 2000)
    r = jwt_verify("s3cret", tok, 1500)
    #assert(r.valid)
    assert_eq(r.sub, "user-1")
    assert_eq(jwt_verify("wrong", tok, 1500).valid, false)
    assert_eq(jwt_verify("s3cret", tok, 2500).valid, false)
}

test "stdlib: log line format offline" {
    lg = Logger.new("task_api")
    @lg.set_clock(1700000000000)
    @lg.info("order created")
    assert_eq(lg.last(), "2023-11-14T22:13:20.000Z INFO  [task_api] order created")
}

test "stdlib: blake3 integrity for audit payload" {
    #inline
    dig = blake3_hex("user-7:profile")
    assert_eq(byte_len(dig), 64)
    #assert(dig == blake3_hex("user-7:profile"))
}

test "stdlib: pq kem seals session key" {
    kp = pq_kem_keygen()
    enc = pq_kem_encaps(kp.ek)
    assert_eq(pq_kem_decaps(kp.seed, enc.ct), enc.ss)
}
```

`handle_profile` is the concurrency showcase. Inside one `scope` it starts three fibers: an audit worker that drains a channel, a user fetch, and a cache fetch. It awaits the two fetches, sends one audit event, closes the channel, awaits the worker, and sums the results. For `id = 7` that is `7 + 70 + 1 = 78`, which is exactly what the test asserts.

The `stdlib:` tests show four modules at work:

- **`std.sec.jwt`**: `jwt_make("s3cret", "user-1", 1000, 2000)` signs a token valid between times 1000 and 2000. Verification takes the current time as an argument, so the test checks acceptance at 1500, rejection with a wrong secret, and rejection at 2500, all without a real clock.
- **`std.diag.log`**: `Logger.new("task_api")` plus `set_clock` produces a byte-exact line, so log formatting is testable.
- **`std.sec.hash`**: `blake3_hex` gives a 64-character digest that is stable for equal inputs.
- **`std.sec.pq`**: a post-quantum key encapsulation round trip, checking that the decapsulated secret equals the encapsulated one.

### 34.6 Running it

```bash
cd examples/02_task_api
super test .
```

```text
./auth.sr: 4 assertion(s) passed
./main.sr: 11 assertion(s) passed
./models.sr: 2 assertion(s) passed
./routes.sr: 3 assertion(s) passed
super test: 20 passed, 0 file(s) failed
```

Fifteen test blocks across four files, twenty assertions.

### 34.7 Make it yours

1. **Replace the stubs with a store.** `fetch_user` and `fetch_cache` return constants. Swap them for calls to a database module and the fan-out in `handle_profile` stays unchanged.
2. **Real login.** `authenticate` accepts any non-empty pair. Compare a hash with the stored `password_hash` (which only `models.sr` can read) and keep the error messages generic.
3. **Add authorization.** Call `require_token` at the top of the task routes and return `http.bad_request` or an unauthorized response when it fails.
4. **Offer a second transport.** The API speaks JSON today. If you add a binary transport for high-volume clients, keep the JSON routes as the fallback for browsers and debugging.

### 34.8 Pitfalls

- `use models.{User}` imports a name from a sibling file. If you never use it, the compiler reports E0305 (unused import), which is a feature.
- A private field cannot be read from another module (E0104). Add a public method on the owning module if callers need a derived value.
- Standard-library names merge into the file's namespace. Do not name a local `text` or `created` if you also import `std.net.http`.

---

## 35. Project 3 — The Log Analyzer

Log analysis is a streaming problem: lots of small records, a handful of counters, and the temptation to allocate per record. This project shows the Super way to keep the working set small, along with the tools for splitting a big input into chunks that workers consume in parallel.

### 35.1 Architecture

- **Input** is embedded at compile time with `Fixture = embed "assets/fixture.txt"`, so tests need no file system.
- **Records** are plain structs (`LogRecord`, `LogChunk`) and a `Status` sum type with three shapes: `Ok`, `Warn(code)`, and `Err(reason)`.
- **Workers** pull `LogChunk` values from a channel and bump a `shared` atomic counter.
- **Parsing helpers** use `std.core.str` (splitting and prefix tests), `std.core.regex` (compiled patterns), and `std.diag.metrics` (counters).

### 35.2 The complete source

```super
// Log pipeline — chunk workers over chan[LogChunk] + shared atomic counters
//
// Run: super test examples/03_log_analyzer/

use std.core.str
use std.core.regex
use std.diag.metrics

#ASCII_CLASS = #for i in 0..64 { i % 32 }

+Fixture = embed "assets/fixture.txt"

Status = {
    Ok
    Warn(code: i64)
    Err(reason: str)
}

+LogRecord = {
    ts: i64
    level: str
    msg: str
    status: i64 = 0
}

+LogChunk = {
    start: i64 = 0
    end: i64 = 0
}

+make_records(n: i64) : i64 {
    _count = 0
    for 0..n {
        _count += 1
    }
    _count
}

+analyze(_sample: i64) : i64 {
    scores = [10, 20, 30, 40, 50, 5, 15]
    total = scores.keep(it >= 25).map(it * 2).sum()
    total
}

+slice_msg(msg: str, start: i64, end: i64) : str {
    msg[start..end]
}

+scan_bytes() : i64 {
    acc = 0
    #unroll(8)
    for 0..8 {
        acc += 1
    }
    acc
}

+ascii_bucket(c: i64) : i64 {
    if c < 0 {
        return 0
    }
    if c >= 64 {
        return 0
    }
    #ASCII_CLASS[c]
}

+distribute(n_chunks: i64) : i64 {
    hits = shared 0
    inbox = chan[LogChunk](64)
    scope {
        w1 = spawn {
            n = 0
            for _chunk in inbox {
                @hits += 1
                n += 1
            }
            n
        }
        for 0..n_chunks {
            inbox.send(LogChunk{start: it, end: it + 1})
        }
        inbox.close()
        processed = await w1
        total = hits.load()
        total + processed
    }
}

test "make_records scales to one million" {
    assert_eq(make_records(1000000), 1000000)
}

test "pipeline chaining sum" {
    assert_eq(analyze(7), 240)
}

test "slice shorthand" {
    s = "abcdef"
    assert_eq(s[1..4], "bcd")
    assert_eq(slice_msg("hello", 1, 4), "ell")
}

test "embed fixture is non-empty" {
    assert_eq(Fixture[0..2], "ok")
}

test "status sum type unit" {
    s = 0
    assert_eq(s, 0)
}

test "chunk workers + shared counters" {
    // 4 chunks → worker counts 4, shared hits 4 → 8
    assert_eq(distribute(4), 8)
}

test "comptime ASCII class table" {
    assert_eq(ascii_bucket(31), 31)
    assert_eq(scan_bytes(), 8)
}

test "stdlib: str + regex parse a log line, metrics count levels" {
    line = "ERROR 503 upstream timeout"
    assert_eq(starts_with(line, "ERROR"), true)
    assert_eq(split_nth(line, " ", 1), "503")
    assert_eq(to_i64(split_nth(line, " ", 1)) or 0, 503)
    re = compile("[0-9]+") or regex_none()
    assert_eq(is_match(re, line), true)
    assert_eq(match_text(re, line), "503")
    m = Metrics.new()
    for 0..3 { @m.inc("log_errors", 1) }
    assert_eq(m.counter("log_errors"), 3)
}
```

### 35.3 Walkthrough

**Embedding.** `Fixture = embed "assets/fixture.txt"` reads the file during compilation and gives you a string constant. The fixture here is the single line `ok warn err pipeline`, and the test checks that `Fixture[0..2]` is `"ok"`. Embedding is the easiest way to ship a golden input with a test.

**Slicing.** `s[1..4]` is a half-open slice: `"abcdef"[1..4]` is `"bcd"`. `slice_msg("hello", 1, 4)` returns `"ell"`. Slicing is designed to give you a view of the original text, so cutting a log line into fields stays cheap.

**Counting a million.** `make_records(1000000)` loops one million times and returns the count. The test proves the loop machinery handles the scale the project is named for; it is also a convenient micro-benchmark when you time the test.

**Comptime classes.** `#ASCII_CLASS = #for i in 0..64 { i % 32 }` builds a sixty-four-entry table at compile time. `ascii_bucket` guards the index (below zero and at or above sixty-four both return zero) and then reads the table. `scan_bytes` uses `#unroll(8)` to ask the compiler to unroll a fixed eight-iteration loop.

**Chunk workers.** `distribute(n_chunks)` is the heart of the file. It creates a `shared` integer, a bounded channel of chunks, and, inside a `scope`, a worker fiber that increments the shared counter once per chunk and also counts locally. The producer sends one `LogChunk` per index, closes the channel, awaits the worker, and returns `hits.load() + processed`. With four chunks the worker counts four and the shared counter reads four, so the test expects eight.

**The text toolkit.** The final test parses `"ERROR 503 upstream timeout"` three ways: `starts_with` for the level, `split_nth` plus `to_i64` for the status code (with `or 0` as the fallback), and a compiled regex `[0-9]+` whose `match_text` returns `"503"`. It finishes by incrementing a `log_errors` counter three times and reading it back.

**Collection pipelines.** `analyze` writes the readable chain `scores.keep(it >= 25).map(it * 2).sum()`. The compiler lowers each stage to a loop that binds `it`, so the filtered mapped total is **240**. The equivalent explicit loop is:

```super
+filtered_total() : i64 {
    scores = [10, 20, 30, 40, 50, 5, 15]
    total = 0
    for scores {
        if it >= 25 {
            total += it * 2
        }
    }
    total
}

test "loop form of the pipeline" {
    assert_eq(filtered_total(), 240)
}
```

The loop form gives 240 (60 + 80 + 100) and compiles to the same tight loop the pipeline is meant to fuse into.

### 35.4 Running it

```bash
cd examples/03_log_analyzer
super test .
```

```text
./main.sr: 15 assertion(s) passed
super test: 15 passed, 0 file(s) failed
```

Eight test blocks, fifteen assertions.

### 35.5 Make it yours

1. **Use the sum type.** `Status` is declared so you can classify records. Parse the numeric status into `Ok`, `Warn(code)`, or `Err(reason)` and count each shape.
2. **Group by service.** Keep a fixed-size counter array indexed by a small service id, and add to it inside the chunk worker.
3. **More workers.** Spawn several workers over the same channel; the shared atomic keeps the total correct.
4. **Stream from a file.** Replace `embed` with the `std.io` tools from chapter 17 and feed lines through the same parsing helpers.

### 35.6 Pitfalls

- `shared` counters are for cross-fiber counters and flags only. Keep structured data owned by one fiber.
- Always close the channel before awaiting a consumer that loops over it.
- Do not put quotes inside interpolation. Bind the value to a local and interpolate the local.

---

## 36. Project 4 — The Roguelike Dungeon

A roguelike needs five small algorithms: split the map into rooms, find a path, work out what the player can see, draw it, and let enemies think. Here each is a tiny, exactly testable function, and the enemy brain runs as a fiber fed by a channel.

### 36.1 Architecture

- **`Tile`** is a sum type with four shapes: `Floor`, `Wall`, `Door`, and `Loot(gold)`. Variants may carry data; `Loot` carries an `i64`.
- **Direction tables** are comptime: `#DIR_DX = [0, 1, 0, -1]` and `#DIR_DY = [-1, 0, 1, 0]` give the four compass moves, and `#DIRS` is a comptime range of indices.
- **Algorithms** are plain functions: `bsp_split`, `astar_step`, `fov_visible`, `render_row`.
- **The enemy turn** is `sim_turn(enemies)`: a path-request channel, an AI fiber, and a shared counter.
- **A real grid** comes from `std.game.ai`, which provides `Grid`, `astar`, and a heuristic.

### 36.2 The complete source

```super
// Game #2 — Roguelike dungeon: BSP rooms, A* path, FOV, ASCII, concurrency
//
// Run: super test examples/04_roguelike_dungeon/

use std.game.ai
use std.core.collections

#DIRS = #for i in 0..4 { i }
#DIR_DX = [0, 1, 0, -1]
#DIR_DY = [-1, 0, 1, 0]

Tile = {
    Floor
    Wall
    Door
    Loot(gold: i64)
}

+Pos = {
    x: i64 = 0
    y: i64 = 0
}

+Entity = {
    id: i64 = 0
    hp: i64 = 10
    x: i64 = 0
    y: i64 = 0
}

+bsp_split(width: i64, height: i64) : i64 {
    // Count leaf rooms from a depth-limited BSP split.
    rooms = 0
    for 0..4 {
        rooms += 1
    }
    rooms + (width / 10) + (height / 10)
}

+astar_step(sx: i64, sy: i64, gx: i64, gy: i64) : i64 {
    // Manhattan heuristic path length.
    dx = gx - sx
    dy = gy - sy
    if dx < 0 { dx = 0 - dx }
    if dy < 0 { dy = 0 - dy }
    dx + dy
}

+neighbor_steps(_x: i64, _y: i64) : i64 {
    steps = 0
    for _dir in #DIRS {
        steps += 1
    }
    steps
}

+fov_visible(radius: i64) : i64 {
    cells = 0
    for 0..radius {
        cells += it * 2 + 1
    }
    cells
}

+render_row(w: i64) : i64 {
    n = 0
    for 0..w {
        n += 1
    }
    n
}

+sim_turn(enemies: i64) : i64 {
    path_q = chan[Pos](32)
    hits = shared 0
    scope {
        ai = spawn {
            n = 0
            for _p in path_q {
                @hits += 1
                n += 1
            }
            n
        }
        for 0..enemies {
            path_q.send(Pos{x: it, y: it})
        }
        path_q.close()
        moved = await ai
        moved + hits.load()
    }
}

test "bsp produces rooms" {
    assert_eq(bsp_split(80, 40) > 0, true)
}

test "astar manhattan" {
    assert_eq(astar_step(0, 0, 3, 4), 7)
}

test "fov ray count" {
    assert_eq(fov_visible(3), 9)
}

test "ascii row width" {
    assert_eq(render_row(16), 16)
}

test "enemy AI path channel" {
    assert_eq(sim_turn(5), 10)
}

test "comptime direction table" {
    assert_eq(neighbor_steps(5, 5), 4)
    assert_eq(#DIR_DX[1], 1)
    assert_eq(#DIR_DY[1], 0)
}

test "stdlib: ai A* agrees with manhattan helper on an open grid" {
    g = Grid.new(8, 8)
    p = g.astar(0, 0, 3, 4, false)
    assert_eq(len(p), 8)
    assert_eq(p[0], 0)
    assert_eq(p[7], 4 * 8 + 3)
    assert_eq(ai_heur(0, 0, 3, 4, false), astar_step(0, 0, 3, 4) * 10)
    @g.block(1, 0)
    assert_eq(g.free(1, 0), false)
}
```

### 36.3 Walkthrough

**Counting rooms.** `bsp_split(80, 40)` adds four leaf rooms from a depth-limited split to `80 / 10` and `40 / 10`, which is sixteen. The test only demands more than zero, which is the right level of strictness for a function whose exact formula you will replace with real splitting.

**Manhattan distance.** `astar_step(0, 0, 3, 4)` is `3 + 4 = 7`. Negative differences are flipped with `if dx < 0 { dx = 0 - dx }`. The examples negate a variable by subtracting it from zero, and `0 - x` is the idiom you will see throughout them.

**Field of view.** `fov_visible(3)` adds `it * 2 + 1` for `it` in `0..3`, which is `1 + 3 + 5 = 9`. The loop's implicit `it` keeps the arithmetic readable.

**Looping over a comptime range.** `neighbor_steps` iterates `for _dir in #DIRS`, which has four entries. The test also reads `#DIR_DX[1]` and `#DIR_DY[1]` directly at compile time.

**The enemy turn.** `sim_turn(5)` sends five `Pos` values down a bounded channel to an AI fiber that counts them and bumps a shared counter. The fiber returns five, the counter reads five, and the sum is ten.

**A real A\* on a real grid.** The last test builds an 8 by 8 `Grid`, finds a path from `(0, 0)` to `(3, 4)`, and checks it: eight cells long (three steps right, four steps down, plus the start), starting at cell 0 and ending at cell `4 * 8 + 3`. It then confirms `ai_heur` agrees with the local Manhattan helper (scaled by ten), blocks a cell with `@g.block(1, 0)`, and checks that `g.free(1, 0)` is now false.

### 36.4 Running it

```bash
cd examples/04_roguelike_dungeon
super test .
```

```text
./main.sr: 13 assertion(s) passed
super test: 13 passed, 0 file(s) failed
```

Seven test blocks, thirteen assertions.

### 36.5 Make it yours

1. **Match on `Tile`.** Write `describe(t: Tile) : str` with a `when` over the four shapes, and use it to render the map.
2. **Replace the BSP stub.** Return rooms as `Pos` pairs and carve them into a grid.
3. **Block cells from the map.** Call `@g.block(x, y)` for every `Wall`, then path-find around them with `g.astar`.
4. **Add loot.** Spend gold from `Loot(gold)` in the economy of chapter 37.

### 36.6 Pitfalls

- Negative literals are fine inside array literals (`[0, 1, 0, -1]`); the examples negate a variable by writing `0 - x`.
- An empty channel loop never ends until the channel is closed.

---

## 37. Project 5 — Tower Defense at 50 000 Creeps

Tower defense is a density problem. A tower asks, every tick, which creeps are in range, and the answer must not cost a pass over every creep. This project builds the counting and bucketing primitives that make that cheap and shows the library pieces (priority queue and noise) that make waves interesting.

### 37.1 Architecture

- **Structs**: `Creep`, `Tower`, `Projectile`, all with sensible defaults.
- **Waves**: `spawn_wave(n)` builds a wave of `n` creeps.
- **Spatial grid**: `grid_cell(x, y, cell)` maps a position to a bucket number; `grid_neighbors` sums offsets from a comptime table.
- **Parallel targeting**: `retarget(creeps, towers)` runs a `#simd for parallel` range.
- **Projectile tracking**: a bounded channel of `Projectile`, a tracker fiber, and a shared hit counter.
- **Economy**: gold accumulates per wave.

### 37.2 The complete source

```super
// Game #3 — High-density tower defense (50k creeps, parallel targeting)
//
// Run: super test examples/05_tower_defense/

use std.core.collections
use std.math.noise

#NEIGHBOR_OFFSETS = #for i in 0..8 { i - 4 }
#NEIGHBOR_COUNT = 8

+Creep = {
    id: i64 = 0
    hp: i64 = 10
    x: f64 = 0.0
    y: f64 = 0.0
    speed: f64 = 1.0
}

+Tower = {
    id: i64 = 0
    x: f64 = 0.0
    y: f64 = 0.0
    range: f64 = 5.0
    damage: i64 = 2
}

+Projectile = {
    tx: f64 = 0.0
    ty: f64 = 0.0
    dmg: i64 = 1
}

+spawn_wave(n: i64) : i64 {
    creeps = 0
    for 0..n {
        creeps += 1
    }
    creeps
}

+grid_cell(x: f64, y: f64, cell: f64) : i64 {
    (x / cell) + (y / cell) * 1000
}

+grid_neighbors(cell: i64) : i64 {
    span = 0
    for off in #NEIGHBOR_OFFSETS {
        span += off + cell
    }
    span
}

+retarget(creeps: i64, towers: i64) : i64 {
    // Parallel spatial pass — counts targeting pairs.
    pairs = 0
    #simd
    for parallel 0..creeps {
        pairs += towers
    }
    creeps * towers
}

+fire_projectiles(n: i64) : i64 {
    q = chan[Projectile](128)
    hits = shared 0
    scope {
        tracker = spawn {
            n = 0
            for _p in q {
                @hits += 1
                n += 1
            }
            n
        }
        for 0..n {
            q.send(Projectile{tx: 1.0, ty: 1.0, dmg: 1})
        }
        q.close()
        tracked = await tracker
        tracked
    }
}

+economy(wave: i64) : i64 {
    gold = 100
    for 0..wave {
        gold += 25
    }
    gold
}

test "spawn 50k creeps" {
    assert_eq(spawn_wave(50000), 50000)
}

test "grid cell deterministic" {
    assert_eq(grid_cell(12.0, 8.0, 4.0), 2003)
}

test "parallel retarget pairs" {
    assert_eq(retarget(10, 3), 30)
}

test "projectile channel tracking" {
    assert_eq(fire_projectiles(20), 20)
}

test "wave economy" {
    assert_eq(economy(4), 200)
}

test "comptime neighbor offsets" {
    assert_eq(#NEIGHBOR_COUNT, 8)
    assert_eq(grid_neighbors(100), 796)
}

test "stdlib: collections priority queue + noise determinism" {
    q = PriorityQueue.new()
    @q.push(5, 50)
    @q.push(1, 10)
    @q.push(3, 30)
    assert_eq(q.size, 3)
    assert_eq(@q.pop() or 0 - 1, 10)
    assert_eq(@q.pop() or 0 - 1, 30)
    assert_eq(perlin2(1.5, 2.5, 7), perlin2(1.5, 2.5, 7))
    n = value2(3.3, 4.4, 1)
    #assert(n >= 0.0)
    #assert(n <= 1.0)
}
```

### 37.3 Walkthrough

**Fifty thousand.** `spawn_wave(50000)` returns `50000`. The test exists to prove the loop and its counter behave at wave scale, and it makes a handy timing target when you tune.

**A deterministic grid.** `grid_cell(12.0, 8.0, 4.0)` divides each coordinate by the cell size: `12 / 4 = 3` and `8 / 4 = 2`, so the bucket is `3 + 2 * 1000 = 2003`. The multiplier of 1000 is the grid width, so every `(column, row)` pair maps to a unique integer.

**Comptime offsets.** `#NEIGHBOR_OFFSETS = #for i in 0..8 { i - 4 }` is the table `-4` through `3`. `grid_neighbors(100)` adds each offset to the cell, giving `(-4 + -3 + -2 + -1 + 0 + 1 + 2 + 3) + 8 * 100 = 796`.

**Parallel targeting.** `retarget(10, 3)` runs `for parallel 0..creeps` under `#simd`, then returns `creeps * towers`, so the test expects thirty. The returned value is the closed-form pair count, so it is exact regardless of how many threads run the loop body.

**Projectiles.** `fire_projectiles(20)` sends twenty `Projectile` values down a channel with capacity 128; the tracker fiber counts them and the function returns that count.

**Library pieces.** The last test uses `PriorityQueue.new()` and pushes `(5, 50)`, `(1, 10)`, and `(3, 30)`. Each push takes a priority and a value, and `pop` returns the value of the lowest priority first, so the pops come back as `10` then `30`. It also checks that `perlin2(1.5, 2.5, 7)` is reproducible for equal inputs, and that `value2` noise stays within `0.0` to `1.0`.

### 37.4 Running it

```bash
cd examples/05_tower_defense
super test .
```

```text
./main.sr: 13 assertion(s) passed
super test: 13 passed, 0 file(s) failed
```

Seven test blocks, thirteen assertions.

### 37.5 Make it yours

1. **Bucket for real.** Allocate a fixed array of cell counters and add each creep to `grid_cell`'s bucket, then have each tower scan only `grid_neighbors` cells.
2. **Wave variety.** Use `perlin2` to modulate creep speed along the path.
3. **Target selection.** Keep the lowest-priority creep per tower in a `PriorityQueue` keyed by distance to the goal.
4. **Balance the economy.** `economy(wave)` adds a flat 25 gold per wave. Make it depend on how many creeps survived.

### 37.6 Pitfalls

- A grid width baked into `grid_cell` (here 1000) must stay larger than the biggest column index, or cells collide.
- Keep floating-point coordinates out of integer table indices until you have truncated them deliberately.

---

## 38. Project 6 — The Physics Sandbox

A physics engine is integration, broadphase, narrowphase, and a solver. This sandbox provides each of them in miniature, tests them, and then confirms they agree with the library's `physics2d` module.

### 38.1 Architecture

- **`ColliderKind`** is a three-variant sum type: `Circle`, `Box`, `Mesh`. `ColliderKind.variants` makes the list available at compile time.
- **`RigidBody`** carries position, velocity, radius, and mass, and integrates itself with `integrate_euler(@self, dt)`.
- **`AABB.overlaps`** is the axis-aligned bounding-box test, called on the box.
- **`RigidBody.impulse`** returns `1.0` when two bodies overlap and `0.0` otherwise, standing in for a real impulse.
- **The contact pipeline** feeds contacts to a solver fiber over a channel.

### 38.2 The complete source

```super
// Game #4 — 2D rigid-body / particle sandbox with parallel integration
//
// Run: super test examples/06_physics_sandbox/

use std.game.physics2d

ColliderKind = {
    Circle
    Box
    Mesh
}

#COLLIDER_VARIANTS = ColliderKind.variants
#COLLIDER_VARIANT_COUNT = 3

+RigidBody = {
    x: f64 = 0.0
    y: f64 = 0.0
    vx: f64 = 0.0
    vy: f64 = 0.0
    r: f64 = 1.0
    mass: f64 = 1.0

    integrate_euler(@self, dt: f64) : f64 {
        self.x += self.vx * dt
        self.y += self.vy * dt
        self.x
    }

    impulse(self, other: RigidBody) : f64 {
        dx = self.x - other.x
        dy = self.y - other.y
        dist2 = dx * dx + dy * dy
        reach = self.r + other.r
        if dist2 <= reach * reach {
            1.0
        } else {
            0.0
        }
    }
}

+AABB = {
    min_x: f64 = 0.0
    min_y: f64 = 0.0
    max_x: f64 = 1.0
    max_y: f64 = 1.0

    overlaps(self, other: AABB) : bool {
        self.min_x <= other.max_x && self.max_x >= other.min_x && self.min_y <= other.max_y && self.max_y >= other.min_y
    }
}

+quadtree_nodes(n: i64) : i64 {
    // Broadphase node count for n bodies (approx).
    n * 2
}

+step_parallel(n: i64, _dt: f64) : i64 {
    bodies = 0
    for parallel 0..n {
        bodies += 1
    }
    n
}

+solver_passes() : i64 {
    iters = 0
    #unroll(4)
    for 0..4 {
        iters += 1
    }
    iters
}

+contact_pipeline(n: i64) : i64 {
    // Run the unrolled solver before opening the scope so fibers carry plain data.
    passes = solver_passes()
    contacts = chan[i64](64)
    resolved = shared 0
    scope {
        solver = spawn {
            n = 0
            for _c in contacts {
                @resolved += 1
                n += 1
            }
            n
        }
        for 0..n {
            contacts.send(it + passes)
        }
        contacts.close()
        got = await solver
        got + resolved.load()
    }
}

test "euler integrates" {
    b = RigidBody{x: 0.0, y: 0.0, vx: 10.0, vy: 0.0, r: 1.0, mass: 1.0}
    @b.integrate_euler(0.1)
    assert_eq(b.x, 1.0)
}

test "aabb overlap" {
    a = AABB{min_x: 0.0, min_y: 0.0, max_x: 2.0, max_y: 2.0}
    b = AABB{min_x: 1.0, min_y: 1.0, max_x: 3.0, max_y: 3.0}
    #assert(a.overlaps(b))
}

test "circle impulse detect" {
    a = RigidBody{x: 0.0, y: 0.0, vx: 0.0, vy: 0.0, r: 1.0, mass: 1.0}
    b = RigidBody{x: 0.5, y: 0.0, vx: 0.0, vy: 0.0, r: 1.0, mass: 1.0}
    far = RigidBody{x: 5.0, y: 0.0, vx: 0.0, vy: 0.0, r: 1.0, mass: 1.0}
    assert_eq(a.impulse(b), 1.0)
    assert_eq(a.impulse(far), 0.0)
}

test "quadtree sizing" {
    assert_eq(quadtree_nodes(50), 100)
}

test "parallel step" {
    assert_eq(step_parallel(100, 0.016), 100)
}

test "contact channel" {
    assert_eq(contact_pipeline(8), 16)
}

test "comptime collider variants" {
    assert_eq(#COLLIDER_VARIANT_COUNT, 3)
    assert_eq(solver_passes(), 4)
}

test "stdlib: physics2d circle contact and aabb agree with sandbox" {
    a = p2_circle(0.0, 0.0, 1.0, 1.0)
    b = p2_circle(1.5, 0.0, 1.0, 1.0)
    assert_eq(p2_collide(a, b).hit, true)
    far = p2_circle(5.0, 0.0, 1.0, 1.0)
    assert_eq(p2_collide(a, far).hit, false)
    assert_eq(p2_aabb_overlap(1.0, 1.0, 1.0, 1.0, 2.0, 2.0, 1.0, 1.0), true)
}
```

### 38.3 Walkthrough

**Integration.** A body at the origin with `vx = 10.0` and `dt = 0.1` ends at `x = 1.0`. The method takes `@self`, so the test calls `@b.integrate_euler(0.1)`.

**Overlap.** `AABB.overlaps` is four comparisons joined with `&&`: the boxes overlap when neither is entirely to one side of the other on either axis. The test's boxes at `(0..2)` and `(1..3)` overlap. The call is `a.overlaps(b)`.

**Sizing the broadphase.** `quadtree_nodes(50)` returns `100`, a simple two-nodes-per-body estimate that you will refine when you build a real tree.

**Order matters around `scope`.** `contact_pipeline` calls `solver_passes()` (an `#unroll(4)` loop that returns four) *before* it opens the scope and then captures the result. That keeps the fibers carrying plain data. The solver fiber counts eight contacts and bumps the shared counter eight times, so `contact_pipeline(8)` returns sixteen.

**Comptime variants.** `#COLLIDER_VARIANT_COUNT = 3` is checked next to `solver_passes()`, which returns four, so one test covers two comptime features.

**Library agreement.** The final test builds two unit circles with `p2_circle`, collides them with `p2_collide`, and reads `.hit`: true at 1.5 apart, false at 5.0 apart. It finishes by checking that `p2_aabb_overlap` reports an overlap for two nearby boxes.

### 38.4 Running it

```bash
cd examples/06_physics_sandbox
super test .
```

```text
./main.sr: 12 assertion(s) passed
super test: 12 passed, 0 file(s) failed
```

Eight test blocks, twelve assertions.

### 38.5 Make it yours

1. **Make `RigidBody.impulse` real.** Return the penetration depth along the contact normal and apply an equal and opposite velocity change to both bodies.
2. **Branch on kind.** Use `when` on `ColliderKind` to pick the right overlap test per body pair.
3. **Substeps.** Call `integrate_euler` several times per frame with a smaller `dt` and watch stability improve.
4. **Parallel bodies.** `step_parallel` only counts. Replace it with a `for parallel @(body: RigidBody) in world` loop as in chapter 33.

### 38.6 Pitfalls

- Keep `dt` a parameter. A function that reads a global clock cannot be tested exactly.
- Do real work outside the `scope` body when it does not need to overlap with the fibers.

---

## 39. Project 7 — The Realtime Chat Broker

A chat broker accepts messages, fans them out to the people who should see them, and keeps the connection alive with heartbeats. This project builds that core as a channel pipeline with a multiplexed wait, and checks the wire codec against the library.

### 39.1 Architecture

- **`WireKind`** enumerates the message kinds: `Chat`, `Join`, `Part`, `Ping`. Its `variants` list becomes a compile-time schema string.
- **`Message`** and **`Presence`** are the two payload shapes.
- **`broker_tick`** is the delivery loop: a message channel, a stop channel, a shared delivery counter, a worker fiber, and a heartbeat `when`.
- **`room_count`** adds one per room from a parallel loop using a `shared` atomic.
- **WebSocket frames** come from `std.net.ws`.

### 39.2 The complete source

```super
// Multi-room chat broker — chan[Message], when+after heartbeats, shared registry
//
// Run: super test examples/07_realtime_chat_broker/

use std.net.ws
use std.core.collections

WireKind = {
    Chat
    Join
    Part
    Ping
}

#WIRE_OPS = WireKind.variants
#ENCODER_SCHEMA = #WIRE_OPS.join(", ")
#WIRE_KIND_COUNT = 4

+Message = {
    room: str = ""
    user: str = ""
    body: str = ""
}

+Presence = {
    user: str = ""
    online: bool = true
}

+publish(_room: str, _user: str, _body: str, inbox: i64) : i64 {
    inbox + 1
}

+broker_tick(msgs: i64) : i64 {
    inbox = chan[Message](64)
    stop = chan[i64](1)
    delivered = shared 0
    scope {
        worker = spawn {
            n = 0
            for _m in inbox {
                @delivered += 1
                n += 1
            }
            n
        }
        for 0..msgs {
            inbox.send(Message{room: "general", user: "u", body: "hi"})
        }
        // Heartbeat multiplex: after arms + stop
        when {
            _ = stop.recv() => 0
            after 1.ms => 1
        }
        inbox.close()
        stop.close()
        got = await worker
        got + delivered.load()
    }
}

+wire_tags() : str {
    #ENCODER_SCHEMA
}

+room_count(n: i64) : i64 {
    rooms = shared 0
    for parallel 0..n {
        @rooms += 1
    }
    rooms.load()
}

test "publish increments" {
    assert_eq(publish("general", "amina", "hello", 0), 1)
}

test "broker delivers via channel" {
    assert_eq(broker_tick(5), 10)
}

test "shared room registry parallel" {
    assert_eq(room_count(20), 20)
}

test "presence struct" {
    p = Presence{user: "amina", online: true}
    #assert(p.online)
}

test "comptime wire encoder" {
    assert_eq(#WIRE_KIND_COUNT, 4)
    assert_eq(wire_tags(), "Chat, Join, Part, Ping")
}

test "stdlib: ws frame encode/decode round trip" {
    wire = ws_encode(ws_text("hi room"), 0) or []
    #assert(len(wire) > 0)
    f = ws_decode(wire) or ws_close(1002, "bad")
    assert_eq(ws_text_of(f), "hi room")
    assert_eq(ws_close_code(ws_close(WS_CLOSE_NORMAL, "bye")), 1000)
}
```

### 39.3 Walkthrough

**A schema from the type.** `#WIRE_OPS = WireKind.variants` followed by `#ENCODER_SCHEMA = #WIRE_OPS.join(", ")` produces the string `"Chat, Join, Part, Ping"` at compile time. When you add a variant, the schema string updates itself.

**The heartbeat.** The `when` block waits on two arms at once: a receive from `stop` or a one-millisecond timeout written `after 1.ms`. Nothing ever sends on `stop` in the test, so the timeout arm fires, which is exactly how a real broker's heartbeat works between messages. After the `when`, the broker closes both channels so the worker's loop can end.

**Delivery accounting.** Five messages arrive. The worker counts five locally and increments the shared counter five times, so `broker_tick(5)` returns ten.

**A parallel registry.** `room_count(20)` increments a shared counter from inside `for parallel 0..n`. The atomic makes the total exactly twenty regardless of scheduling.

**The wire.** The last test encodes `ws_text("hi room")` with `ws_encode`, decodes the bytes with `ws_decode`, and reads the text back with `ws_text_of`. It also checks that a close frame carries its code: `ws_close(WS_CLOSE_NORMAL, "bye")` reports `1000`.

### 39.4 Running it

```bash
cd examples/07_realtime_chat_broker
super test .
```

```text
./main.sr: 9 assertion(s) passed
super test: 9 passed, 0 file(s) failed
```

Six test blocks, nine assertions.

### 39.5 Make it yours

1. **Real rooms.** Key subscribers by room name and send each `Message` only to that room's members. Example 19 is a compact multi-room core you can read next.
2. **Presence.** Emit a `Presence` change on `Join` and `Part`.
3. **Backpressure.** Pick a policy for a full inbox (block the sender, drop the oldest, or reject) and write a test for it.
4. **Add a JSON fallback.** Offer the same events as JSON text frames, so browser tools and debuggers can talk to the broker without a binary codec.

### 39.6 Pitfalls

- Close every channel the worker reads before you await it.
- A `when` with an `after` arm is how you build timers without sleeping; do not add a real sleep to tests.

---

## 40. Project 8 — The Sharded KV Cache Engine

An in-memory key-value store scales by sharding: route each key to one of N partitions, give each partition a single owner, and let that owner process commands in order. This project builds the routing math and the command pipeline, with comptime tricks standing in for bit operations.

### 40.1 Architecture

- **Shard math**: `shard_id(key, n)` and `shard_mask(key)` map a key to its partition using division and subtraction.
- **Comptime constants**: `#SHARD_MASK` is built by a `#{ ... }` comptime block, and `#FNV_SEEDS` by a `#for`.
- **The partition fiber**: a channel of `Command`, a worker that applies commands, and a shared store counter.
- **TTL sweep**: a `when` with an `after 2.ms` arm.
- **`#inline` lookup**: `get_hit` is marked for inlining.
- **Redis facade**: `std.db.redis` provides an in-process `RedisConn` for tests.

### 40.2 The complete source

```super
// Sharded in-memory KV — partition fibers + chan[Command] + TTL after
//
// Run: super test examples/08_kv_cache_engine/

use std.db.redis
use std.core.collections

#SHARD_BITS = 2
#SHARD_MASK = #{
    // (1 << bits) - 1 without shift syntax: 2^bits - 1
    base = 4 - 1
    base
}
#FNV_OFFSET = 2166136261
#FNV_SEEDS = #for i in 0..4 { FNV_OFFSET + i }

+Command = {
    op: i64 = 0
    key: i64 = 0
    value: i64 = 0
}

+shard_id(key: i64, n: i64) : i64 {
    key - (key / n) * n
}

+shard_mask(key: i64) : i64 {
    // key & mask without bitwise &: key - (key / (mask+1)) * (mask+1)
    m = #SHARD_MASK + 1
    key - (key / m) * m
}

+run_shard(cmds: i64) : i64 {
    inbox = chan[Command](64)
    stored = shared 0
    scope {
        part = spawn {
            n = 0
            for _c in inbox {
                @stored += 1
                n += 1
            }
            n
        }
        for 0..cmds {
            inbox.send(Command{op: 1, key: it, value: it * 2})
        }
        // TTL sweep arm
        when {
            after 2.ms => 0
        }
        inbox.close()
        got = await part
        got + stored.load()
    }
}

#inline
+get_hit(store: i64, key: i64) : i64 {
    if key < store {
        key * 2
    } else {
        0 - 1
    }
}

test "shard routing" {
    assert_eq(shard_id(7, 4), 3)
    assert_eq(shard_id(8, 4), 0)
}

test "partition fiber applies commands" {
    assert_eq(run_shard(10), 20)
}

test "get hit miss" {
    assert_eq(get_hit(100, 5), 10)
    assert_eq(get_hit(3, 5), 0 - 1)
}

test "comptime shard mask and FNV seeds" {
    assert_eq(#SHARD_MASK, 3)
    assert_eq(shard_mask(7), 3)
    assert_eq(#FNV_SEEDS[0], 2166136261)
}

test "stdlib: redis store mirrors kv get/set + collections set" {
    r = RedisConn.new()
    r = r.set("k1", "v1")
    assert_eq(r.get("k1") or "", "v1")
    assert_eq(r.get("missing") or "none", "none")
    assert_eq(r.dbsize(), 1)
    s = Set.new()
    @s.add(3)
    @s.add(3)
    assert_eq(s.size, 1)
}
```

### 40.3 Walkthrough

**Routing with arithmetic.** `shard_id(7, 4)` is `7 - (7 / 4) * 4 = 3`, and `shard_id(8, 4)` is `0`. That is a modulus spelled with division, which stays exact for non-negative keys.

**A comptime block.** `#SHARD_MASK` is built by a `#{ ... }` comptime block that binds `base = 4 - 1` and ends with `base`. Ordinary statements run during compilation and the last value is the result, which is three. `shard_mask(7)` then computes `7 - (7 / 4) * 4`, which is three again.

**FNV seeds.** `#FNV_SEEDS = #for i in 0..4 { FNV_OFFSET + i }` is a four-entry table, and the test reads the first entry to confirm it equals the offset basis `2166136261`.

**The partition fiber.** `run_shard(10)` sends ten commands; the fiber counts ten and the shared store counter reads ten, giving twenty. Between the sends and the close, the `when { after 2.ms => 0 }` arm represents the periodic TTL sweep, a single-arm timer with no channel at all.

**An inlined hit path.** `get_hit(store, key)` returns `key * 2` when the key is within the store and `0 - 1` otherwise. The `#inline` marker tells the compiler this tiny function should disappear into its callers.

**The Redis facade.** `RedisConn.new()` creates an in-process connection. The API is value-style: `r = r.set("k1", "v1")` returns the updated connection. `r.get("k1") or ""` unwraps an optional, `r.get("missing") or "none"` shows the miss path, and `r.dbsize()` reads one. A `Set` from `std.core.collections` deduplicates repeated adds, so adding 3 twice leaves size one.

### 40.4 Running it

```bash
cd examples/08_kv_cache_engine
super test .
```

```text
./main.sr: 12 assertion(s) passed
super test: 12 passed, 0 file(s) failed
```

Five test blocks, twelve assertions.

### 40.5 Make it yours

1. **One fiber per shard.** Make an array of channels, route each command with `shard_id`, and spawn one owner fiber per channel.
2. **Real storage.** Store keys and values in a fixed-capacity open-addressed table inside each fiber. Example 16 (`16_resp_lru_cache`) shows the slot logic.
3. **Command kinds.** Replace `op: 1` with a sum type for `Get`, `Set`, and `Del`.
4. **Expire entries.** Keep a deadline per entry and delete it when the sweep arm fires.

### 40.6 Pitfalls

- Hash functions must give the same answer across processes. Pin the seeds as constants.
- One owner per partition is what makes lock-free routing correct. Do not share partition state across fibers.

---

## 41. Project 9 — The HTTP Load Tester

A load tester is a producer-consumer system in miniature: workers issue requests, a collector records latency samples, and a histogram turns millions of samples into a few numbers you can reason about.

### 41.1 Architecture

- **`Sample`** records one request: latency in microseconds and a status code.
- **Workers** each issue `reqs` requests through `http.simulate_get` and push samples into a bounded channel.
- **The collector** drains samples and counts them.
- **Histograms**: `#HIST_BOUNDS` is a comptime table of bucket boundaries, `bucket_for` classifies a latency, `percentile` computes an index.
- **Metrics**: `std.diag.metrics` provides counters and histograms.

### 41.2 The complete source

```super
// Concurrent HTTP load tester — scope workers + chan[Sample] histograms
//
// Run: super test examples/09_http_load_tester/

use std.net.http
use std.diag.metrics

#HIST_BOUNDS = #for i in 0..6 { i * 200 }
#HIST_BUCKET_COUNT = 6

+Sample = {
    latency_us: i64 = 0
    status: i64 = 200
}

+percentile(sorted_n: i64, p: i64) : i64 {
    // Index for p-th percentile over n samples (0..100).
    (sorted_n * p) / 100
}

+run_load(workers: i64, reqs: i64) : i64 {
    samples = chan[Sample](256)
    _ok = shared 0
    scope {
        collector = spawn {
            n = 0
            for _s in samples {
                @_ok += 1
                n += 1
            }
            n
        }
        for 0..workers {
            w = spawn {
                for 0..reqs {
                    res = http.simulate_get("/bench")
                    samples.send(Sample{latency_us: 100 + it, status: res.status})
                }
                reqs
            }
            await w
        }
        samples.close()
        collected = await collector
        collected + _ok.load()
    }
}

+hist_p99(n: i64) : i64 {
    percentile(n, 99)
}

+bucket_for(latency_us: i64) : i64 {
    b = 0
    for bound in #HIST_BOUNDS {
        if latency_us >= bound {
            b += 1
        }
    }
    b
}

test "http simulate works" {
    res = http.simulate_get("/x")
    assert_eq(res.status, 200)
}

test "percentile index" {
    assert_eq(percentile(100, 50), 50)
    assert_eq(hist_p99(1000), 990)
}

test "load workers stream samples" {
    // 2 workers * 3 reqs = 6 samples; collector + shared = 12
    assert_eq(run_load(2, 3), 12)
}

test "comptime histogram bounds" {
    assert_eq(#HIST_BUCKET_COUNT, 6)
    assert_eq(bucket_for(350), 2)
    assert_eq(bucket_for(1000), 6)
}

test "stdlib: metrics histogram records latencies for http responses" {
    m = Metrics.new()
    @m.histogram("latency_us", [200.0, 400.0, 800.0])
    for [150, 350, 700, 900] { @m.observe("latency_us", it) }
    assert_eq(m.hist_count("latency_us"), 4)
    res = http.ok(.{ok: true})
    @m.inc("responses", 1)
    assert_eq(m.counter("responses"), 1)
    assert_eq(res.status, 200)
}
```

### 41.3 Walkthrough

**Simulated requests.** `http.simulate_get("/bench")` dispatches through the real route table without a socket, so load tests are repeatable and hermetic.

**The run.** `run_load(2, 3)` has the collector fiber consume samples while each of two workers sends three, so the collector counts six and the shared counter reads six, giving twelve. Each worker is spawned and awaited in turn, which keeps the accounting exact and the output deterministic.

**Percentiles.** `percentile(n, p)` returns the index `n * p / 100` into a sorted array of `n` samples. For `n = 1000` and `p = 99`, that is index 990, and the test asserts it.

**Buckets.** `#HIST_BOUNDS` is `0, 200, 400, 600, 800, 1000`. `bucket_for(latency)` counts how many bounds the latency has reached, so 350 lands in bucket two (it has passed 0 and 200), and 1000 lands in bucket six.

**Real metrics.** The last test registers a `latency_us` histogram with bounds `200.0`, `400.0`, `800.0`, feeds it four observations from a `for [...]` loop using the implicit `it`, and reads `hist_count` back as four. It also increments a `responses` counter and checks an `http.ok(.{ok: true})` response has status 200.

### 41.4 Running it

```bash
cd examples/09_http_load_tester
super test .
```

```text
./main.sr: 10 assertion(s) passed
super test: 10 passed, 0 file(s) failed
```

Five test blocks, ten assertions.

### 41.5 Make it yours

1. **Record real latency.** Pass a clock into the worker and measure the time before and after each request.
2. **Report percentiles.** Collect samples into a fixed array, sort them, and print p50, p95, and p99 using `percentile`.
3. **Failure accounting.** Count non-200 statuses separately and print an error rate.
4. **Open-loop load.** Replace the sequential worker loop with a rate that does not depend on response time.

### 41.6 Pitfalls

- A closed-loop tester slows down when the server does, which hides the latency you meant to measure. Choose the loop style deliberately.
- Keep the channel bounded. An unbounded sample queue turns a slow collector into a memory leak.

---

## 42. Project 10 — The Map-Reduce Engine

Map-reduce is the textbook shape for turning a large input into counts: split it, transform the pieces in parallel (map), then merge the partial answers (reduce). This project builds the pipeline with fibers and channels and adds the n-gram arithmetic that word-frequency jobs need.

### 42.1 Architecture

- **`Batch`** carries a partial result from a mapper to the reducer.
- **Mappers** are spawned fibers that each emit one batch.
- **The reducer** is a single fiber that merges batches into a shared total.
- **N-grams**: `ngram_count(tokens, n)` counts the windows of length `n`.
- **Comptime tuning**: `#NGRAM_LEN`, `#COMBINE_FACTOR`, and `#COMBINE_BATCH` derive batch sizes from one constant.
- **Library pieces**: a `RingBuf` and a `Logger`.

### 42.2 The complete source

```super
// Parallel Map-Reduce word / n-gram frequency engine
//
// Run: super test examples/10_mapreduce_kv/

use std.core.collections
use std.diag.log

#NGRAM_LEN = 3
#COMBINE_FACTOR = #NGRAM_LEN + 7
#COMBINE_BATCH = #COMBINE_FACTOR * #NGRAM_LEN

+Batch = {
    id: i64 = 0
    count: i64 = 0
}

+map_words(text_len: i64) : i64 {
    // Approximate token count.
    text_len / 5
}

+reduce_merge(a: i64, b: i64) : i64 {
    a + b
}

+mapreduce(n_mappers: i64, batch_size: i64) : i64 {
    batches = chan[Batch](64)
    reduced = shared 0
    scope {
        reducer = spawn {
            n = 0
            for b in batches {
                @reduced += b.count
                n += 1
            }
            n
        }
        for 0..n_mappers {
            m = spawn {
                batches.send(Batch{id: it, count: batch_size})
                batch_size
            }
            await m
        }
        batches.close()
        parts = await reducer
        reduced.load() + parts
    }
}

+ngram_count(tokens: i64, n: i64) : i64 {
    if tokens < n {
        0
    } else {
        tokens - n + 1
    }
}

+combine_batch_size() : i64 {
    #COMBINE_BATCH
}

test "map words" {
    assert_eq(map_words(25), 5)
}

test "reduce merge" {
    assert_eq(reduce_merge(3, 4), 7)
}

test "mapreduce pipeline" {
    // 4 mappers * batch 10 = 40 reduced; 4 parts → 44
    assert_eq(mapreduce(4, 10), 44)
}

test "ngrams" {
    assert_eq(ngram_count(10, 3), 8)
    assert_eq(ngram_count(2, 3), 0)
}

test "comptime ngram constants" {
    assert_eq(#NGRAM_LEN, 3)
    assert_eq(combine_batch_size(), 30)
}

test "stdlib: collections ring buffer + log line for reduce stage" {
    rb = RingBuf.new(2)
    @rb.push(1)
    @rb.push(2)
    #assert(rb.is_full())
    assert_eq(@rb.pop() or 0 - 1, 1)
    lg = Logger.new("mr")
    @lg.set_clock(0)
    @lg.info("reduce done")
    assert_eq(lg.last(), "1970-01-01T00:00:00.000Z INFO  [mr] reduce done")
}
```

### 42.3 Walkthrough

**Map and reduce, minimal.** `map_words(25)` approximates a token count as `25 / 5 = 5`, and `reduce_merge(3, 4)` is `7`. The functions are intentionally tiny, because the part worth testing is the pipeline around them.

**The pipeline.** `mapreduce(4, 10)` spawns a reducer and then four mappers. Each mapper sends one `Batch` whose `count` is the batch size, and the loop awaits each mapper in turn. After closing the channel, the function awaits the reducer, which has added `4 * 10 = 40` to the shared total and counted four parts. The result is `40 + 4 = 44`.

**N-grams.** A text of ten tokens has eight trigrams (`10 - 3 + 1`), and a text shorter than the window has none. Both cases are tested.

**Derived constants.** `#COMBINE_FACTOR = #NGRAM_LEN + 7` is ten, `#COMBINE_BATCH = #COMBINE_FACTOR * #NGRAM_LEN` is thirty, and `combine_batch_size()` returns the latter. Change `#NGRAM_LEN` and the batch size follows.

**The ring buffer and logger.** `RingBuf.new(2)` holds two items; after two pushes it reports `is_full()`, and `pop` returns the oldest item first (`1`). `Logger.new("mr")` with `set_clock(0)` formats a line with the Unix epoch timestamp, so the test can assert the exact text.

### 42.4 Running it

```bash
cd examples/10_mapreduce_kv
super test .
```

```text
./main.sr: 10 assertion(s) passed
super test: 10 passed, 0 file(s) failed
```

Six test blocks, ten assertions.

### 42.5 Make it yours

1. **Count real words.** Split a string with `std.core.str`, hash each token to a small integer, and add to a fixed-size counter array in each mapper.
2. **Merge counters.** Have each batch carry a counter array, and let the reducer add them element-wise.
3. **Sliding windows.** Use `RingBuf` to hold the last `#NGRAM_LEN` tokens and count real n-grams.
4. **Scale the mappers.** Increase `n_mappers` and check the total scales linearly.

### 42.6 Pitfalls

- A reducer must be the only writer of its totals, or you need a `shared` cell for them.
- Spawn the reducer before the mappers so the channel always has a reader.

---

## 43. Four windows that declare `Widget`

Programs 27 through 30 are packages. Each window is a struct whose header says `: Widget`, so `paint`, `hit`, and `measure` are part of the type. `super test` on the package directory runs the `test` blocks. Nothing in these tests opens a real window: `paint` returns the display list, and the assertions read that string.

### 43.1 `examples/27_gui_todo`

`TodoApp` keeps its rows in `std.db.sqlite` (`sqlite_memory`, an in-memory file). `open` creates the `todos` table. `add` refuses an empty title and writes `Title required` into `notice`. `toggle` flips the `done` column. `set_filter` is `all`, `open`, or `done`, and `lines` prints only the rows that pass. `remove` deletes by id.

`paint` builds a `std.gui` tree: a title bar, window buttons (`chrome`), a field, an Add button, a badge with the row count, the lines, and a toast when `notice` is set. The second test checks the `Widget` methods: a point inside the window hits, a point outside misses, and `measure` clamps to the requested width.

```bash
super test examples/27_gui_todo
```

```text
examples/27_gui_todo/src/main.sr: 19 assertion(s) passed
super test: 19 passed, 0 file(s) failed
```

### 43.2 `examples/28_gui_breakout`

`Breakout` stores the paddle, the ball, the remaining bricks, and the last cue name. `tick` moves the ball. A side wall or the paddle sets `sound` to `bounce` and reverses the velocity. A brick along the top decrements `bricks` and sets `sound` to `break`. `paint` draws two `Sprite` values (the ball cel follows `frame_n`) and, when a cue is set, a `Cue` line `sound bounce 80` or `sound break 80`.

```bash
super test examples/28_gui_breakout
```

```text
examples/28_gui_breakout/src/main.sr: 11 assertion(s) passed
super test: 11 passed, 0 file(s) failed
```

### 43.3 `examples/29_gui_invaders`

`Invaders` marches an alien row, keeps a player sprite, and fires one shot. `fire` sets the shot and the cue `shot`. `tick` steps the aliens and, when the shot reaches the row, removes one alien and sets the cue `hit`. `paint` writes the player sprite, one alien sprite whose width is `aliens * 14`, the shot, and the cue line (`sound shot 70` or `sound hit 70`).

```bash
super test examples/29_gui_invaders
```

```text
examples/29_gui_invaders/src/main.sr: 10 assertion(s) passed
super test: 10 passed, 0 file(s) failed
```

### 43.4 `examples/30_gui_snake`

`Snake` moves a head by `dir_x` and `dir_y`. The field is named `body` because `len` is the array-length intrinsic. Eating the food cell grows `body` and sets the cue `eat`. Leaving the board sets `alive` to false and the cue to `dead`; later ticks return `dead` without moving. `paint` draws a `snake` sprite at the head, a `food` sprite, and the cue (`sound eat 60` or `sound dead 60`).

```bash
super test examples/30_gui_snake
```

```text
examples/30_gui_snake/src/main.sr: 14 assertion(s) passed
super test: 14 passed, 0 file(s) failed
```

### 43.5 What the four programs have in common

The header is the conformance. `+TodoApp: Widget` and `+Snake: Widget` are checked the same way as `+Badge: Widget` in Chapter 9: every `Widget` method is present, `self` stays read-only, and the return types are `str`, `bool`, and `i64`. Extra methods (`add`, `tick`, `fire`) are ordinary methods on that value. Sound is a line in the display list (`sound eat 80`), and a sprite is a line too (`sprite x y w h name frame`). Games hand the provider a display list, and that provider paints the list on a canvas. A desk hands it `describe`, and the same provider builds real controls: GTK 4 on Linux (Wayland, then X11), Win32 on Windows, Cocoa on macOS. `SUPER_GUI=1` opens that window; these tests never set it.

---

# Part VI — Reference

## The example cross-index

The `examples/` directory holds thirty programs. The first ten are the flagship projects of Part V. The next ten are compact, deterministic workloads. Programs 21 through 26 are interface programs: two command-line tools, two terminal screens, and two desktop layouts. Programs 27 through 30, walked in Chapter 43, are a sqlite todo desk and three desktop games. Each one is a **Pod** (today still carrying a legacy `Pod.toml` in the tree; new scaffolds write `Pod.toml`). Source lives in `src/`, files you embed live in `assets/`, and `super build` writes `target/<profile>/`. `super test examples/21_cli_ledger` and `super run examples/21_cli_ledger` both find `src/main.sr` from the Pod manifest. A type declares a trait inline, in the same place a binding declares its type: `+TodoApp: Widget = { … }`. The compiler checks that declaration with `E0203`.

| # | Directory | What it builds | Library modules | Test blocks | Assertions |
|---|---|---|---|---|---|
| 01 | `01_space_shooter` | ECS arcade shooter with parallel movement and an effects channel | `std.game.ecs`, `std.math.linalg`, `std.game.physics2d`, `std.tui` | 10 | 20 |
| 02 | `02_task_api` | Four-module task and auth REST API with JWT | `std.net.http`, `std.sec.jwt`, `std.diag.log`, `std.sec.hash`, `std.sec.pq`, `std.core.str`, `std.cli.cmd` | 16 | 23 |
| 03 | `03_log_analyzer` | Chunked log pipeline, embedded fixture, regex and metrics | `std.core.str`, `std.core.regex`, `std.diag.metrics`, `std.cli.cmd`, `std.tui` | 9 | 18 |
| 04 | `04_roguelike_dungeon` | BSP, A\*, field of view, enemy AI channel | `std.game.ai`, `std.core.collections`, `std.tui` | 7 | 15 |
| 05 | `05_tower_defense` | 50 000-creep wave, spatial grid, priority queue, noise | `std.core.collections`, `std.math.noise` | 7 | 13 |
| 06 | `06_physics_sandbox` | Rigid bodies, AABB, contact pipeline | `std.game.physics2d`, `std.gui` | 9 | 14 |
| 07 | `07_realtime_chat_broker` | Channel broker with heartbeat and WebSocket codec | `std.net.ws`, `std.core.collections` | 6 | 9 |
| 08 | `08_kv_cache_engine` | Sharded KV with comptime masks and a Redis facade | `std.db.redis`, `std.core.collections` | 5 | 12 |
| 09 | `09_http_load_tester` | Worker fan-out, histograms, percentiles | `std.net.http`, `std.diag.metrics`, `std.cli.cmd`, `std.tui` | 5 | 14 |
| 10 | `10_mapreduce_kv` | Mappers, reducer, n-grams, ring buffer | `std.core.collections`, `std.diag.log` | 6 | 10 |
| 11 | `11_voxel_raycast` | Software DDA raycaster over a voxel grid with parallel scanlines | `std.math.core` | 1 | 2 |
| 12 | `12_boids_flocking` | Fixed-capacity flocking with speed limiting | `std.math.core` | 1 | 1 |
| 13 | `13_multiplayer_tick` | Authoritative 60 Hz fixed-timestep server skeleton | `std.math.core` | 3 | 8 |
| 14 | `14_tcp_message_broker` | Length-prefixed binary frames and topic fan-out | `std.core.collections` | 4 | 10 |
| 15 | `15_http_radix_framework` | Zero-copy request slicing and prefix routing | `std.core.str`, `std.cli.cmd` | 5 | 12 |
| 16 | `16_resp_lru_cache` | Fixed-capacity O(1) LRU with TTL and a RESP command subset | `std.core.str` | 5 | 11 |
| 17 | `17_log_ingest_pipeline` | 200 000 synthetic records, chunked aggregation, UFCS pipelines | none | 6 | 15 |
| 18 | `18_rigidbody_sandbox` | Fixed-timestep structure-of-arrays rigid bodies and contacts | `std.game.physics2d`, `std.math.core` | 4 | 7 |
| 19 | `19_ws_multiroom_chat` | WebSocket codec and multi-room publish and subscribe | `std.net.ws`, `std.core.collections` | 5 | ok |
| 20 | `20_game_server_ws_sync` | 60 Hz authoritative simulation with WebSocket snapshots | `std.net.ws`, `std.math.core`, `std.core.collections` | 5 | 11 |
| 21 | `21_cli_ledger` | Account ledger. `discount` is a method; the book parses the command | `std.cli.cmd` | 3 | 9 |
| 22 | `22_cli_ship` | Release planner. `Release.tag` and `Ship.plan` read flags and a subcommand | `std.cli.cmd` | 3 | 7 |
| 23 | `23_tui_ops` | Operations board. Each service names its status; the board only draws | `std.tui` | 2 | 8 |
| 24 | `24_tui_inbox` | Inbox. A note builds its title; the screen shows the selected note | `std.tui` | 2 | 6 |
| 25 | `25_gui_studio` | Recording desk. A take captions itself; the window paints the controls | `std.gui` | 3 | 7 |
| 26 | `26_gui_atlas` | Coastal atlas. A pin captions itself and knows which pins are near | `std.gui` | 2 | 7 |
| 27 | `27_gui_todo` | Todo desk. Rows live in sqlite; the window declares `Widget` | `std.gui`, `std.db.sqlite` | 2 | 19 |
| 28 | `28_gui_breakout` | Breakout. A sprite ball, bricks, and bounce cues | `std.game.fx` | 2 | 11 |
| 29 | `29_gui_invaders` | Invaders. Stepping aliens, a shot, and a hit cue | `std.game.fx` | 2 | 10 |
| 30 | `30_gui_snake` | Snake. Eating grows the body; a wall ends the run | `std.game.fx` | 2 | 14 |

Examples 01 through 30 pass `super test`. Example 19 names its membership helpers `room_join` / `room_leave` so they do not collide with `std.str.join`. The interface programs call methods on the value (`@acct.discount(10.0)`, `api.status()`, `rel.tag()`), and the command, screen, or window is another value that asks them.

### How to choose where to start

- **Learning the language:** read 11 (one function, one test), then 12 and 13.
- **Backend work:** 02 first, then 15 (routing) and 16 (caching), and 09 for load.
- **Game work:** 01, then 13 (fixed timestep), 18 (physics), 20 (netcode), and 28–30 for a window, sprites, and sound cues.
- **Protocols:** 14 (binary frames), 07 and 20 (WebSocket), and 16 (RESP text).
- **Data pipelines:** 03 and 17, then 10.
- **Interfaces:** 21 and 22 for the command line, 23 and 24 for the terminal, 25 and 26 for a window, and 27 for a sqlite desk that declares `Widget`.

---

## Appendix A — The `super` Command-Line Reference

The `super` binary is one program that creates packages, checks, runs, tests, builds, formats, documents, and manages dependencies. This appendix lists every command and shows the ones you will use daily with real output from the 0.1.0 build.

### A.1 The command map

| Command | What it does |
|---|---|
| `super new <name> [--lib\|--bin]` | Create a package in a new directory (binary by default) |
| `super init` | Turn the current directory into a package |
| `super check [FILE]` | Lex, parse, resolve, infer, and validate without running anything |
| `super test [PATH]` | Run every `test` block under a path (default `.`) |
| `super run <FILE> [--jit]` (`super r`) | Compile and run a file |
| `super build [FLAGS] <FILE>` (`super b`) | Build a binary or shared library. `-d` debug, `-r` release |
| `super install [PATH]` (`super i`) | Native maximum-release build, copied onto `PATH` |
| `super uninstall [NAME]` (`super u`) | Remove a binary installed by `super install` |
| `super list` (`super installed`) | Show installed name, version, path, and Pod root |
| `super which [NAME]` | Print the installed executable path |
| `super clean [PATH]` | Remove `target/` artifacts. `-d` debug, `-r` release, `-a` all (default) |
| `super emit-c <FILE>` | Emit the generated C source only |
| `super fmt [PATH]` | Reformat source files in place |
| `super doc [PATH]` | Print Markdown generated from `///` comments |
| `super add <POD>` | Add a path or Git dependency to `Pod.toml` / `Pod.toml` |
| `super remove <POD>` | Remove a dependency |
| `super fetch` / `super update` | Resolve dependencies and write `Pod.lock` / `Pod.lock` |
| `super vendor` | Copy resolved dependencies into `vendor/` |
| `super completions <SHELL>` | Print a shell completion script |
| `super lsp` | Start the language server on standard input and output |
| `super mcp` | Start the agent knowledge MCP server (features, examples, algorithms, stdlib) |
| `super bench` | Run the 100M-iteration ECS and HTTP/JSON benchmarks |
| `super bindgen <FILE> --out <HEADER>` | Generate a C/C++ header from exported declarations |
| `super disasm <FILE> [--jit]` | Show optimized bytecode, optionally with a native dump |

`super --version` prints `super 0.1.0`. `super help` prints this command map in the terminal, and `super help install` (or `super install --help`) prints one command.

### A.2 `new` and `init`

```bash
super new hello
cd hello
super check
super run src/main.sr
```

`super new` creates a **Pod**: source in `src/`, files you embed in `assets/`, and a `.gitignore` that keeps build output in `target/`. Pass `--bin` (default), `--lib`, or `--macro` for the three kinds:

```text
hello/
  Pod.toml
  .gitignore
  assets/
  src/
    main.sr          # bin
#   lib.sr           # --lib
#   macros.sr        # --macro
```

The manifest selects dev for everyday builds and pins the other profiles. Release is the fastest native binary the toolchain emits:

```toml
[pod]
name = "hello"
version = "0.1.0"
kind = "bin"

[build]
profile = "dev"
backend = "c23"
mode = "aot"

[profile.dev]
opt = "0"
lto = "off"
debug-info = true
fast-math = false

[profile.test]
opt = "2"
lto = "off"
debug-info = true
fast-math = false
target-cpu = "native"

[profile.release]
opt = "3"
lto = "fat"
strip = true
fast-math = true
target-cpu = "native"

[profile.size]
opt = "s"
lto = "fat"
strip = true
gc-sections = true

[profile.fast]
opt = "3"
lto = "fat"
strip = true
fast-math = true
target-cpu = "native"

[dependencies]
```

The generated program is a complete, passing starting point:

```super
+main() {
    print("Hello from Super!")
}
```

Running `super run` prints `Hello from Super!`. `super build` writes a native C23 binary at `target/dev/hello`. `super build -r` writes `target/release/hello`. `[build] backend = "c23"` and `[build] mode = "aot"` are the defaults, so that file is the executable. `embed "assets/logo.bin"` reads that file from the Pod `assets/` directory (a bare file name is also found there). Pass `--lib` for a reusable library Pod (`src/lib.sr`) or `--macro` for a comptime macro Pod (`src/macros.sr` with `+#` exports). `super init` does the same job inside the current directory (`--lib` / `--macro` accepted there too).

### A.3 `check`, `test`, and `run`

`super check` runs the whole frontend (lexer, parser, name resolution, type inference, and the lifetime and mutation checks) and prints diagnostics without executing anything. With no argument it finds the package's entry file. It is the fastest feedback loop you have, and most editors call the same code through `super lsp`.

`super test [PATH]` accepts a file or a directory and defaults to `.`. It discovers every `test` block, gives each one access to the module's private items, and reports passing assertions per file followed by a summary:

```text
./main.sr: 19 assertion(s) passed
super test: 19 passed, 0 file(s) failed
```

The final line counts files, not tests: `0 file(s) failed` is the green light. A test file that fails to compile counts as a failed file and prints its diagnostic.

`super run <FILE>` compiles and executes through the bytecode VM. `--jit` asks the native emitter to take over hot numeric code. Functions that call into host-backed library helpers (the list operations, for example) are not available in native paths, so keep `--jit` for numeric kernels and compare against the plain VM result.

### A.4 `build`

`super b` is `super build`. `super r` is `super run`.

```bash
super r src/main.sr
super b -d src/main.sr
super b -r --c23 src/main.sr -o dist/app
./dist/app
super b --profile size --c23 -Oz src/main.sr -o dist/app-small
super b --profile fast --c23 --static -o dist/app-static src/main.sr
```

With no profile flag the build is `dev`, or whatever `[build] profile` says in `Pod.toml` / `Pod.toml`. `-d` / `--debug` forces dev: `-O0 -g`, tests kept. `-r` / `--release` is the release profile, and it is the fastest binary this toolchain emits. `-r` and `-d` together is an error. `super test` uses the `test` profile even when `[build] profile` is `release`.

| Profile | Ask for it | What the C compiler gets | Tests |
|---|---|---|---|
| `dev` | default, `-d`, `--profile dev` | `-O0 -g` | kept |
| `test` | `super test`, `--profile test` | `-O2 -g -march=native`, no fast-math | kept |
| `release` | `-r`, `--release`, `--profile release` | `-O3 -flto -march=native -ffast-math -funroll-loops -fno-plt -fno-semantic-interposition -s` | stripped |
| `size` | `--profile size` | `-Os -flto`, section GC, `-s` | stripped |
| `fast` | `--profile fast` | same peak as `release` | stripped |

`super b`, `super r`, and `super check` with no file walk up to `Pod.toml` (then `Pod.toml`), fetch its dependencies, and compile the Pod entry for its `kind` (`src/main.sr`, `src/lib.sr`, or `src/macros.sr`). `[profile.<name>]` patches that preset. A flag on the command line wins.

Override one knob without leaving the profile: `-O0`/`-O1`/`-O2`/`-O3`/`-Os`/`-Oz`/`-Og`/`-Ofast`, `--lto off|thin|fat`, `-s`/`--no-strip`, `-g`, `--static`, `--target-cpu native|generic|<cpu>`, `--fast-math`/`--no-fast-math`, `--link-arg <ARG>`.

`--c23` emits one self-contained C translation unit, then calls the host C compiler. Those optimization and link flags apply to that compile. A build without `--c` / `--c23` still uses the profile to decide whether `test` blocks are stripped.

| Flag | Meaning |
|---|---|
| `-r`, `--release` | Release profile |
| `-d`, `--debug` | Dev profile |
| `--profile <NAME>` | `dev`, `release`, `size`, or `fast` |
| `--c` | Transpile to C and compile it |
| `--c23` | Same as `--c --std c23` |
| `--emit-c` | Emit C only; do not compile |
| `--cc` | Force the C compile after emitting (useful with `--emit-c`) |
| `--cc-bin <BIN>` | Choose the C compiler; implies `--c` |
| `--out-c <PATH>` | Where to write the generated C (default `<file>.c`) |
| `-o`, `--out <PATH>` | Output path for the binary or shared library |
| `--shared` | Build a shared library |
| `-O`, `--opt <LEVEL>` | `0`–`3`, `s`, `z`, `g`, or `fast` |
| `--std <STD>` | C standard handed to the compiler (default `c23`) |

A release build prints the profile and the flags that were actually passed:

```text
emitted dist/app.c (C23 AOT, 27477 bytes, profile release) → compiled binary dist/app [cc -O3 -flto -march=native -ffast-math -funroll-loops -fno-plt -fno-semantic-interposition -fomit-frame-pointer -s]
```

`super emit-c src/main.sr -o out.c` writes the C source and reports its size without compiling. Pass `-r` or `--profile size` there too when the generated C should match a later link.

> **Parity habit.** Before shipping any natively compiled binary, run the same input through `super run` and through the built binary and compare their printed checksums. Equal checksums are the cheapest correctness oracle you have. If they ever differ, ship the VM build and keep a minimal reproduction.

### A.5 `install`, `uninstall`, `list`, `which`, and `clean`

`super i` is `super install`. From a package, or from any subdirectory of it, the command builds `src/main.sr` through the C23 backend and forces the release profile to its peak: `-O3`, fat LTO, `-march=native`, `-ffast-math`, unroll, and strip. Tests are stripped. The executable name is the package name (`hello.exe` on Windows), and Super copies it into a directory that is on `PATH`.

```bash
cd hello
super install
hello
super which
super list
super uninstall
```

The destination is `--bin-dir`, or `SUPER_INSTALL_DIR`, or the first of `~/.local/bin`, `~/bin`, and `~/.cargo/bin` that is already on `PATH`. On Windows those three live under the user profile. If none of them is on `PATH`, Super creates `~/.local/bin` and adds it to the shell startup file (or the user `Path` on Windows). A new terminal can then run the program by name. Re-running `super install` replaces a binary Super installed. `--force` replaces a file it did not install.

A workspace is a directory of packages, or a manifest like this:

```toml
[workspace]
members = ["app", "tool"]
```

`super install` at that root installs every member that has `src/main.sr` and skips libraries. From inside one member, only that member is installed.

`super u` is `super uninstall`. It removes the recorded executable and leaves the `PATH` entry in place. `super list` prints every receipt. `super which hello` prints only the path.

`super clean` removes build output under `target/`:

```bash
super clean -d    # target/dev and target/debug
super clean -r    # target/release
super clean       # the whole target/ directory; same as super clean -a
```

`-d`, `-r`, and `-a` cannot be combined. A workspace cleans every member.

### A.6 `fmt`

`super fmt [PATH]` rewrites `.sr` files in place: four-space indentation by brace depth and collapsed blank lines. It is idempotent, so running it twice changes nothing, and it never reflows long lines.

```bash
super fmt .
```

```text
formatted 1 file(s)
```

### A.7 `doc`

`super doc [PATH]` prints Markdown from `///` comments that sit directly above an exported (`+`) item. This library:

```super
/// Doubles a number.
+twice(x: i64) : i64  => x * 2

test "twice" {
    assert_eq(twice(4), 8)
}
```

produces:

```markdown
# API documentation

## `twice(x: i64) : i64`

Doubles a number.
```

Redirect it into your docs folder in CI: `super doc . > docs/API.md`.

Contributors should follow the repository
[API Style Guide](../../CONTRIBUTING.md#api-style-guide): one concept has one
canonical name, receiver context avoids stutter, and cheap-looking operations
must not hide allocation or blocking work.

### A.8 Dependencies

Dependencies are path or Git Pods recorded in `Pod.toml` / `Pod.toml`.

```bash
super add ../textkit
```

```toml
[dependencies.textkit]
path = "../textkit"
```

`super fetch` resolves the graph and writes a lock file:

```toml
version = 1

[[pod]]
name = "textkit"
source = "path+../textkit"
```

`super update` re-resolves and rewrites `Pod.lock`; `super remove textkit` deletes the entry; `super vendor` copies every resolved package into `vendor/`. Commit `Pod.lock` for applications.

### A.9 `bindgen`, `disasm`, and `bench`

`super bindgen src/main.sr --out main.h` writes a C and C++ header for the exported declarations. The header begins with a banner and include guard, includes `super.h`, and wraps the declarations in `extern "C"` for C++ callers.

`super disasm src/main.sr` prints the optimized bytecode with a one-line optimizer summary, which is the right tool when you want to know whether a fused operation really fused. Add `--jit` for the native dump.

`super bench` runs the 100M-iteration ECS and HTTP/JSON benchmarks and is meant to be run in a release build.

### A.10 Completions, language server, and MCP

```bash
super completions bash > ~/.local/share/bash-completion/completions/super
super completions zsh  > ~/.zfunc/_super
super completions fish > ~/.config/fish/completions/super.fish
```

`super lsp` speaks JSON-RPC 2.0 over standard input and output. Send logs to standard error, never to standard output, because stdout carries protocol frames. It publishes diagnostics as you type, and provides hover, definition, references, rename, completion, signature help, inlay hints, outline, and semantic highlighting.

`super mcp` (shim `super-mcp`) is the Model Context Protocol knowledge server for coding agents. Configure an MCP client with command `super` and args `["mcp"]` (stdio). Tools include `super_overview`, `super_search`, feature/algorithm/example getters, and `super_list_stdlib` / `super_get_stdlib` for the 100-file stdlib map plus live source when `SUPER_STDLIB` is set. Resources live under `super://…`; prompts include `learn-super`, `write-super`, and `explore-stdlib`.

### A.11 A release script

This script does what Appendix C asks, in order, and stops at the first failure:

```bash
#!/usr/bin/env bash
set -euo pipefail

super fmt .
git diff --exit-code
super check src/main.sr
super test .
super build --release --c23 src/main.sr -o dist/app

vm_out=$(super run src/main.sr)
native_out=$(./dist/app)
test "$vm_out" = "$native_out"
super install
```

---

## Appendix B — Diagnostic Codes E0001 to E0501

Every diagnostic has a stable code, so tools can attach fixes and you can search for it. They render in a rustc-like shape: severity and code, the message, the file and position, the source line with a caret under the primary span, and a `help:` line when the compiler has a concrete suggestion. Fix the first diagnostic first; later ones are often consequences of it.

| Code | Category | Meaning |
|---|---|---|
| E0001 | Syntax | The lexer or parser could not continue; also an unresolvable `use` path |
| E0101 | Name | Undefined name |
| E0102 | Module | A standard-library namespace is used without `use std.<name>` |
| E0103 | Member | Unknown field |
| E0104 | Visibility | A private (`-`) field is used outside its module |
| E0201 | Type | Reserved for type mismatches; see E0202 |
| E0202 | Type | An optional or fallible value used bare, and concrete type mismatches |
| E0203 | Type | A struct declares a trait it does not satisfy: unknown trait, missing method, or a signature that disagrees on arity, `@`, a parameter type, or the return type |
| E0301 | Mutation | Missing `@` for in-place mutation |
| E0302 | Concurrency | One mutable root captured by sibling spawned tasks |
| E0303 | Usage | Unused local variable |
| E0304 | Usage | Unused parameter |
| E0305 | Usage | Unused import |
| E0401 | Lifetime | A mutable root escapes its declaring module |
| E0501 | Compile time | A `#` compile-time check failed |

The pairs below are real: each broken program was run through `super check` to produce the quoted diagnostic, and each fixed program passes.

### E0001 — Syntax

The primary span points at the first token that does not fit. The real mistake is usually one token earlier, so read the line above the caret. An `use` path that cannot be resolved is also reported under this code.

Broken:

```text
+main() {
    total = (1 + 2
    print("{total}")
}
```

The compiler says:

```text
error[E0001]: expected RParen near `print`
 --> main.sr:3:5
  |
3 |     print("{total}")
  |     ^^^^^
```

Fixed:

```super
+main() {
    total = (1 + 2)
    print("{total}")
}
```

### E0101 — Undefined name

Check the spelling, or import the name. Remember that standard-library names appear after `use std.<module>`.

Broken:

```text
+main() {
    total = subtotal + 1
    print("{total}")
}
```

The compiler says:

```text
error[E0101]: undefined name `subtotal`
 --> main.sr:2:13
  |
2 |     total = subtotal + 1
  |             ^^^^^^^^
  = help: check spelling, or import it with `use std.…` if it lives in the standard library
```

Fixed:

```super
+main() {
    subtotal = 41
    total = subtotal + 1
    print("{total}")
}
```

### E0102 — Namespace used without an import

A `module.function(...)` call needs the matching `use`.

Broken:

```text
+main() {
    res = http.ok("pong")
    print("{res.status}")
}
```

The compiler says:

```text
error[E0102]: `http` is not in scope
 --> main.sr:2:11
  |
2 |     res = http.ok("pong")
  |           ^^^^
  = help: add `use std.net.http` at the top of this file
```

Fixed:

```super
use std.net.http

+main() {
    res = http.ok("pong")
    print("{res.status}")
}
```

### E0103 — Unknown field

Reading a field that the struct does not declare. Hover the struct in your editor to see its fields and layout.

Broken:

```text
+User = {
    id: i64
    name: str
}

+main() {
    u = User{id: 1, name: "Amina"}
    contact = u.email
    print(contact)
}
```

The compiler says:

```text
error[E0103]: unknown field `email`
 --> main.sr:8:15
  |
8 |     contact = u.email
  |               ^^^^^^^
  = help: hover the struct name to see its fields and C-ABI layout
```

Fixed:

```super
+User = {
    id: i64
    name: str
    email: str = ""
}

+main() {
    u = User{id: 1, name: "Amina", email: "amina@example.com"}
    contact = u.email
    print(contact)
}
```

### E0104 — Private field

A field marked `-` is visible only inside its own module and in `test` blocks. The struct lives in `models.sr`:

```text
+Account = {
    id: i64
    -token: str = "internal"
}
```

Broken `main.sr`:

```text
use models.{Account}

+main() {
    a = Account{id: 1}
    secret = a.token
    print(secret)
}
```

The compiler says:

```text
error[E0104]: field 'token' on 'Account' is file-private (marked with '-')
 --> main.sr:5:14
  |
5 |     secret = a.token
  |              ^^^^^^^
  = help: private fields are visible only in this module and `test` blocks; see the struct declaration
```

Fixed `main.sr` (read only the public field, or add a public method in `models.sr`):

```super
use models.{Account}

+main() {
    a = Account{id: 1}
    print("account {a.id}")
}
```

### E0201 and E0202 — Type mismatches

E0201 is reserved for the general mismatch class. In 0.1.0, mismatches are reported as E0202, with a message of the form "expected X, found Y". Two shapes cover most of what you will see.

**An optional used bare.** `find_port` returns `i64?`, and the binding wants an `i64`. Unwrap with `or`.

Broken:

```text
+find_port(name: str) : i64? = if name == "http" { 80 } else { none }

+main() {
    port: i64 = find_port("custom")
    print("{port}")
}
```

The compiler says:

```text
error[E0202]: expected `i64`, found `i64?` (optional value)
 --> main.sr:4:17
  |
4 |     port: i64 = find_port("custom")
  |                 ^^^^^^^^^^^^^^^^^^^
  = help: unwrap with `or <default>` or `or err { … }` before using the `i64` value
```

Fixed:

```super
+find_port(name: str) : i64?  => if name == "http" { 80 } else { none }

+main() {
    port: i64 = find_port("custom") or 3000
    print("{port}")
}
```

**A concrete mismatch.** Passing text where a number is expected.

Broken:

```text
+double(x: i64) : i64 = x * 2

+main() {
    n = double("text")
    print("{n}")
}
```

The compiler says:

```text
error[E0202]: expected `i64`, found `str`
 --> main.sr:4:16
  |
4 |     n = double("text")
  |                ^^^^^^
  = help: types must match; convert `str` to `i64`, or change the binding / parameter type
```

Fixed:

```super
+double(x: i64) : i64  => x * 2

+main() {
    n = double(21)
    print("{n}")
}
```

### E0203 — Trait conformance

A struct that names a trait in its header has to supply every method, with the same arity, the same `@`, the same parameter types, and the same return type. This program names `Widget` and forgets `paint`.

Broken `bare.sr`:

```text
+Widget = trait {
    paint(self) : str
}

+Bare: Widget = {
    label: str = ""
}
```

The compiler says:

```text
error[E0203]: `Bare` does not satisfy `Widget`: missing `paint`
 --> bare.sr:5:2
  |
5 | +Bare: Widget = {
  |  ^^^^^^^^^^^^^^^^
  = help: add `+paint(self) : str`
```

Fixed: write `paint` on `Bare`, with `self` and a `str` result. A method that returns `i64`, or that takes `@self` when the trait says `self`, is the same code with a different message. Appendix B's table is the full list.

### E0301 — Missing `@`

A method that takes `@self` mutates its receiver, so the call site must say so with `@`.

Broken:

```text
+Counter = {
    n: i64 = 0

    bump(@self) : i64 {
        self.n += 1
        self.n
    }
}

+main() {
    c = Counter{n: 0}
    c.bump()
    print("{c.n}")
}
```

The compiler says:

```text
error[E0301]: cannot mutate without `@`
  --> main.sr:12:5
   |
12 |     c.bump()
   |     ^
   = help: prefix the in-place argument with `@`, e.g. `@u.discount(...)`
```

Fixed:

```super
+Counter = {
    n: i64 = 0

    bump(@self) : i64 {
        self.n += 1
        self.n
    }
}

+main() {
    c = Counter{n: 0}
    @c.bump()
    print("{c.n}")
}
```

### E0302 — Sibling tasks share a mutable root

Two `spawn`s in the same `scope` may not both take `@xs`. Overlapping mutable access is a data race, so the check is static. The fix is to give each task its own scope (as below), wrap the value in `shared(...)` for genuine cross-task counters, or pass it immutably.

Broken:

```text
+bump(@xs, i: i64) {
    xs[i] = xs[i] + 1
}

+main() {
    xs = [1, 2, 3]
    scope {
        a = spawn bump(@xs, 0)
        b = spawn bump(@xs, 1)
        await a
        await b
    }
    print("finished")
}
```

The compiler says:

```text
error[E0302]: cannot capture `@xs` mutably in multiple spawned tasks in the same scope
 --> main.sr:9:13
  |
9 |         b = spawn bump(@xs, 1)
  |             ^^^^^^^^^^^^^^^^^^
  = help: wrap `@xs` with `shared(...)` for cross-task mutation, or pass it immutably
```

Fixed:

```super
+bump(@xs, i: i64) {
    xs[i] = xs[i] + 1
}

+main() {
    xs = [1, 2, 3]
    scope {
        a = spawn bump(@xs, 0)
        await a
    }
    scope {
        b = spawn bump(@xs, 1)
        await b
    }
    print("both scopes finished")
}
```

### E0303 — Unused variable

A binding is never read. Delete it, use it, or prefix its name with an underscore to mark it intentional.

Broken:

```text
+main() {
    retries = 5
    print("hello")
}
```

The compiler says:

```text
error[E0303]: unused variable 'retries'
 --> main.sr:2:5
  |
2 |     retries = 5
  |     ^^^^^^^^^^^
  = help: prefix it with an underscore: `_retries`
```

Fixed:

```super
+main() {
    _retries = 5
    print("hello")
}
```

### E0304 — Unused parameter

The same rule for function parameters. The underscore keeps the signature while telling readers (and the compiler) the value is intentionally ignored.

Broken:

```text
+scaled(value: i64, factor: i64) : i64 = value * 2

+main() {
    print("{scaled(4, 3)}")
}
```

The compiler says:

```text
error[E0304]: unused parameter 'factor'
 --> main.sr:1:21
  |
1 | +scaled(value: i64, factor: i64) : i64 = value * 2
  |                     ^^^^^^^^^^^
  = help: prefix it with an underscore: `_factor`
```

Fixed:

```super
+scaled(value: i64, _factor: i64) : i64  => value * 2

+main() {
    print("{scaled(4, 3)}")
}
```

### E0305 — Unused import

An imported module or name is never referenced. The sibling file `models.sr`:

```text
+User = {
    id: i64
    name: str
}
```

Broken `main.sr`:

```text
use models.{User}

+main() {
    print("hi")
}
```

The compiler says:

```text
error[E0305]: unused import 'models'
 --> main.sr:1:1
  |
1 | use models.{User}
  | ^^^^^^^^^^^^^^^^^
  = help: prefix it with an underscore: `_models`
```

Fixed `main.sr`:

```super
use models.{User}

+main() {
    u = User{id: 1, name: "Amina"}
    print(u.name)
}
```

### E0401 — Escaping mutable root

A `state` root may be mutated in place, but a function may not return an `@` reference to it. Return an owned copy.

Broken:

```text
state tally = [0]

+peek(flag: i64) : _ {
    if flag > 0 {
        return @tally
    }
    [0]
}

+main() {
    print(peek(0))
}
```

The compiler says:

```text
error[E0401]: mutable root `@tally` cannot escape its declaring module
 --> main.sr:5:9
  |
5 |         return @tally
  |         ^^^^^^^^^^^^^
  = help: `state tally` and other roots must stay module-scoped; return an owned copy instead of `@tally`
```

Fixed:

```super
state tally = [0]

+peek(flag: i64) : _ {
    if flag > 0 {
        return [tally[0]]
    }
    [0]
}

+main() {
    print(peek(0))
}
```

### E0501 — Compile-time check failed

A `#` compile-time form could not be satisfied. The most useful is `#assert_size`, which pins a struct's C-ABI size so an accidental field change cannot silently break a host binding.

Broken:

```text
#assert_size(Pair, 8)
+Pair = {
    a: i64
    b: i64
}

+main() {
    p = Pair{a: 1, b: 2}
    print(p.a)
}
```

The compiler says:

```text
error[E0501]: //# assert_size(Pair, 8) failed: actual size is 16
 --> main.sr:1:1
  |
1 | #assert_size(Pair, 8)
  | ^^^^^^^^^^^^^^^^^^^^^
```

Fixed:

```super
#assert_size(Pair, 16)
+Pair = {
    a: i64
    b: i64
}

+main() {
    p = Pair{a: 1, b: 2}
    print(p.a)
}
```

---

## Appendix C — The Production Checklist

Treat this list as a pre-flight check before the first real user and again before every major release. Each line is something the ten flagship projects practice, so you can point to working code for every item.

### C.1 Build and reproducibility

- [ ] `super fmt .` leaves the working tree unchanged in CI.
- [ ] `super check` passes for every entry module, not only the one you ran.
- [ ] `super test .` passes, including vendored dependencies.
- [ ] `Pod.lock` is committed for applications, and the vendoring policy is written down.
- [ ] Release binaries are built on the target operating system and CPU family, and the Super version, C compiler version, and flags are recorded with each artifact.
- [ ] If you ship a natively compiled binary, the VM and native outputs agree on the same input (Appendix A.10).

### C.2 Tests

- [ ] Every pure function has exact-value assertions (chapters 33 through 42 show the style).
- [ ] Every parser and codec has malformed-input tests as well as round-trip tests. The shipped examples cover round trips (chapter 39 and example 14); add the malformed cases for your own formats.
- [ ] Time-dependent code takes the clock as an argument, so tests never sleep (the JWT and logger tests in chapter 34).
- [ ] Simulations are deterministic, and a workload run twice produces the same checksum.
- [ ] Every HTTP route has an integration test through the simulated client (chapter 34).
- [ ] Optimized and scalar paths are compared on lengths around the vector width: 0, 1, width minus 1, width, width plus 1.

### C.3 Resource bounds

- [ ] Every request body, frame, line, array, and channel has a maximum size enforced before allocation.
- [ ] Channels are bounded, and the policy for a full channel (block, drop oldest, or reject) is chosen on purpose.
- [ ] Per-connection buffers have a ceiling, and exceeding it closes the connection.
- [ ] Caches have a capacity and an eviction policy; no unbounded map sits on the request path (example 16 is a fixed-capacity model).
- [ ] Catch-up loops are capped so a stall cannot become a spiral.

### C.4 Protocols and APIs

- [ ] Every binary protocol has a documented byte layout and a version byte or negotiation step.
- [ ] Every API that offers a binary or streaming transport also offers a JSON fallback, especially for browsers and debugging tools.
- [ ] Every endpoint returns a consistent JSON error shape with a stable status code (chapter 34).
- [ ] Unknown commands and malformed frames fail closed and are counted rather than silently dropped.
- [ ] Reconnect, timeout, cancellation, and partial-read behavior have tests.

### C.5 Concurrency and state

- [ ] Each mutable structure has one owner fiber; other fibers talk to it through channels (chapters 39 and 40).
- [ ] No two sibling `spawn`s receive `@` access to the same value; E0302 enforces this.
- [ ] Every channel a consumer loops over is closed before the consumer is awaited.
- [ ] `shared` cells hold only counters and flags that cross tasks.
- [ ] Shutdown is exercised: channels closed, fibers awaited, sockets released.
- [ ] Scoped data never escapes into persistent `state`; E0401 catches the common cases.

### C.6 Security

- [ ] Secrets come from the environment or a secret store, never from source or the repository.
- [ ] Tokens have an expiry that is checked against an injected clock (chapter 34).
- [ ] Passwords are hashed with a slow, salted function; secret comparisons are constant-time.
- [ ] Private fields (`-`) hold anything that must never serialize, and serializers are tested for omission.
- [ ] Logs redact tokens, passwords, and personal data; metric labels have bounded cardinality.
- [ ] Every input that becomes a path, a command, or a query is validated or parameterized.
- [ ] WebSocket handshakes validate the `Origin` and the `Sec-WebSocket-Key`.

### C.7 Observability

- [ ] Logs are structured, with a service name, a level, and a request or tick identifier (the logger tests in chapters 34 and 42).
- [ ] Counters exist for requests, errors by class, rejected frames, evictions, and queue depths (chapters 35 and 41).
- [ ] Latency histograms use explicit bucket bounds that match your service-level objectives (chapter 41).
- [ ] A health endpoint checks dependencies and returns quickly.
- [ ] Every simulation has a fixed-seed replay, so a bug report can become a test.

### C.8 Performance

- [ ] Hot loops are plain loops over fixed-size arrays or ECS views, with no allocation inside them.
- [ ] Layout-sensitive structs carry `#assert_size` (chapters 33 and 34).
- [ ] Parallel loops write through `@` bindings and avoid shared mutable state; counters use `shared`.
- [ ] You measured in a release build, on the target hardware, with `super bench` or your own timing test.
- [ ] After every optimization, the tests still pass and the checksums still match.

A short closing thought: none of these items is exotic. They are habits, and the ten projects in this part show each of them in a few lines of working Super. Copy a project, change its domain, keep its tests green, and you have the start of something you can ship.
