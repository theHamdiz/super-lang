// 18_stdlib_core_algorithms — one checksum from four std-style kernels:
// (A) KMP-DFA "regex" field extraction over 3000 generated log lines,
// (B) strict UTF-8 validation/counting over 60 documents (some corrupted),
// (C) SQL-ish filter / group-by / order-by-limit / join over 6000 rows x 24 queries,
// (D) HM-style type unification (union-find, iterative) over 8504 type nodes.
// Build: rustc -C opt-level=3 -C target-cpu=native algo.rs

const MOD: i64 = 1_000_000_007;
const NL: usize = 3000;
const NDOC: usize = 60;
const PERDOC: usize = 200;
const NR: usize = 6000;
const NQ: usize = 24;
const NVARS: usize = 2500;
const NCOMP: usize = 6000;
const NCON: usize = 5000;
const NTOT: usize = NVARS + 4 + NCOMP;

struct Ctx {
    rng: i64,
    cs: i64,
}

impl Ctx {
    fn next(&mut self) -> i64 {
        self.rng = self.rng * 48271 % 2147483647;
        self.rng
    }
    fn fold(&mut self, v: i64) {
        self.cs = (self.cs * 131 + v) % MOD;
    }
}

// ---------- (A) log extraction ----------
fn build_dfa(needle: &[u8]) -> Vec<i32> {
    let n = needle.len();
    let mut dfa = vec![0i32; n * 256];
    dfa[needle[0] as usize] = 1;
    let mut x = 0usize;
    for j in 1..n {
        dfa.copy_within(x * 256..x * 256 + 256, j * 256);
        dfa[j * 256 + needle[j] as usize] = j as i32 + 1;
        x = dfa[x * 256 + needle[j] as usize] as usize;
    }
    dfa
}

fn extract(line: &[u8], dfa: &[i32], n: usize) -> i64 {
    let (mut st, mut i) = (0usize, 0usize);
    while i < line.len() {
        st = dfa[st * 256 + line[i] as usize] as usize;
        i += 1;
        if st == n {
            let (mut v, mut nd) = (0i64, 0);
            while i < line.len() && line[i].is_ascii_digit() {
                v = v * 10 + (line[i] - b'0') as i64;
                nd += 1;
                i += 1;
            }
            if nd > 0 {
                return v;
            }
            st = 0;
        }
    }
    -1
}

fn part_a(c: &mut Ctx) {
    const LEVELS: [&str; 4] = ["INFO", "WARN", "ERROR", "DEBUG"];
    const CODES: [i64; 9] = [200, 201, 204, 301, 400, 401, 404, 500, 503];
    let (d_status, d_took, d_user) = (build_dfa(b"status="), build_dfa(b"took="), build_dfa(b"user="));
    let mut cls = [0i64; 8];
    let (mut took_sum, mut took_max, mut took_n, mut user_sum, mut bad, mut total) = (0i64, 0i64, 0i64, 0i64, 0i64, 0i64);
    for i in 0..NL {
        let (mo, dd, hh) = (1 + c.next() % 12, 1 + c.next() % 28, c.next() % 24);
        let (mi, ss, lv) = (c.next() % 60, c.next() % 60, (c.next() % 4) as usize);
        let (svc, uid, sidx, ms) = (c.next() % 100, c.next() % 100000, (c.next() % 9) as usize, c.next() % 3000);
        let mut s = format!("2026-{mo:02}-{dd:02} {hh:02}:{mi:02}:{ss:02} {} svc-{svc:02} user={uid}", LEVELS[lv]);
        if i % 13 == 5 {
            s.push_str(" status=abc");
        } else {
            s.push_str(&format!(" status={}", CODES[sidx]));
        }
        if i % 17 != 3 {
            s.push_str(&format!(" took={ms}ms"));
        }
        let line = s.as_bytes();
        let (st, tk, us) = (extract(line, &d_status, 7), extract(line, &d_took, 5), extract(line, &d_user, 5));
        if st < 0 {
            bad += 1;
        } else {
            cls[(st / 100) as usize] += 1;
        }
        if tk >= 0 {
            took_sum += tk;
            took_n += 1;
            took_max = took_max.max(tk);
        }
        user_sum += us;
        total += line.len() as i64 + 1;
    }
    for v in cls {
        c.fold(v);
    }
    for v in [took_sum, took_max, took_n, user_sum, bad, total] {
        c.fold(v);
    }
}

// ---------- (B) UTF-8 ----------
fn cont(b: u8) -> bool {
    b & 0xC0 == 0x80
}

fn utf8_valid(d: &[u8]) -> bool {
    let (mut i, n) = (0usize, d.len());
    while i < n {
        let b0 = d[i] as i32;
        if b0 < 128 {
            i += 1;
        } else if b0 < 194 {
            return false;
        } else if b0 < 224 {
            if i + 1 >= n || !cont(d[i + 1]) {
                return false;
            }
            i += 2;
        } else if b0 < 240 {
            if i + 2 >= n || !cont(d[i + 1]) || !cont(d[i + 2]) {
                return false;
            }
            let cp = (b0 - 224) * 4096 + (d[i + 1] as i32 - 128) * 64 + (d[i + 2] as i32 - 128);
            if cp < 2048 || (55296..=57343).contains(&cp) {
                return false;
            }
            i += 3;
        } else if b0 < 245 {
            if i + 3 >= n || !cont(d[i + 1]) || !cont(d[i + 2]) || !cont(d[i + 3]) {
                return false;
            }
            let cp = (b0 - 240) * 262144 + (d[i + 1] as i32 - 128) * 4096 + (d[i + 2] as i32 - 128) * 64 + (d[i + 3] as i32 - 128);
            if cp < 65536 || cp > 1114111 {
                return false;
            }
            i += 4;
        } else {
            return false;
        }
    }
    true
}

fn utf8_count(d: &[u8]) -> i64 {
    d.iter().filter(|&&b| !cont(b)).count() as i64
}

fn encode(cp: i64, out: &mut Vec<u8>) {
    if cp < 128 {
        out.push(cp as u8);
    } else if cp < 2048 {
        out.extend([(192 + cp / 64) as u8, (128 + cp % 64) as u8]);
    } else if cp < 65536 {
        out.extend([(224 + cp / 4096) as u8, (128 + cp / 64 % 64) as u8, (128 + cp % 64) as u8]);
    } else {
        out.extend([(240 + cp / 262144) as u8, (128 + cp / 4096 % 64) as u8, (128 + cp / 64 % 64) as u8, (128 + cp % 64) as u8]);
    }
}

fn part_b(c: &mut Ctx) {
    const BADB: [u8; 6] = [0x80, 0xC0, 0xC1, 0xF5, 0xFF, 0xE0];
    let (mut valid_docs, mut total_cp) = (0i64, 0i64);
    for d in 0..NDOC {
        let mut doc: Vec<u8> = Vec::with_capacity(PERDOC * 4);
        for _ in 0..PERDOC {
            let (cat, r) = (c.next() % 4, c.next());
            let cp = match cat {
                0 => r % 128,
                1 => 128 + r % 1920,
                2 => 2048 + r % 53248,
                _ => 65536 + r % 1048576,
            };
            encode(cp, &mut doc);
        }
        if d % 5 == 2 {
            let (p, b) = ((c.next() % doc.len() as i64) as usize, (c.next() % 6) as usize);
            doc[p] = BADB[b];
        }
        let ok = utf8_valid(&doc);
        let cnt = if ok { utf8_count(&doc) } else { 0 };
        c.fold(ok as i64);
        c.fold(cnt);
        c.fold(doc.len() as i64);
        valid_docs += ok as i64;
        total_cp += cnt;
    }
    c.fold(valid_docs);
    c.fold(total_cp);
}

// ---------- (C) SQL-ish ----------
fn part_c(c: &mut Ctx) {
    let (mut dept, mut age, mut sal, mut act) = (vec![0i64; NR], vec![0i64; NR], vec![0i64; NR], vec![0i64; NR]);
    for i in 0..NR {
        dept[i] = c.next() % 16;
        age[i] = 18 + c.next() % 50;
        sal[i] = 30000 + c.next() % 90000;
        act[i] = (c.next() % 10 != 0) as i64;
    }
    for _ in 0..NQ {
        let (age_lo, sal_max, need) = (18 + c.next() % 30, 40000 + c.next() % 80000, c.next() % 2);
        let (mut cnt, mut sum, mut mx, mut taken) = ([0i64; 16], [0i64; 16], [0i64; 16], [false; 16]);
        for i in 0..NR {
            if age[i] >= age_lo && sal[i] < sal_max && act[i] >= need {
                let g = dept[i] as usize;
                cnt[g] += 1;
                sum[g] += sal[i];
                mx[g] = mx[g].max(age[i]);
            }
        }
        for _ in 0..3 {
            let mut best: Option<usize> = None;
            for g in 0..16 {
                if cnt[g] >= 5 && !taken[g] && best.is_none_or(|b| sum[g] > sum[b]) {
                    best = Some(g);
                }
            }
            if let Some(b) = best {
                taken[b] = true;
                for v in [b as i64, cnt[b], sum[b], mx[b], sum[b] / ((b as i64 + 1) * 1000)] {
                    c.fold(v);
                }
            } else {
                c.fold(999);
            }
        }
    }
}

// ---------- (D) HM unification ----------
struct Types {
    kind: Vec<u8>,
    na: Vec<i32>,
    nb: Vec<i32>,
    parent: Vec<i32>,
    stack: Vec<(i32, i32)>,
    unions: i64,
}

impl Types {
    fn find(&mut self, mut x: i32) -> i32 {
        while self.parent[x as usize] != x {
            let p = self.parent[x as usize];
            self.parent[x as usize] = self.parent[p as usize];
            x = self.parent[x as usize];
        }
        x
    }

    fn unify(&mut self, x: i32, y: i32) -> bool {
        self.stack.clear();
        self.stack.push((x, y));
        while let Some((a, b)) = self.stack.pop() {
            let (rx, ry) = (self.find(a), self.find(b));
            if rx == ry {
                continue;
            }
            let (kx, ky) = (self.kind[rx as usize], self.kind[ry as usize]);
            if kx == 0 {
                self.parent[rx as usize] = ry;
                self.unions += 1;
            } else if ky == 0 {
                self.parent[ry as usize] = rx;
                self.unions += 1;
            } else if kx == 1 || ky == 1 {
                if kx == 1 && ky == 1 && self.na[rx as usize] == self.na[ry as usize] {
                    self.parent[ry as usize] = rx;
                    self.unions += 1;
                } else {
                    return false;
                }
            } else {
                self.parent[ry as usize] = rx;
                self.unions += 1;
                self.stack.push((self.na[rx as usize], self.na[ry as usize]));
                self.stack.push((self.nb[rx as usize], self.nb[ry as usize]));
            }
        }
        true
    }
}

fn part_d(c: &mut Ctx) {
    let mut t = Types {
        kind: vec![0; NTOT],
        na: vec![-1; NTOT],
        nb: vec![-1; NTOT],
        parent: (0..NTOT as i32).collect(),
        stack: Vec::with_capacity(2 * NTOT + 16),
        unions: 0,
    };
    for k in 0..4 {
        t.kind[NVARS + k] = 1;
        t.na[NVARS + k] = k as i32;
    }
    for i in NVARS + 4..NTOT {
        t.kind[i] = 2;
        t.na[i] = (c.next() % i as i64) as i32;
        t.nb[i] = (c.next() % i as i64) as i32;
    }
    let mut fails = 0i64;
    for _ in 0..NCON {
        let tsel = c.next() % 2;
        let x = if tsel == 0 { c.next() % NVARS as i64 } else { c.next() % NTOT as i64 } as i32;
        let y = (c.next() % NTOT as i64) as i32;
        if !t.unify(x, y) {
            fails += 1;
        }
    }
    c.fold(fails);
    c.fold(t.unions);
    let mut classes = 0i64;
    for i in 0..NTOT {
        let r = t.find(i as i32) as usize;
        if r == i {
            classes += 1;
        }
        c.fold(r as i64 * 3 + t.kind[r] as i64);
    }
    c.fold(classes);
}

fn main() {
    let mut c = Ctx { rng: 18018, cs: 0 };
    part_a(&mut c);
    part_b(&mut c);
    part_c(&mut c);
    part_d(&mut c);
    println!("CHECKSUM={}", c.cs);
}
