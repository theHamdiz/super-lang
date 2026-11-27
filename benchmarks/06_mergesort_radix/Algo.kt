/* 06_mergesort_radix — sort 50000 u64, verify + checksum */
fun main() {
    val n = 50_000
    val lcgMod = 2147483648L
    var rng = 395799L
    fun nextU(): Long {
        rng = (rng * 1103515245L + 12345L) % lcgMod
        return rng
    }

    val a = LongArray(n) { nextU() }
    val tmp = LongArray(n)

    fun merge(lo: Int, mid: Int, hi: Int) {
        var i = lo
        var j = mid
        var k = lo
        while (i < mid && j < hi) {
            if (java.lang.Long.compareUnsigned(a[i], a[j]) <= 0) tmp[k++] = a[i++] else tmp[k++] = a[j++]
        }
        while (i < mid) tmp[k++] = a[i++]
        while (j < hi) tmp[k++] = a[j++]
        for (t in lo until hi) a[t] = tmp[t]
    }

    fun mergesort(lo: Int, hi: Int) {
        if (hi - lo <= 1) return
        val mid = lo + (hi - lo) / 2
        mergesort(lo, mid)
        mergesort(mid, hi)
        merge(lo, mid, hi)
    }

    mergesort(0, n)

    var cs = 0L
    var ok = 1
    for (i in 0 until n) {
        cs = cs xor (a[i].toULong() and 0xFFFFFFFFUL).toLong()
        cs = cs * 131 + (a[i] ushr 32)
        if (i > 0 && java.lang.Long.compareUnsigned(a[i], a[i - 1]) < 0) ok = 0
    }
    println("CHECKSUM=${cs xor if (ok == 1) 1L else 0L}")
}