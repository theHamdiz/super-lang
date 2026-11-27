/* 09_matmul_tiled — 256×256 f64 tiled multiply */
import kotlin.math.floor
import kotlin.math.sqrt

fun main() {
    val n = 256
    val tile = 32
    val a = DoubleArray(n * n) { i -> ((i * 17) % 100) / 100.0 }
    val b = DoubleArray(n * n) { i -> ((i * 31) % 100) / 100.0 }
    val c = DoubleArray(n * n)

    var ii = 0
    while (ii < n) {
        var jj = 0
        while (jj < n) {
            var kk = 0
            while (kk < n) {
                val iimax = minOf(ii + tile, n)
                val jjmax = minOf(jj + tile, n)
                val kkmax = minOf(kk + tile, n)
                for (i in ii until iimax) {
                    for (k in kk until kkmax) {
                        val aik = a[i * n + k]
                        for (j in jj until jjmax) {
                            c[i * n + j] += aik * b[k * n + j]
                        }
                    }
                }
                kk += tile
            }
            jj += tile
        }
        ii += tile
    }

    var fro = 0.0
    for (v in c) fro += v * v
    val cs = floor(sqrt(fro) * 1000.0).toLong()
    println("CHECKSUM=$cs")
}