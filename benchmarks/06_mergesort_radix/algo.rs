//! 06_mergesort — top-down merge sort of 50000 u64, verify + checksum
const N: usize = 50_000;

fn merge(a: &mut [u64], tmp: &mut [u64], mid: usize) {
    let n = a.len();
    let (mut i, mut j, mut k) = (0, mid, 0);
    while i < mid && j < n {
        if a[i] <= a[j] {
            tmp[k] = a[i];
            i += 1;
        } else {
            tmp[k] = a[j];
            j += 1;
        }
        k += 1;
    }
    if i < mid {
        tmp[k..k + mid - i].copy_from_slice(&a[i..mid]);
        k += mid - i;
    }
    if j < n {
        tmp[k..k + n - j].copy_from_slice(&a[j..n]);
    }
    a.copy_from_slice(&tmp[..n]);
}

fn mergesort(a: &mut [u64], tmp: &mut [u64]) {
    let n = a.len();
    if n <= 1 {
        return;
    }
    let mid = n / 2;
    {
        let (l, r) = a.split_at_mut(mid);
        let (tl, tr) = tmp.split_at_mut(mid);
        mergesort(l, tl);
        mergesort(r, tr);
    }
    merge(a, tmp, mid);
}

fn main() {
    let mut rng: u64 = 395799;
    let mut a: Vec<u64> = (0..N)
        .map(|_| {
            rng = (rng.wrapping_mul(1103515245).wrapping_add(12345)) % 2147483648;
            rng & 0xffff_ffff
        })
        .collect();
    let mut tmp = vec![0u64; N];
    mergesort(&mut a, &mut tmp);
    let mut cs: i64 = 0;
    for &v in &a {
        cs ^= (v & 0xffff_ffff) as i64;
        cs = cs.wrapping_mul(131).wrapping_add((v >> 32) as i64);
    }
    let ok = a.windows(2).all(|w| w[0] <= w[1]);
    println!("CHECKSUM={}", cs ^ i64::from(ok));
}
