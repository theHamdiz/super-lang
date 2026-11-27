// 20_end_to_end_lakehouse_service — in-process pipeline:
// ingest JSONL customers + CSV order batches -> filter/join/group -> fixed-point GD linear fit on the
// per-product aggregates -> serve HTTP-like request bytes (bearer-token check via SHA-256, Merkle root of
// the group table in every response) -> checksum of response bytes.
// Build: rustc -C opt-level=3 -C target-cpu=native algo.rs

use std::fmt::Write as _;

const MOD: i64 = 1_000_000_007;
const NCUST: usize = 500;
const NPROD: usize = 50;
const NREG: usize = 8;
const NTIER: usize = 4;
const NGRP: usize = NREG * NTIER;
const BATCHES: usize = 6;
const ROWS_PB: usize = 4000;
const NREQ: usize = 3000;
const NCLIENT: usize = 8;
const ITERS: usize = 400;

struct Rng(i64);
impl Rng {
    fn next(&mut self) -> i64 {
        self.0 = self.0 * 48271 % 2147483647;
        self.0
    }
}

fn sdiv(a: i64, d: i64) -> i64 {
    if a >= 0 { a / d } else { -((-a) / d) }
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

type Digest = [u8; 32];

fn sha256(msg: &[u8]) -> Digest {
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut total = msg.len() + 1;
    while total % 64 != 56 {
        total += 1;
    }
    total += 8;
    let mut buf = vec![0u8; total];
    buf[..msg.len()].copy_from_slice(msg);
    buf[msg.len()] = 0x80;
    buf[total - 8..].copy_from_slice(&((msg.len() as u64) * 8).to_be_bytes());
    for chunk in buf.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
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
            hh = g; g = f; f = e; e = d.wrapping_add(t1); d = c; c = b; b = a; a = t1.wrapping_add(t2);
        }
        for (s, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *s = s.wrapping_add(v);
        }
    }
    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

fn hex(d: &[u8]) -> String {
    let mut s = String::with_capacity(d.len() * 2);
    for b in d {
        write!(s, "{b:02x}").unwrap();
    }
    s
}

const fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 == 1 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}
static CRC: [u32; 256] = crc_table();
fn crc32(d: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in d {
        c = CRC[((c ^ b as u32) & 255) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

/// Parse the decimal run at `*p` (advancing it); -1 when there are no digits.
fn scan_int(b: &[u8], p: &mut usize) -> i64 {
    let mut v = 0i64;
    let mut have = false;
    while *p < b.len() && b[*p].is_ascii_digit() {
        v = v * 10 + (b[*p] - b'0') as i64;
        *p += 1;
        have = true;
    }
    if have { v } else { -1 }
}

fn index_of(b: &[u8], pat: &[u8], from: usize) -> Option<usize> {
    (from..=b.len().saturating_sub(pat.len())).find(|&i| b[i..].starts_with(pat))
}

fn main() {
    let mut rng = Rng(20020);
    let mut cs = 0i64;
    let mut fold = |v: i64| cs = (cs * 131 + v) % MOD;

    // ---- ingest customers (JSONL) ----
    let mut cj = String::new();
    for i in 0..NCUST {
        let id = i as i64 * 7919 % NCUST as i64;
        let r = rng.next() % NREG as i64;
        let t = rng.next() % NTIER as i64;
        writeln!(cj, "{{\"id\":{id},\"region\":{r},\"tier\":{t}}}").unwrap();
    }
    let mut cb = cj.into_bytes();
    let clen = cb.len();
    cb.push(0);
    let mut region_of = [-1i64; NCUST];
    let mut tier_of = [-1i64; NCUST];
    let mut ncust = 0i64;
    let mut p = 0usize;
    while p < clen {
        while p < clen && !cb[p].is_ascii_digit() { p += 1; }
        let id = scan_int(&cb, &mut p);
        while !cb[p].is_ascii_digit() { p += 1; }
        let r = scan_int(&cb, &mut p);
        while !cb[p].is_ascii_digit() { p += 1; }
        let t = scan_int(&cb, &mut p);
        region_of[id as usize] = r;
        tier_of[id as usize] = t;
        ncust += 1;
        while p < clen && cb[p] != b'\n' { p += 1; }
        p += 1;
    }

    // ---- ingest + filter/join/group order batches (CSV) ----
    let (mut grp_cnt, mut grp_qty, mut grp_rev) = ([0i64; NGRP], [0i64; NGRP], [0i64; NGRP]);
    let (mut prod_cnt, mut prod_qty, mut prod_rev, mut prod_price) = ([0i64; NPROD], [0i64; NPROD], [0i64; NPROD], [0i64; NPROD]);
    let mut reg_prod_rev = [0i64; NREG * NPROD];
    let (mut oid, mut rows, mut null_cust, mut bad_qty, mut valid) = (0i64, 0i64, 0i64, 0i64, 0i64);
    for _ in 0..BATCHES {
        let mut ob = String::from("order_id,cust_id,product,qty,price_cents\n");
        for _ in 0..ROWS_PB {
            let nf = rng.next() % 100;
            let c = rng.next() % NCUST as i64;
            let pr = rng.next() % NPROD as i64;
            let q = rng.next() % 9;
            let price = 100 + pr * 37 + rng.next() % 200;
            if nf != 0 {
                writeln!(ob, "{oid},{c},{pr},{q},{price}").unwrap();
            } else {
                writeln!(ob, "{oid},,{pr},{q},{price}").unwrap();
            }
            oid += 1;
        }
        let mut ob = ob.into_bytes();
        let olen = ob.len();
        ob.push(0);
        let mut pos = ob.iter().position(|&b| b == b'\n').unwrap() + 1;
        while pos < olen {
            let mut f = [0i64; 5];
            for v in f.iter_mut() {
                *v = scan_int(&ob, &mut pos);
                pos += 1;
            }
            rows += 1;
            if f[1] < 0 { null_cust += 1; continue; }
            if f[3] <= 0 { bad_qty += 1; continue; }
            valid += 1;
            let rev = f[3] * f[4];
            let reg = region_of[f[1] as usize];
            let g = (reg * NTIER as i64 + tier_of[f[1] as usize]) as usize;
            grp_cnt[g] += 1; grp_qty[g] += f[3]; grp_rev[g] += rev;
            let pd = f[2] as usize;
            prod_cnt[pd] += 1; prod_qty[pd] += f[3]; prod_rev[pd] += rev; prod_price[pd] += f[4];
            reg_prod_rev[reg as usize * NPROD + pd] += rev;
        }
    }
    let _ = (&prod_qty, &grp_qty);

    // ---- fit y = w*x + b (w in Q16) by gradient descent on per-product aggregates ----
    let (mut xs, mut ys) = (Vec::new(), Vec::new());
    for p in 0..NPROD {
        if prod_cnt[p] > 0 {
            xs.push(prod_price[p] / prod_cnt[p] / 16);
            ys.push(prod_rev[p] / prod_cnt[p] / 16);
        }
    }
    let n = xs.len() as i64;
    let (mut w, mut bias) = (0i64, 0i64);
    for _ in 0..ITERS {
        let (mut gw, mut gb) = (0i64, 0i64);
        for (&x, &y) in xs.iter().zip(&ys) {
            let err = sdiv(w * x, 65536) + bias - y;
            gw += err * x;
            gb += err;
        }
        w -= sdiv(gw * 65536, n * 8000);
        bias -= sdiv(gb, n * 4);
    }
    let mut loss = 0i64;
    for (&x, &y) in xs.iter().zip(&ys) {
        let err = sdiv(w * x, 65536) + bias - y;
        loss += err * err;
    }
    let mut pred_cents = [0i64; NPROD];
    for p in 0..NPROD {
        let x = if prod_cnt[p] > 0 { prod_price[p] / prod_cnt[p] / 16 } else { 0 };
        let y = sdiv(w * x, 65536) + bias;
        pred_cents[p] = y.max(0) * 16;
    }

    // ---- merkle root over the region x tier group table ----
    let mut level: Vec<Digest> = (0..NGRP)
        .map(|g| {
            let mut leaf = [0u8; 32];
            for (k, v) in [g as i64, grp_cnt[g], grp_qty[g], grp_rev[g]].iter().enumerate() {
                leaf[k * 8..k * 8 + 8].copy_from_slice(&(*v as u64).to_be_bytes());
            }
            sha256(&leaf)
        })
        .collect();
    while level.len() > 1 {
        level = level
            .chunks(2)
            .map(|c| {
                let mut cat = [0u8; 64];
                cat[..32].copy_from_slice(&c[0]);
                cat[32..].copy_from_slice(&c[1]);
                sha256(&cat)
            })
            .collect();
    }
    let root = level[0];
    let root_hex = hex(&root);

    // ---- client tokens ----
    let tokens: Vec<Vec<u8>> = (0..NCLIENT).map(|k| hex(&sha256(format!("key-{k}").as_bytes())[..8]).into_bytes()).collect();

    // ---- serve ----
    let (mut ok200, mut un401, mut nf404, mut total_bytes, mut resp_cs) = (0i64, 0i64, 0i64, 0i64, 0i64);
    for _ in 0..NREQ {
        let k = (rng.next() % NCLIENT as i64) as usize;
        let p = rng.next() % 60;
        let rg = rng.next() % NREG as i64;
        let bad = rng.next() % 100;
        let at = (rng.next() % 16) as usize;
        let mut tok = tokens[k].clone();
        if bad < 8 { tok[at] = b'z'; }
        let mut req = format!("GET /v1/product/{p}?region={rg} HTTP/1.1\r\nHost: lake\r\nX-Client: {k}\r\nAuthorization: Bearer ").into_bytes();
        req.extend_from_slice(&tok);
        req.extend_from_slice(b"\r\n\r\n");
        // server side: parse request bytes
        let mut status = 400;
        let (mut pp, mut rr) = (-1i64, -1i64);
        if req.len() > 16 && req.starts_with(b"GET /v1/product/") {
            let mut q = 16usize;
            pp = scan_int(&req, &mut q);
            q += 8;
            rr = scan_int(&req, &mut q);
            if let (Some(xc), Some(bt)) = (index_of(&req, b"X-Client: ", q), index_of(&req, b"Bearer ", q)) {
                let mut a = xc + 10;
                let kk = scan_int(&req, &mut a);
                let good = kk >= 0 && (kk as usize) < NCLIENT && bt + 7 + 16 <= req.len() && req[bt + 7..bt + 23] == tokens[kk as usize][..];
                status = if good {
                    if (0..NPROD as i64).contains(&pp) && (0..NREG as i64).contains(&rr) && prod_cnt[pp as usize] > 0 { 200 } else { 404 }
                } else {
                    401
                };
            }
        }
        let (head, body) = match status {
            200 => {
                ok200 += 1;
                let pi = pp as usize;
                ("HTTP/1.1 200 OK", format!(
                    "{{\"product\":{pp},\"region\":{rr},\"orders\":{},\"revenue\":{},\"region_revenue\":{},\"forecast\":{},\"root\":\"{root_hex}\"}}",
                    prod_cnt[pi], prod_rev[pi], reg_prod_rev[rr as usize * NPROD + pi], pred_cents[pi]))
            }
            404 => { nf404 += 1; ("HTTP/1.1 404 Not Found", "{\"error\":\"not found\"}".to_string()) }
            401 => { un401 += 1; ("HTTP/1.1 401 Unauthorized", "{\"error\":\"unauthorized\"}".to_string()) }
            _ => ("HTTP/1.1 400 Bad Request", "{\"error\":\"bad request\"}".to_string()),
        };
        let resp = format!("{head}\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        total_bytes += resp.len() as i64;
        resp_cs = (resp_cs * 131 + crc32(resp.as_bytes()) as i64) % MOD;
    }

    // ---- checksum ----
    for v in [ncust, rows, null_cust, bad_qty, valid] { fold(v); }
    for g in 0..NGRP { fold(grp_cnt[g]); fold(grp_qty[g]); fold(grp_rev[g]); }
    for p in 0..NPROD { fold(prod_cnt[p]); fold(prod_rev[p]); }
    for v in [w, bias, loss] { fold(v); }
    for b in root { fold(b as i64); }
    for v in [ok200, un401, nf404, total_bytes, resp_cs] { fold(v); }
    println!("CHECKSUM={cs}");
}
