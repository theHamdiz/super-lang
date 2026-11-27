// 13_polars_data_pipeline — 64k orders: filter -> inner join -> group_by aggregates -> rolling-mean window
const ROWS: usize = 64000;
const CUSTOMERS: usize = 2000;
const KNOWN: usize = 1800;
const REGIONS: usize = 16;

struct Lcg(i64);
impl Lcg {
    fn next(&mut self) -> i64 {
        self.0 = (self.0 * 1103515245 + 12345) % 2147483648;
        self.0
    }
}

#[derive(Default, Clone, Copy)]
struct Group {
    tot: i64,
    n: i64,
    mx: i64,
    sum_amt: f64,
    seen: usize,
    ring: [f64; 8],
}

fn main() {
    let mut rng = Lcg(130013);
    let mut cust = vec![0usize; ROWS];
    let mut qty = vec![0i64; ROWS];
    let mut amtq = vec![0i64; ROWS];
    let mut amt = vec![0.0f64; ROWS];
    for i in 0..ROWS {
        let c = (rng.next() / 256 % CUSTOMERS as i64) as usize;
        let q = 1 + rng.next() / 256 % 9;
        let p = rng.next() / 256 % 400;
        cust[i] = c;
        qty[i] = q;
        amtq[i] = q * p;
        amt[i] = (q * p) as f64 * 0.25;
    }
    // dimension table: direct-address hash on customer id
    let mut region_of = vec![-1i64; CUSTOMERS];
    for slot in region_of.iter_mut().take(KNOWN) {
        *slot = rng.next() / 256 % 16;
    }

    let (mut big, mut joined, mut roll_sum, mut roll_cnt) = (0i64, 0i64, 0i64, 0i64);
    let mut groups = [Group::default(); REGIONS];
    for i in 0..ROWS {
        if !(amt[i] >= 20.0 && qty[i] > 1) {
            continue;
        }
        big += 1;
        let r = region_of[cust[i]];
        if r < 0 {
            continue;
        }
        joined += 1;
        let g = &mut groups[r as usize];
        g.tot += amtq[i];
        g.n += 1;
        g.sum_amt += amt[i];
        if g.n == 1 || amtq[i] > g.mx {
            g.mx = amtq[i];
        }
        g.ring[g.seen % 8] = amt[i];
        g.seen += 1;
        if g.seen >= 8 {
            let mut s = 0.0;
            for k in 0..8 {
                s += g.ring[(g.seen - 8 + k) % 8];
            }
            roll_sum += (s / 8.0 * 1024.0).floor() as i64;
            roll_cnt += 1;
        }
    }
    let mut cs = big + joined * 3;
    for (r, g) in groups.iter().enumerate() {
        if g.n == 0 {
            continue;
        }
        let avg = g.sum_amt / g.n as f64;
        cs += (r as i64 + 1) * (g.tot + 7 * g.n + (avg * 1024.0).floor() as i64 + 13 * g.mx);
    }
    cs += roll_sum + 5 * roll_cnt;
    println!("CHECKSUM={cs}");
}
