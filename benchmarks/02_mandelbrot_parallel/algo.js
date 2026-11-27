/* 02_mandelbrot — 1024×1024 maxiter 128 */
const W = 1024;
const H = 1024;
const MAXIT = 128;

let cs = 0;
for (let y = 0; y < H; y++) {
  let local = 0;
  const cy = (y / H) * 2.0 - 1.0;
  for (let x = 0; x < W; x++) {
    const cx = (x / W) * 3.5 - 2.5;
    let zx = 0.0,
      zy = 0.0;
    let it = 0;
    for (; it < MAXIT; it++) {
      const zx2 = zx * zx,
        zy2 = zy * zy;
      if (zx2 + zy2 > 4.0) break;
      const nzy = 2.0 * zx * zy + cy;
      zx = zx2 - zy2 + cx;
      zy = nzy;
    }
    local += it;
  }
  cs += local;
}
console.log(`CHECKSUM=${cs}`);