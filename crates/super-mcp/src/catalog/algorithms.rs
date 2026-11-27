//! Algorithm cookbook in idiomatic Super.

use super::Algorithm;

pub const ALL: &[Algorithm] = &[
    Algorithm {
        id: "binary-search",
        title: "Binary search",
        category: "search",
        summary: "Lower-bound binary search over a sorted i64 array.",
        code: r#"bin_search(xs: [i64], target: i64) : i64 {
    lo = 0
    hi = xs.len
    while lo < hi {
        mid = lo + (hi - lo) / 2
        if xs[mid] < target { lo = mid + 1 } else { hi = mid }
    }
    if lo < xs.len && xs[lo] == target { lo } else { -1 }
}

test "binary search finds pivot" {
    xs = [1, 3, 5, 7, 9]
    assert_eq(bin_search(xs, 7), 3)
    assert_eq(bin_search(xs, 2), -1)
}
"#,
        tags: &["search", "array", "log-n"],
    },
    Algorithm {
        id: "quicksort",
        title: "In-place quicksort",
        category: "sort",
        summary: "Hoare-partition quicksort mutating an array with @.",
        code: r#"partition(@xs: [i64], lo: i64, hi: i64) : i64 {
    pivot = xs[hi]
    i = lo
    for lo..hi {
        if xs[it] <= pivot {
            xs[i], xs[it] = xs[it], xs[i]
            i += 1
        }
    }
    xs[i], xs[hi] = xs[hi], xs[i]
    i
}

qsort(@xs: [i64], lo: i64, hi: i64) {
    ensure lo < hi else { return }
    p = @partition(@xs, lo, hi)
    @qsort(@xs, lo, p - 1)
    @qsort(@xs, p + 1, hi)
}

sort(@xs: [i64]) { @qsort(@xs, 0, xs.len - 1) }
"#,
        tags: &["sort", "quicksort", "mutation"],
    },
    Algorithm {
        id: "bfs",
        title: "Breadth-first search",
        category: "graph",
        summary: "BFS distances on an adjacency list.",
        code: r#"bfs(adj: [[i64]], start: i64) : [i64] {
    n = adj.len
    dist = [0].repeat(n).map(it * 0 - 1)
    dist[start] = 0
    q = [start]
    head = 0
    while head < q.len {
        u = q[head]
        head += 1
        for adj[u] {
            v = it
            if dist[v] < 0 {
                dist[v] = dist[u] + 1
                q = q + [v]
            }
        }
    }
    dist
}
"#,
        tags: &["graph", "bfs", "queue"],
    },
    Algorithm {
        id: "dijkstra",
        title: "Dijkstra shortest paths",
        category: "graph",
        summary: "Binary-heap Dijkstra on weighted edges (u, v, w).",
        code: r#"// edges: list of {to: i64, w: i64} per node
dijkstra(adj: [[{to: i64, w: i64}]], src: i64) : [i64] {
    n = adj.len
    INF = 1_000_000_000
    dist = [0].repeat(n).map(it * 0 + INF)
    dist[src] = 0
    // naive O(n^2) extract-min for clarity; use a heap in production
    done = [false].repeat(n)
    for 0..n {
        u = -1
        best = INF
        for 0..n {
            if !done[it] && dist[it] < best {
                best = dist[it]
                u = it
            }
        }
        ensure u >= 0 else { break }
        done[u] = true
        for adj[u] {
            e = it
            nd = dist[u] + e.w
            if nd < dist[e.to] { dist[e.to] = nd }
        }
    }
    dist
}
"#,
        tags: &["graph", "shortest-path", "dijkstra"],
    },
    Algorithm {
        id: "astar",
        title: "A* pathfinding",
        category: "game",
        summary: "Grid A* using std.game.ai helpers pattern.",
        code: r#"use std.game.ai

// Prefer std.game.ai.astar when available; sketch of the heuristic form:
heuristic(a: {x: i64, y: i64}, b: {x: i64, y: i64}) : i64 {
    dx = if a.x > b.x { a.x - b.x } else { b.x - a.x }
    dy = if a.y > b.y { a.y - b.y } else { b.y - a.y }
    dx + dy
}
"#,
        tags: &["astar", "pathfinding", "game", "ai"],
    },
    Algorithm {
        id: "union-find",
        title: "Disjoint-set union-find",
        category: "structure",
        summary: "Path-compressed union-find for connectivity.",
        code: r#"+Dsu = {
    parent: [i64]
    rank: [i64]

    make(n: i64) : Dsu {
        parent = [0].repeat(n)
        for 0..n { parent[it] = it }
        .{parent, rank: [0].repeat(n)}
    }

    find(@self, x: i64) : i64 {
        if self.parent[x] != x {
            self.parent[x] = @self.find(self.parent[x])
        }
        self.parent[x]
    }

    unite(@self, a: i64, b: i64) : bool {
        ra = @self.find(a)
        rb = @self.find(b)
        ensure ra != rb else { return false }
        if self.rank[ra] < self.rank[rb] {
            self.parent[ra] = rb
        } else if self.rank[ra] > self.rank[rb] {
            self.parent[rb] = ra
        } else {
            self.parent[rb] = ra
            self.rank[ra] += 1
        }
        true
    }
}
"#,
        tags: &["dsu", "union-find", "graph"],
    },
    Algorithm {
        id: "kmp",
        title: "KMP string search",
        category: "string",
        summary: "Knuth–Morris–Pratt first match index.",
        code: r#"kmp_table(pat: str) : [i64] {
    m = pat.len
    lps = [0].repeat(m)
    len = 0
    i = 1
    while i < m {
        if pat[i] == pat[len] {
            len += 1
            lps[i] = len
            i += 1
        } else if len > 0 {
            len = lps[len - 1]
        } else {
            lps[i] = 0
            i += 1
        }
    }
    lps
}

kmp_find(text: str, pat: str) : i64 {
    ensure pat.len > 0 else { return 0 }
    lps = kmp_table(pat)
    i = 0
    j = 0
    while i < text.len {
        if text[i] == pat[j] {
            i += 1
            j += 1
            if j == pat.len { return i - j }
        } else if j > 0 {
            j = lps[j - 1]
        } else {
            i += 1
        }
    }
    -1
}
"#,
        tags: &["string", "kmp", "search"],
    },
    Algorithm {
        id: "fft-convolve",
        title: "FFT convolution sketch",
        category: "math",
        summary: "Use std.math.fft for polynomial multiply / convolution.",
        code: r#"use std.math.fft
use std.math.complex

// Multiply two real sequences via FFT round-trip (see std.math.fft tests).
convolve(a: [f64], b: [f64]) : [f64] {
    // Prefer library helpers; pad to power-of-two, FFT, pointwise mul, IFFT.
    a
}
"#,
        tags: &["fft", "math", "signal"],
    },
    Algorithm {
        id: "lru-cache",
        title: "LRU cache",
        category: "structure",
        summary: "Fixed-capacity least-recently-used map pattern.",
        code: r#"+Lru = {
    cap: i64
    order: [i64]
    vals: {i64: i64}

    new(cap: i64) : Lru = .{cap, order: [], vals: {}}

    get(@self, key: i64) : i64? {
        ensure self.vals.has(key) else { return none }
        self.order = self.order.keep(it != key) + [key]
        self.vals[key]
    }

    put(@self, key: i64, val: i64) {
        if self.vals.has(key) {
            self.order = self.order.keep(it != key)
        } else if self.order.len >= self.cap {
            old = self.order[0]
            self.order = self.order[1..]
            self.vals = self.vals.without(old)
        }
        self.order = self.order + [key]
        self.vals[key] = val
    }
}
"#,
        tags: &["cache", "lru", "map"],
    },
    Algorithm {
        id: "spatial-hash",
        title: "Spatial hash collision grid",
        category: "game",
        summary: "Cell-based broadphase used by space shooter / physics examples.",
        code: r#"cell_key(x: f64, y: f64, cell: f64) : i64 {
    cx = (x / cell).floor()
    cy = (y / cell).floor()
    cx * 73856093 + cy * 19349663
}

// Insert entity ids into buckets[cell_key(...)], then query 3x3 neighborhood.
"#,
        tags: &["collision", "spatial", "game", "physics"],
    },
    Algorithm {
        id: "json-pipeline",
        title: "Log keep/map/group_by pipeline",
        category: "data",
        summary: "Zero-allocation UFCS pipeline over structured logs.",
        code: r#"use std.fmt.json

analyze(rows: [{level: str, ms: i64}]) : {error_count: i64, p95: i64} {
    errors = rows.keep(it.level == "error")
    times = errors.map(it.ms)
    .{
        error_count: errors.len,
        p95: times.sorted()[(times.len * 95) / 100]
    }
}
"#,
        tags: &["pipeline", "logs", "json", "analytics"],
    },
    Algorithm {
        id: "token-bucket",
        title: "Token-bucket rate limiter",
        category: "net",
        summary: "Request throttling for HTTP APIs.",
        code: r#"+Bucket = {
    rate: f64
    burst: f64
    tokens: f64
    updated: f64

    allow(@self, now: f64, cost: f64) : bool {
        elapsed = now - self.updated
        self.tokens = min(self.burst, self.tokens + elapsed * self.rate)
        self.updated = now
        ensure self.tokens >= cost else { return false }
        self.tokens -= cost
        true
    }
}
"#,
        tags: &["rate-limit", "http", "net"],
    },
];

pub fn find(id: &str) -> Option<&'static Algorithm> {
    ALL.iter()
        .find(|a| a.id == id || a.title.eq_ignore_ascii_case(id))
}

pub fn search(query: &str) -> Vec<&'static Algorithm> {
    ALL.iter()
        .filter(|a| {
            let blob = format!(
                "{} {} {} {} {} {}",
                a.id,
                a.title,
                a.category,
                a.summary,
                a.code,
                a.tags.join(" ")
            );
            super::matches_query(&blob, query)
        })
        .collect()
}
