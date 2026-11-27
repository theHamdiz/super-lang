//! 09_matmul_tiled — 256×256 f64 cache-tiled multiply (i-k-j order inside tiles)
const N: usize = 256;
const TILE: usize = 32;

fn main() {
    let a: Vec<f64> = (0..N * N).map(|i| ((i * 17) % 100) as f64 / 100.0).collect();
    let b: Vec<f64> = (0..N * N).map(|i| ((i * 31) % 100) as f64 / 100.0).collect();
    let mut c = vec![0f64; N * N];

    for ii in (0..N).step_by(TILE) {
        for jj in (0..N).step_by(TILE) {
            for kk in (0..N).step_by(TILE) {
                let (iimax, jjmax, kkmax) = ((ii + TILE).min(N), (jj + TILE).min(N), (kk + TILE).min(N));
                for i in ii..iimax {
                    let crow = &mut c[i * N + jj..i * N + jjmax];
                    for k in kk..kkmax {
                        let aik = a[i * N + k];
                        let brow = &b[k * N + jj..k * N + jjmax];
                        for (cv, &bv) in crow.iter_mut().zip(brow) {
                            *cv += aik * bv;
                        }
                    }
                }
            }
        }
    }
    let fro: f64 = c.iter().fold(0.0, |s, &v| s + v * v);
    let cs = (fro.sqrt() * 1000.0).floor() as i64;
    println!("CHECKSUM={cs}");
}
