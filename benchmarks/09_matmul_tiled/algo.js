/* 09_matmul_tiled — 256×256 f64 tiled multiply */
const N = 256;
const TILE = 32;

const A = new Float64Array(N * N);
const B = new Float64Array(N * N);
const C = new Float64Array(N * N);

for (let i = 0; i < N * N; i++) {
  A[i] = ((i * 17) % 100) / 100.0;
  B[i] = ((i * 31) % 100) / 100.0;
}

for (let ii = 0; ii < N; ii += TILE) {
  for (let jj = 0; jj < N; jj += TILE) {
    for (let kk = 0; kk < N; kk += TILE) {
      const iimax = ii + TILE < N ? ii + TILE : N;
      const jjmax = jj + TILE < N ? jj + TILE : N;
      const kkmax = kk + TILE < N ? kk + TILE : N;
      for (let i = ii; i < iimax; i++) {
        for (let k = kk; k < kkmax; k++) {
          const aik = A[i * N + k];
          for (let j = jj; j < jjmax; j++) {
            C[i * N + j] += aik * B[k * N + j];
          }
        }
      }
    }
  }
}

let fro = 0.0;
for (let i = 0; i < N * N; i++) fro += C[i] * C[i];
const cs = Math.floor(Math.sqrt(fro) * 1000.0);
console.log(`CHECKSUM=${cs}`);