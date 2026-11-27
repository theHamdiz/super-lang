/* 12_sparse_fem_fft — 128x128 reaction-Poisson CSR, Jacobi-PCG x20, 2D FFT low-pass bank */
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.floor
import kotlin.math.sin
import kotlin.math.sqrt

const val G = 128
const val N = 16384
const val ITERS = 20

val indptr = IntArray(N + 1)
val indices = IntArray(5 * N)
val values = DoubleArray(5 * N)
val diag = DoubleArray(N)

fun buildSystem() {
    var nz = 0
    for (i in 0 until N) {
        val row = i / G
        val col = i % G
        val d = 4.0 + (i * 37 % 11) * 0.25
        diag[i] = d
        if (row > 0) { indices[nz] = i - G; values[nz++] = -1.0 }
        if (col > 0) { indices[nz] = i - 1; values[nz++] = -1.0 }
        indices[nz] = i; values[nz++] = d
        if (col < G - 1) { indices[nz] = i + 1; values[nz++] = -1.0 }
        if (row < G - 1) { indices[nz] = i + G; values[nz++] = -1.0 }
        indptr[i + 1] = nz
    }
}

fun matvec(x: DoubleArray, out: DoubleArray) {
    for (r in 0 until N) {
        var s = 0.0
        for (k in indptr[r] until indptr[r + 1]) s += values[k] * x[indices[k]]
        out[r] = s
    }
}

fun dot(a: DoubleArray, b: DoubleArray): Double {
    var s = 0.0
    for (i in 0 until N) s += a[i] * b[i]
    return s
}

fun pcg(b: DoubleArray, x: DoubleArray): Double {
    val r = DoubleArray(N)
    val z = DoubleArray(N)
    val p = DoubleArray(N)
    val ap = DoubleArray(N)
    for (i in 0 until N) { x[i] = 0.0; r[i] = b[i]; z[i] = r[i] / diag[i]; p[i] = z[i] }
    var rz = dot(r, z)
    for (it in 0 until ITERS) {
        matvec(p, ap)
        val alpha = rz / dot(p, ap)
        for (i in 0 until N) {
            x[i] += alpha * p[i]
            r[i] -= alpha * ap[i]
            z[i] = r[i] / diag[i]
        }
        val rzNew = dot(r, z)
        val beta = rzNew / rz
        for (i in 0 until N) p[i] = z[i] + beta * p[i]
        rz = rzNew
    }
    return dot(r, r)
}

val bitrev = IntArray(G) { i ->
    var r = 0
    var v = i
    for (b in 0 until 7) { r = (r shl 1) or (v and 1); v = v shr 1 }
    r
}
val tre = DoubleArray(G)
val tim = DoubleArray(G)

fun fft1(re: DoubleArray, im: DoubleArray, inv: Boolean) {
    if (inv) for (i in 0 until G) im[i] = 0.0 - im[i]
    for (i in 0 until G) { val s = bitrev[i]; tre[i] = re[s]; tim[i] = im[s] }
    var span = 2
    while (span <= G) {
        val half = span / 2
        val ang = 0.0 - 2.0 * PI / span.toDouble()
        var start = 0
        while (start < G) {
            for (k in 0 until half) {
                val wr = cos(ang * k.toDouble())
                val wi = sin(ang * k.toDouble())
                val a = start + k
                val b = a + half
                val tr = tre[b] * wr - tim[b] * wi
                val ti = tre[b] * wi + tim[b] * wr
                tre[b] = tre[a] - tr; tim[b] = tim[a] - ti
                tre[a] = tre[a] + tr; tim[a] = tim[a] + ti
            }
            start += span
        }
        span *= 2
    }
    if (inv) {
        val s = 1.0 / G.toDouble()
        for (i in 0 until G) { re[i] = tre[i] * s; im[i] = (0.0 - tim[i]) * s }
    } else {
        for (i in 0 until G) { re[i] = tre[i]; im[i] = tim[i] }
    }
}

fun fft2(re: DoubleArray, im: DoubleArray, inv: Boolean) {
    val lr = DoubleArray(G)
    val li = DoubleArray(G)
    for (r in 0 until G) {
        for (c in 0 until G) { lr[c] = re[r * G + c]; li[c] = im[r * G + c] }
        fft1(lr, li, inv)
        for (c in 0 until G) { re[r * G + c] = lr[c]; im[r * G + c] = li[c] }
    }
    for (c in 0 until G) {
        for (r in 0 until G) { lr[r] = re[r * G + c]; li[r] = im[r * G + c] }
        fft1(lr, li, inv)
        for (r in 0 until G) { re[r * G + c] = lr[r]; im[r * G + c] = li[r] }
    }
}

fun main() {
    val b = DoubleArray(N) { i -> ((i * 7 + (i / G) * 13) % 17 - 8) / 8.0 }
    val x = DoubleArray(N)
    buildSystem()
    val res = sqrt(pcg(b, x))

    val sre = x.copyOf()
    val sim = DoubleArray(N)
    fft2(sre, sim, false)

    val cuts = intArrayOf(64, 256, 1024)
    val fre = DoubleArray(N)
    val fim = DoubleArray(N)
    var energy = 0.0
    var corner = 0.0
    for (q in 0 until 3) {
        sre.copyInto(fre)
        sim.copyInto(fim)
        for (k in 0 until N) {
            val ky = k / G
            val kx = k % G
            val fy = if (ky <= G / 2) ky else G - ky
            val fx = if (kx <= G / 2) kx else G - kx
            if (fx * fx + fy * fy > cuts[q]) { fre[k] = 0.0; fim[k] = 0.0 }
        }
        fft2(fre, fim, true)
        for (k in 0 until N) energy += fre[k] * fre[k]
        corner += fre[0] + fre[N / 2 + G / 2] + fre[N - 1]
    }

    val xs = dot(x, x)
    val cs = floor(sqrt(xs) * 1000000.0).toLong() + floor(energy * 1000.0).toLong() +
        floor(corner * 1000000.0).toLong() + floor(res * 1000000000.0).toLong()
    println("CHECKSUM=$cs")
}
