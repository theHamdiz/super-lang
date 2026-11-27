/* 17_ai_tensor_hnsw_search — fixed-point (Q10) transformer blocks (causal attention, softmax,
   ReLU FFN, RMS norm) on 12 sequences, then brute-force top-10 cosine search over 4096 unit
   vectors of dim 32 using the pooled embeddings as queries. Integer-only => bit-exact everywhere. */
const val MOD = 1_000_000_007L
const val BIAS = 1L shl 40
const val D = 32
const val T = 16
const val FF = 64
const val LAYERS = 2
const val SEQS = 12
const val NV = 4096
const val K = 10

var rng = 17017L
fun nextRand(): Long {
    rng = rng * 48271L % 2147483647L
    return rng
}

fun isqrt(n: Long): Long {
    if (n == 0L) return 0
    var x = n
    var y = (x + 1) shr 1
    while (y < x) { x = y; y = (x + n / x) shr 1 }
    return x
}

fun expFrac(f: Long): Long {
    var term = 1024L
    var sum = 1024L
    for (k in 1..6) {
        term = ((term * f) shr 10) / k
        if (k and 1 == 1) sum -= term else sum += term
    }
    return if (sum < 0) 0 else sum
}

fun expQ(x: Long, e1: Long): Long {
    val a = -x
    val n = (a shr 10).toInt()
    if (n >= 16) return 0
    var r = expFrac(a and 1023)
    repeat(n) { r = (r * e1) shr 10 }
    return r
}

fun matmul(x: LongArray, w: LongArray, y: LongArray, din: Int, dout: Int) {
    for (t in 0 until T) for (o in 0 until dout) {
        var s = 0L
        for (i in 0 until din) s += x[t * din + i] * w[i * dout + o]
        y[t * dout + o] = s shr 10
    }
}

fun rmsNorm(h: LongArray) {
    for (t in 0 until T) {
        var ss = 0L
        for (d in 0 until D) ss += h[t * D + d] * h[t * D + d]
        val rms = isqrt(ss / D)
        val inv = (1L shl 20) / (rms + 1)
        for (d in 0 until D) h[t * D + d] = (h[t * D + d] * inv) shr 10
    }
}

fun fillW(n: Int) = LongArray(n) { nextRand() % 257 - 128 }

class Layer {
    val wq = fillW(D * D); val wk = fillW(D * D); val wv = fillW(D * D)
    val wo = fillW(D * D); val w1 = fillW(D * FF); val w2 = fillW(FF * D)
}

fun block(ly: Layer, h: LongArray, e1: Long) {
    val q = LongArray(T * D); val k = LongArray(T * D); val v = LongArray(T * D)
    val at = LongArray(T * D); val o = LongArray(T * D); val f = LongArray(T * FF); val g = LongArray(T * D)
    matmul(h, ly.wq, q, D, D)
    matmul(h, ly.wk, k, D, D)
    matmul(h, ly.wv, v, D, D)
    val sc = LongArray(T); val pr = LongArray(T)
    for (t in 0 until T) {
        var mx = Long.MIN_VALUE
        var sum = 0L
        for (j in 0..t) {
            var s = 0L
            for (d in 0 until D) s += q[t * D + d] * k[j * D + d]
            sc[j] = s shr 13
            if (sc[j] > mx) mx = sc[j]
        }
        for (j in 0..t) { pr[j] = expQ(sc[j] - mx, e1); sum += pr[j] }
        for (j in 0..t) pr[j] = pr[j] * 1024 / sum
        for (d in 0 until D) {
            var s = 0L
            for (j in 0..t) s += pr[j] * v[j * D + d]
            at[t * D + d] = s shr 10
        }
    }
    matmul(at, ly.wo, o, D, D)
    for (i in 0 until T * D) h[i] += o[i]
    rmsNorm(h)
    matmul(h, ly.w1, f, D, FF)
    for (i in 0 until T * FF) if (f[i] < 0) f[i] = 0
    matmul(f, ly.w2, g, FF, D)
    for (i in 0 until T * D) h[i] += g[i]
    rmsNorm(h)
}

fun normalize(raw: LongArray): LongArray {
    var ss = 0L
    for (v in raw) ss += v * v
    val inv = (1L shl 20) / (isqrt(ss) + 1)
    return LongArray(D) { (raw[it] * inv) shr 10 }
}

fun main() {
    val e1 = expFrac(1024)
    val layers = Array(LAYERS) { Layer() }
    val db = Array(NV) { normalize(LongArray(D) { nextRand() % 2049 - 1024 }) }
    var cs = 0L
    fun fold(v: Long) { cs = (cs * 131 + v + BIAS) % MOD }
    repeat(SEQS) {
        val h = LongArray(T * D) { nextRand() % 2049 - 1024 }
        for (ly in layers) block(ly, h, e1)
        val pooled = LongArray(D)
        for (d in 0 until D) {
            var sum = 0L
            for (t in 0 until T) sum += h[t * D + d]
            pooled[d] = sum shr 4
            fold(pooled[d])
        }
        val query = normalize(pooled)
        val topS = LongArray(K); val topI = LongArray(K)
        var cnt = 0
        for (i in 0 until NV) {
            var sc = 0L
            val row = db[i]
            for (d in 0 until D) sc += query[d] * row[d]
            if (cnt < K || sc > topS[K - 1]) {
                var p = if (cnt < K) cnt else K - 1
                while (p > 0 && topS[p - 1] < sc) { topS[p] = topS[p - 1]; topI[p] = topI[p - 1]; p-- }
                topS[p] = sc; topI[p] = i.toLong()
                if (cnt < K) cnt++
            }
        }
        for (j in 0 until K) { fold(topI[j]); fold(topS[j]) }
    }
    println("CHECKSUM=$cs")
}
