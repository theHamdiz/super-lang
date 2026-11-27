//! 04_json_serde — LCG field checksum
const N: usize = 10_000;

fn main() {
    let mut rng: u64 = 4861952;
    let mut step = || {
        rng = (rng.wrapping_mul(1103515245).wrapping_add(12345)) % 2147483648;
        rng
    };
    let mut cs: i64 = 0;
    for _ in 0..N {
        let id = (step() % 1_000_000) as i64;
        let total = (step() % 100_000) as i64;
        let items = (step() % 8) as i64 + 1;
        let v = id + total + items * 17;
        cs += v;
        cs += v;
    }
    println!("CHECKSUM={cs}");
}
