//! 02_mandelbrot — 1024×1024 maxiter 128, std::thread static row partition
use std::thread;

const W: usize = 1024;
const H: usize = 1024;
const MAXIT: i32 = 128;

fn row(y: usize) -> i64 {
    let cy = (y as f64 / H as f64) * 2.0 - 1.0;
    let mut local = 0i64;
    for x in 0..W {
        let cx = (x as f64 / W as f64) * 3.5 - 2.5;
        let (mut zx, mut zy) = (0.0f64, 0.0f64);
        let mut it = 0;
        while it < MAXIT {
            let (zx2, zy2) = (zx * zx, zy * zy);
            if zx2 + zy2 > 4.0 {
                break;
            }
            let nzy = 2.0 * zx * zy + cy;
            zx = zx2 - zy2 + cx;
            zy = nzy;
            it += 1;
        }
        local += it as i64;
    }
    local
}

fn main() {
    let nt = thread::available_parallelism().map_or(1, |n| n.get()).min(H);
    let chunk = H.div_ceil(nt);
    let cs: i64 = thread::scope(|s| {
        let handles: Vec<_> = (0..nt)
            .map(|t| {
                let (lo, hi) = (t * chunk, ((t + 1) * chunk).min(H));
                s.spawn(move || (lo..hi).map(row).sum::<i64>())
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).sum()
    });
    println!("CHECKSUM={cs}");
}
