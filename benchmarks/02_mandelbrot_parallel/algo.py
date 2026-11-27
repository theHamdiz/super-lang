#!/usr/bin/env python3
"""02_mandelbrot — 1024×1024 maxiter 128, parallel by row."""

import os
from concurrent.futures import ProcessPoolExecutor

W = 1024
H = 1024
MAXIT = 128


def mandelbrot_row(y: int) -> int:
    local = 0
    cy = (y / H) * 2.0 - 1.0
    for x in range(W):
        cx = (x / W) * 3.5 - 2.5
        zx = 0.0
        zy = 0.0
        it = 0
        while it < MAXIT:
            zx2 = zx * zx
            zy2 = zy * zy
            if zx2 + zy2 > 4.0:
                break
            nzy = 2.0 * zx * zy + cy
            zx = zx2 - zy2 + cx
            zy = nzy
            it += 1
        local += it
    return local


def main():
    workers = os.cpu_count() or 1
    cs = 0
    with ProcessPoolExecutor(max_workers=workers) as pool:
        for local in pool.map(mandelbrot_row, range(H)):
            cs += local
    print(f"CHECKSUM={cs}")


if __name__ == "__main__":
    main()