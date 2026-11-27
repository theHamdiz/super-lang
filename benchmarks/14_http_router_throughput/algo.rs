// 14_http_router_throughput - 30k simulated HTTP requests: parse bytes, radix route match,
// HMAC-SHA256 bearer-token check, JSON body validation, status-code checksum
const NREQ: usize = 30000;
const NOW: i64 = 1_700_000_000;

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
    0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
    0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
    0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
    0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
    0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
    0xc67178f2,
];

fn sha256(msg: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let total = (msg.len() + 9).div_ceil(64) * 64;
    let mut buf = vec![0u8; total];
    buf[..msg.len()].copy_from_slice(msg);
    buf[msg.len()] = 0x80;
    buf[total - 8..].copy_from_slice(&((msg.len() as u64) * 8).to_be_bytes());
    for blk in buf.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([blk[4 * i], blk[4 * i + 1], blk[4 * i + 2], blk[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let t1 = hh
                .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                .wrapping_add((e & f) ^ (!e & g))
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22)).wrapping_add((a & b) ^ (a & c) ^ (b & c));
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

fn hmac(key: &[u8; 32], msg: &[u8]) -> [u8; 32] {
    let mut inner = Vec::with_capacity(64 + msg.len());
    let mut outer = Vec::with_capacity(96);
    for i in 0..64 {
        let k = if i < 32 { key[i] } else { 0 };
        inner.push(k ^ 0x36);
        outer.push(k ^ 0x5c);
    }
    inner.extend_from_slice(msg);
    outer.extend_from_slice(&sha256(&inner));
    sha256(&outer)
}

const HEX: &[u8; 16] = b"0123456789abcdef";

struct Rng(i64);
impl Rng {
    fn draw(&mut self, m: i64) -> i64 {
        self.0 = (self.0 * 1103515245 + 12345) % 2147483648;
        self.0 / 256 % m
    }
}

fn put_dec(out: &mut Vec<u8>, v: i64) {
    out.extend_from_slice(v.to_string().as_bytes());
}

fn put_id(out: &mut Vec<u8>, r: &mut Rng) {
    let bad = r.draw(50);
    let idv = 1 + r.draw(99999);
    if bad == 0 {
        out.extend_from_slice(b"abc");
    } else {
        put_dec(out, idv);
    }
}

fn rand_chars(out: &mut Vec<u8>, r: &mut Rng, n: i64) {
    for _ in 0..n {
        out.push(b'a' + r.draw(26) as u8);
    }
}

fn build_request(r: &mut Rng, key: &[u8; 32], req: &mut Vec<u8>) {
    req.clear();
    let (mut path, mut body, mut tok) = (Vec::new(), Vec::new(), Vec::new());
    let (mut method, mut has_body) = (0usize, false);
    let a = r.draw(100);
    match a {
        0..=4 => path.extend_from_slice(b"/health"),
        5..=14 => path.extend_from_slice(b"/users"),
        15..=34 => {
            path.extend_from_slice(b"/users/");
            put_id(&mut path, r);
        }
        35..=44 => {
            path.extend_from_slice(b"/users/");
            put_id(&mut path, r);
            path.extend_from_slice(b"/orders");
        }
        45..=54 => {
            (method, has_body) = (1, true);
            path.extend_from_slice(b"/orders");
        }
        55..=64 => {
            path.extend_from_slice(b"/orders/");
            put_id(&mut path, r);
        }
        65..=69 => {
            (method, has_body) = (2, true);
            path.extend_from_slice(b"/orders/");
            put_id(&mut path, r);
        }
        70..=74 => {
            method = 3;
            path.extend_from_slice(b"/orders/");
            put_id(&mut path, r);
        }
        75..=84 => {
            path.extend_from_slice(b"/static/a");
            put_dec(&mut path, r.draw(10));
            path.extend_from_slice(b"/f");
            put_dec(&mut path, r.draw(100));
            path.extend_from_slice(b".js");
        }
        85..=89 => {
            (method, has_body) = (1, true);
            path.extend_from_slice(b"/auth/login");
        }
        90..=94 => {
            path.extend_from_slice(b"/v1/items/");
            put_id(&mut path, r);
            path.extend_from_slice(b"/tags/t");
            put_dec(&mut path, r.draw(50));
        }
        95..=97 => {
            path.extend_from_slice(b"/nope/");
            put_id(&mut path, r);
        }
        _ => {
            method = 3;
            path.extend_from_slice(b"/users");
        }
    }

    let t = r.draw(20);
    let uid = 1 + r.draw(9999);
    let ex = r.draw(1000);
    let has_tok = t >= 2;
    if has_tok {
        let exp = if t == 4 { NOW - 1 - ex } else { NOW + 1000 + ex };
        let mut sb = Vec::new();
        put_dec(&mut sb, uid);
        sb.push(b'.');
        put_dec(&mut sb, exp);
        let mut mac = hmac(key, &sb);
        if t < 4 {
            mac[0] ^= 1;
        }
        tok.extend_from_slice(&sb);
        tok.push(b'.');
        for &m in &mac[..4] {
            tok.push(HEX[(m >> 4) as usize]);
            tok.push(HEX[(m & 15) as usize]);
        }
    }

    if has_body {
        let v = r.draw(10);
        let ws = r.draw(2);
        let sp: &[u8] = if ws == 1 { b" " } else { b"" };
        if (45..55).contains(&a) {
            let mut user = 1 + r.draw(9999);
            let mut qty = 1 + r.draw(100);
            if v == 0 {
                qty = 0;
            }
            if v == 3 {
                user = 10000;
            }
            body.extend_from_slice(b"{\"user\":");
            body.extend_from_slice(sp);
            put_dec(&mut body, user);
            body.push(b',');
            body.extend_from_slice(sp);
            body.extend_from_slice(b"\"qty\":");
            body.extend_from_slice(sp);
            put_dec(&mut body, qty);
            let mut sku = Vec::new();
            rand_chars(&mut sku, r, 4);
            if v != 1 {
                body.push(b',');
                body.extend_from_slice(sp);
                body.extend_from_slice(b"\"sku\":");
                body.extend_from_slice(sp);
                body.push(b'"');
                body.extend_from_slice(&sku);
                body.push(b'"');
            }
            body.push(b'}');
            if v == 2 {
                body.pop();
            }
        } else if (65..70).contains(&a) {
            let mut qty = 1 + r.draw(100);
            let nl = 3 + r.draw(5);
            if v == 0 {
                qty = 101;
            }
            body.push(b'{');
            if v != 1 {
                body.extend_from_slice(b"\"qty\":");
                body.extend_from_slice(sp);
                put_dec(&mut body, qty);
                body.push(b',');
                body.extend_from_slice(sp);
            }
            body.extend_from_slice(b"\"note\":");
            body.extend_from_slice(sp);
            body.push(b'"');
            rand_chars(&mut body, r, nl);
            body.extend_from_slice(b"\"}");
            if v == 2 {
                body.pop();
            }
        } else {
            let ul = 4 + r.draw(6);
            let mut un = Vec::new();
            rand_chars(&mut un, r, ul);
            let pwl = 6 + r.draw(7);
            let mut pw = Vec::new();
            rand_chars(&mut pw, r, pwl);
            if v == 0 {
                pw.clear();
            }
            body.push(b'{');
            if v != 1 {
                body.extend_from_slice(b"\"user\":");
                body.extend_from_slice(sp);
                body.push(b'"');
                body.extend_from_slice(&un);
                body.extend_from_slice(b"\",");
                body.extend_from_slice(sp);
            }
            body.extend_from_slice(b"\"pass\":");
            body.extend_from_slice(sp);
            body.push(b'"');
            body.extend_from_slice(&pw);
            body.extend_from_slice(b"\"}");
            if v == 2 {
                body.push(b'x');
            }
        }
    }

    req.extend_from_slice([&b"GET "[..], b"POST ", b"PUT ", b"DELETE "][method]);
    req.extend_from_slice(&path);
    req.extend_from_slice(b" HTTP/1.1\r\nHost: api.local\r\n");
    if has_tok {
        req.extend_from_slice(b"Authorization: Bearer ");
        req.extend_from_slice(&tok);
        req.extend_from_slice(b"\r\n");
    }
    if has_body {
        req.extend_from_slice(b"Content-Length: ");
        put_dec(req, body.len() as i64);
        req.extend_from_slice(b"\r\n");
    }
    req.extend_from_slice(b"\r\n");
    req.extend_from_slice(&body);
}

// ---- radix router ----
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Static,
    Param,
    Wild,
}

struct Node {
    kind: Kind,
    lit: Vec<u8>,
    first: i32,
    next: i32,
    h: [i32; 4],
}

struct Router {
    nodes: Vec<Node>,
}

impl Router {
    fn new() -> Self {
        Router { nodes: vec![Node { kind: Kind::Static, lit: Vec::new(), first: -1, next: -1, h: [-1; 4] }] }
    }

    fn add(&mut self, tpl: &str, method: usize, rid: i32) {
        let mut cur = 0usize;
        for seg in tpl[1..].split('/') {
            let kind = match seg.as_bytes()[0] {
                b':' => Kind::Param,
                b'*' => Kind::Wild,
                _ => Kind::Static,
            };
            let lit: Vec<u8> = if kind == Kind::Static { seg.as_bytes().to_vec() } else { Vec::new() };
            let mut found = -1i32;
            let mut c = self.nodes[cur].first;
            while c >= 0 && found < 0 {
                let nd = &self.nodes[c as usize];
                if nd.kind == kind && (kind != Kind::Static || nd.lit == lit) {
                    found = c;
                }
                c = nd.next;
            }
            if found < 0 {
                let next = self.nodes[cur].first;
                self.nodes.push(Node { kind, lit, first: -1, next, h: [-1; 4] });
                found = self.nodes.len() as i32 - 1;
                self.nodes[cur].first = found;
            }
            cur = found as usize;
        }
        self.nodes[cur].h[method] = rid;
    }
}

const AUTH: [bool; 11] = [false, true, true, true, true, true, true, true, false, false, true];

fn parse_digits(b: &[u8], p: usize, e: usize) -> i64 {
    if e <= p || e - p > 10 {
        return -1;
    }
    let mut v = 0i64;
    for &c in &b[p..e] {
        if !c.is_ascii_digit() {
            return -1;
        }
        v = v * 10 + (c - b'0') as i64;
    }
    v
}

fn verify_token(b: &[u8], off: usize, len: usize, key: &[u8; 32]) -> i64 {
    let e = off + len;
    let (mut d1, mut d2) = (usize::MAX, usize::MAX);
    for i in off..e {
        if b[i] == b'.' {
            if d1 == usize::MAX {
                d1 = i;
            } else if d2 == usize::MAX {
                d2 = i;
            } else {
                return -1;
            }
        }
    }
    if d1 == usize::MAX || d2 == usize::MAX || e - d2 - 1 != 8 {
        return -1;
    }
    let u = parse_digits(b, off, d1);
    let ex = parse_digits(b, d1 + 1, d2);
    if u < 0 || ex < 0 {
        return -1;
    }
    let mac = hmac(key, &b[off..d2]);
    for i in 0..4 {
        if b[d2 + 1 + 2 * i] != HEX[(mac[i] >> 4) as usize] || b[d2 + 2 + 2 * i] != HEX[(mac[i] & 15) as usize] {
            return -1;
        }
    }
    if ex <= NOW {
        return -1;
    }
    u
}

fn skip_ws(b: &[u8], mut p: usize, e: usize) -> usize {
    while p < e && matches!(b[p], b' ' | b'\t' | b'\n' | b'\r') {
        p += 1;
    }
    p
}

fn key_id(k: &[u8]) -> usize {
    match k {
        b"user" => 1,
        b"qty" => 2,
        b"sku" => 3,
        b"note" => 4,
        b"pass" => 5,
        _ => 0,
    }
}

// f[id] = (type, value); type 0 = absent, 1 = int, 2 = string (value = length)
type Fields = [(u8, i64); 6];

fn parse_json(b: &[u8], off: usize, len: usize) -> Option<Fields> {
    let e = off + len;
    let mut f: Fields = [(0, 0); 6];
    let mut p = skip_ws(b, off, e);
    if p >= e || b[p] != b'{' {
        return None;
    }
    p = skip_ws(b, p + 1, e);
    if p < e && b[p] == b'}' {
        p = skip_ws(b, p + 1, e);
        return (p == e).then_some(f);
    }
    loop {
        if p >= e || b[p] != b'"' {
            return None;
        }
        p += 1;
        let ks = p;
        while p < e && b[p] != b'"' {
            if b[p] == b'\\' {
                return None;
            }
            p += 1;
        }
        if p >= e {
            return None;
        }
        let kid = key_id(&b[ks..p]);
        p = skip_ws(b, p + 1, e);
        if p >= e || b[p] != b':' {
            return None;
        }
        p = skip_ws(b, p + 1, e);
        if p >= e {
            return None;
        }
        let (ty, val);
        if b[p] == b'"' {
            p += 1;
            let vs = p;
            while p < e && b[p] != b'"' {
                if b[p] == b'\\' {
                    return None;
                }
                p += 1;
            }
            if p >= e {
                return None;
            }
            (ty, val) = (2u8, (p - vs) as i64);
            p += 1;
        } else {
            let neg = b[p] == b'-';
            if neg {
                p += 1;
            }
            let ds = p;
            while p < e && b[p].is_ascii_digit() {
                p += 1;
            }
            let v = parse_digits(b, ds, p);
            if v < 0 {
                return None;
            }
            (ty, val) = (1u8, if neg { -v } else { v });
        }
        if kid > 0 {
            f[kid] = (ty, val);
        }
        p = skip_ws(b, p, e);
        if p >= e {
            return None;
        }
        match b[p] {
            b',' => p = skip_ws(b, p + 1, e),
            b'}' => {
                p += 1;
                break;
            }
            _ => return None,
        }
    }
    p = skip_ws(b, p, e);
    (p == e).then_some(f)
}

fn has_int(f: &Fields, i: usize, lo: i64, hi: i64) -> bool {
    f[i].0 == 1 && (lo..=hi).contains(&f[i].1)
}
fn has_str(f: &Fields, i: usize, lo: i64, hi: i64) -> bool {
    f[i].0 == 2 && (lo..=hi).contains(&f[i].1)
}

fn body_valid(rid: i32, b: &[u8], off: usize, len: usize) -> bool {
    let Some(f) = parse_json(b, off, len) else { return false };
    match rid {
        4 => has_int(&f, 1, 1, 9999) && has_int(&f, 2, 1, 100) && has_str(&f, 3, 4, 4),
        6 => has_int(&f, 2, 1, 100) && (f[4].0 == 0 || has_str(&f, 4, 0, 1000)),
        _ => has_str(&f, 1, 1, 64) && has_str(&f, 5, 1, 64),
    }
}

// returns (status, path id)
fn handle(b: &[u8], router: &Router, key: &[u8; 32]) -> (i64, i64) {
    let n = b.len();
    let (method, mut p) = if b.starts_with(b"GET ") {
        (0, 4)
    } else if b.starts_with(b"POST ") {
        (1, 5)
    } else if b.starts_with(b"PUT ") {
        (2, 4)
    } else if b.starts_with(b"DELETE ") {
        (3, 7)
    } else {
        return (400, 0);
    };
    let po = p;
    while p < n && b[p] != b' ' {
        p += 1;
    }
    let pe = p;
    while p < n && b[p] != b'\r' {
        p += 1;
    }
    p += 2;

    let (mut tok_off, mut tok_len, mut clen) = (usize::MAX, 0usize, -1i64);
    while p < n {
        if b[p] == b'\r' {
            p += 2;
            break;
        }
        let mut le = p;
        while le < n && b[le] != b'\r' {
            le += 1;
        }
        if b[p..].starts_with(b"Authorization: Bearer ") {
            tok_off = p + 22;
            tok_len = le - (p + 22);
        } else if b[p..].starts_with(b"Content-Length: ") {
            clen = parse_digits(b, p + 16, le);
        }
        p = le + 2;
    }
    let (body_off, body_len) = (p, n.saturating_sub(p));
    if clen >= 0 && clen != body_len as i64 {
        return (400, 0);
    }

    let plen = pe - po;
    let (mut cur, mut found, mut nparam, mut bad_id, mut pid) = (0usize, true, 0, false, -1i64);
    if plen < 1 || b[po] != b'/' {
        found = false;
    }
    let mut i = 1;
    let nodes = &router.nodes;
    while found && i <= plen {
        let mut e = i;
        while e < plen && b[po + e] != b'/' {
            e += 1;
        }
        if e == i {
            found = false;
            break;
        }
        let seg = &b[po + i..po + e];
        let (mut st, mut pm, mut wc) = (-1i32, -1i32, -1i32);
        let mut c = nodes[cur].first;
        while c >= 0 {
            let nd = &nodes[c as usize];
            match nd.kind {
                Kind::Static => {
                    if nd.lit == seg {
                        st = c;
                        break;
                    }
                }
                Kind::Param => pm = c,
                Kind::Wild => wc = c,
            }
            c = nd.next;
        }
        if st >= 0 {
            cur = st as usize;
        } else if pm >= 0 {
            cur = pm as usize;
            if nparam == 0 {
                pid = parse_digits(b, po + i, po + e);
                if pid < 0 {
                    bad_id = true;
                }
            }
            nparam += 1;
        } else if wc >= 0 {
            cur = wc as usize;
            break;
        } else {
            found = false;
            break;
        }
        i = e + 1;
    }
    if !found {
        return (404, 0);
    }
    let hs = nodes[cur].h;
    let rid = hs[method];
    if rid < 0 {
        return (if hs.iter().any(|&x| x >= 0) { 405 } else { 404 }, 0);
    }
    let pout = if bad_id || pid < 0 { 0 } else { pid };

    if AUTH[rid as usize] {
        if tok_off == usize::MAX {
            return (401, pout);
        }
        let uid = verify_token(b, tok_off, tok_len, key);
        if uid < 0 {
            return (401, pout);
        }
        if (rid == 6 || rid == 7) && uid % 13 == 0 {
            return (403, pout);
        }
    }
    if bad_id {
        return (400, pout);
    }
    if matches!(rid, 4 | 6 | 9) && !body_valid(rid, b, body_off, body_len) {
        return (400, pout);
    }
    let st = match rid {
        2 if pid > 90000 => 404,
        4 => 201,
        5 if pid % 7 == 0 => 404,
        7 => 204,
        _ => 200,
    };
    (st, pout)
}

fn main() {
    let mut key = [0u8; 32];
    for (i, k) in key.iter_mut().enumerate() {
        *k = ((i * 37 + 11) % 256) as u8;
    }
    let mut router = Router::new();
    for (tpl, m, rid) in [
        ("/health", 0, 0),
        ("/users", 0, 1),
        ("/users/:id", 0, 2),
        ("/users/:id/orders", 0, 3),
        ("/orders", 1, 4),
        ("/orders/:id", 0, 5),
        ("/orders/:id", 2, 6),
        ("/orders/:id", 3, 7),
        ("/static/*path", 0, 8),
        ("/auth/login", 1, 9),
        ("/v1/items/:id/tags/:tag", 0, 10),
    ] {
        router.add(tpl, m, rid);
    }

    let mut rng = Rng(140014);
    let (mut cs, mut sum_pid, mut ok2xx) = (0i64, 0i64, 0i64);
    let mut by_status = [0i64; 600];
    let mut req = Vec::with_capacity(512);
    for i in 0..NREQ {
        build_request(&mut rng, &key, &mut req);
        let (st, pid) = handle(&req, &router, &key);
        by_status[st as usize] += 1;
        cs += st * (i as i64 % 97 + 1);
        sum_pid += pid;
        if st < 300 {
            ok2xx += 1;
        }
    }
    let mut mix = 0i64;
    for (s, &c) in by_status.iter().enumerate() {
        mix += c * (s as i64 % 53 + 1) * (s as i64 / 100 + 1);
    }
    cs += sum_pid + 5 * ok2xx + 7 * mix;
    println!("CHECKSUM={cs}");
}
