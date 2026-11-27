/* 16_crypto_pq_argon2_suite — Argon2id-like KDF (m=8KiB t=2 p=1) x32, SHA-256 Merkle of tags,
   toy lattice negacyclic NTT poly-mul (q=12289, n=256) */
import java.security.MessageDigest

const val MOD = 1_000_000_007L
const val NH = 32
const val BW = 64
const val NB = 32
const val PASSES = 2
const val Q = 12289L
const val N = 256
const val PAIRS = 6
const val M32 = 0xFFFFFFFFL

var rng = 16001L
fun nextRand(): Long {
    rng = rng * 48271L % 2147483647L
    return rng
}

val md: MessageDigest = MessageDigest.getInstance("SHA-256")
fun sha256(b: ByteArray): ByteArray = md.digest(b)

fun rot32(x: Long, r: Int): Long = ((x ushr r) or (x shl (32 - r))) and M32
fun blamka(x: Long, y: Long): Long = (x + y + 2 * (x and 0xFFFF) * (y and 0xFFFF)) and M32

fun gMix(z: LongArray, ia: Int, ib: Int, ic: Int, id: Int) {
    var a = z[ia]; var b = z[ib]; var c = z[ic]; var d = z[id]
    a = blamka(a, b); d = rot32(d xor a, 16)
    c = blamka(c, d); b = rot32(b xor c, 12)
    a = blamka(a, b); d = rot32(d xor a, 8)
    c = blamka(c, d); b = rot32(b xor c, 7)
    z[ia] = a; z[ib] = b; z[ic] = c; z[id] = d
}

fun pRound(z: LongArray, base: Int, stride: Int) {
    gMix(z, base, base + 2 * stride, base + 4 * stride, base + 6 * stride)
    gMix(z, base + stride, base + 3 * stride, base + 5 * stride, base + 7 * stride)
    gMix(z, base, base + 3 * stride, base + 4 * stride, base + 7 * stride)
    gMix(z, base + stride, base + 2 * stride, base + 5 * stride, base + 6 * stride)
}

fun compress(x: LongArray, y: LongArray, out: LongArray) {
    val r = LongArray(BW) { x[it] xor y[it] }
    r.copyInto(out)
    for (row in 0 until 8) pRound(out, row * 8, 1)
    for (col in 0 until 8) pRound(out, col, 8)
    for (i in 0 until BW) out[i] = out[i] xor r[i]
}

fun initBlock(h0: ByteArray, j: Int): LongArray {
    val d = sha256(h0 + byteArrayOf(j.toByte()))
    val w = LongArray(BW)
    for (k in 0 until 8) {
        w[k] = ((d[4 * k].toLong() and 0xff) shl 24) or ((d[4 * k + 1].toLong() and 0xff) shl 16) or
            ((d[4 * k + 2].toLong() and 0xff) shl 8) or (d[4 * k + 3].toLong() and 0xff)
    }
    for (k in 8 until BW) w[k] = (w[k - 8] + (w[k - 3] and 0xFFFF) * 40503 + (w[k - 1] shr 16) * 69069) and M32
    return w
}

fun argon2Like(idxPw: Int): ByteArray {
    val msg = ByteArray(36)
    msg[0] = idxPw.toByte(); msg[1] = (NB * 256 / 1024).toByte(); msg[2] = PASSES.toByte(); msg[3] = 1
    for (i in 4 until 36) msg[i] = (nextRand() and 255).toByte()
    val h0 = sha256(msg)
    val b = Array(NB) { LongArray(BW) }
    b[0] = initBlock(h0, 0)
    b[1] = initBlock(h0, 1)
    val nw = LongArray(BW)
    for (pass in 0 until PASSES) {
        for (idx in (if (pass == 0) 2 else 0) until NB) {
            val prev = b[(idx + NB - 1) % NB]
            val r = if (pass == 0 && idx < NB / 2) ((pass * 131L + idx) * 40503L + 12345L) * 69069L % 4294967296L else prev[0]
            val r16 = r shr 16
            val x = (r16 * r16) shr 16
            val ref: Int
            if (pass == 0) {
                val area = (idx - 1).toLong()
                ref = (area - 1 - ((area * x) shr 16)).toInt()
            } else {
                val area = (NB - 2).toLong()
                ref = ((idx + 1 + (area - 1 - ((area * x) shr 16))) % NB).toInt()
            }
            compress(prev, b[ref], nw)
            if (pass == 0) nw.copyInto(b[idx]) else for (i in 0 until BW) b[idx][i] = b[idx][i] xor nw[i]
        }
    }
    val bytes = ByteArray(BW * 4)
    for (i in 0 until BW) {
        val w = b[NB - 1][i]
        bytes[4 * i] = (w shr 24).toByte(); bytes[4 * i + 1] = (w shr 16).toByte()
        bytes[4 * i + 2] = (w shr 8).toByte(); bytes[4 * i + 3] = w.toByte()
    }
    return sha256(bytes)
}

fun powMod(b0: Long, e0: Long): Long {
    var r = 1L; var b = b0 % Q; var e = e0
    while (e > 0) {
        if (e and 1L == 1L) r = r * b % Q
        b = b * b % Q
        e = e shr 1
    }
    return r
}

fun ntt(a: LongArray, w: Long) {
    var j = 0
    for (i in 1 until N) {
        var bit = N shr 1
        while (j and bit != 0) { j = j xor bit; bit = bit shr 1 }
        j = j xor bit
        if (i < j) { val t = a[i]; a[i] = a[j]; a[j] = t }
    }
    var len = 2
    while (len <= N) {
        val wl = powMod(w, (N / len).toLong())
        val half = len / 2
        var i = 0
        while (i < N) {
            var wn = 1L
            for (k in 0 until half) {
                val u = a[i + k]
                val v = a[i + k + half] * wn % Q
                a[i + k] = (u + v) % Q
                a[i + k + half] = (u + Q - v) % Q
                wn = wn * wl % Q
            }
            i += len
        }
        len = len shl 1
    }
}

fun polymul(a0: LongArray, b0: LongArray, psi: Long): LongArray {
    val psiInv = powMod(psi, Q - 2)
    val omega = psi * psi % Q
    val nInv = powMod(N.toLong(), Q - 2)
    val a = LongArray(N); val b = LongArray(N); val c = LongArray(N)
    var p = 1L
    for (i in 0 until N) { a[i] = a0[i] * p % Q; b[i] = b0[i] * p % Q; p = p * psi % Q }
    ntt(a, omega)
    ntt(b, omega)
    for (i in 0 until N) c[i] = a[i] * b[i] % Q
    ntt(c, powMod(omega, Q - 2))
    p = nInv
    for (i in 0 until N) { c[i] = c[i] * p % Q; p = p * psiInv % Q }
    return c
}

fun schoolbook(a: LongArray, b: LongArray): LongArray {
    val c = LongArray(N)
    for (i in 0 until N) for (j in 0 until N) {
        val pr = a[i] * b[j] % Q
        val k = i + j
        if (k >= N) c[k - N] = (c[k - N] + Q - pr) % Q else c[k] = (c[k] + pr) % Q
    }
    return c
}

fun main() {
    var level = Array(NH) { argon2Like(it) }
    while (level.size > 1) {
        val m = (level.size + 1) / 2
        val cur = level
        level = Array(m) { i -> sha256(cur[2 * i] + cur[if (2 * i + 1 < cur.size) 2 * i + 1 else 2 * i]) }
    }
    var cs = 0L
    for (byte in level[0]) cs = (cs * 131 + (byte.toLong() and 0xff)) % MOD
    val psi = powMod(11, (Q - 1) / (2 * N))
    var ok = if (powMod(psi, N.toLong()) == Q - 1) 1L else 0L
    for (pr in 0 until PAIRS) {
        val a = LongArray(N) { nextRand() % Q }
        val b = LongArray(N) { nextRand() % Q }
        val c = polymul(a, b, psi)
        if (pr < 2 && !schoolbook(a, b).contentEquals(c)) ok = 0
        for (v in c) cs = (cs * 131 + v) % MOD
    }
    cs = (cs * 131 + ok) % MOD
    println("CHECKSUM=$cs")
}
