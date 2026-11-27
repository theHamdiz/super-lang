/* 05_parallel_mapreduce — 1 MiB corpus, word + 3-gram frequencies */
import java.util.concurrent.Executors

fun main() {
    val corpus = 65536
    val buckets = 65536
    val words = arrayOf(
        "alpha", "bravo", "charlie", "delta", "echo",
        "foxtrot", "golf", "hotel", "india", "juliet",
    )

    fun hashBytes(data: ByteArray, start: Int, n: Int): Int {
        var h = 2166136261.toUInt()
        for (i in 0 until n) {
            h = h xor (data[start + i].toInt() and 0xff).toUInt()
            h *= 16777619u
        }
        return h.toInt()
    }

    class Tab { var key = 0; var count = 0 }

    fun bump(tab: Array<Tab>, key: Int, add: Int) {
        var i = key and (buckets - 1)
        for (probe in 0 until buckets) {
            val j = (i + probe) and (buckets - 1)
            if (tab[j].count == 0) {
                tab[j].key = key
                tab[j].count = add
                return
            }
            if (tab[j].key == key) {
                tab[j].count += add
                return
            }
        }
    }

    fun worker(text: ByteArray, begin: Int, end: Int): Array<Tab> {
        val tab = Array(buckets) { Tab() }
        var i = begin
        while (i < end) {
            while (i < end) {
                val c = text[i].toInt() and 0xff
                if (c in 'a'.code..'z'.code) break
                i++
            }
            val start = i
            while (i < end) {
                val c = text[i].toInt() and 0xff
                if (c !in 'a'.code..'z'.code) break
                i++
            }
            if (i > start) {
                val len = i - start
                bump(tab, hashBytes(text, start, len), 1)
                if (len >= 3) bump(tab, hashBytes(text, start, 3) xor 0xC0FFEE, 1)
            }
        }
        return tab
    }

    fun scanCorpus(text: ByteArray): Array<Tab> {
        val merged = Array(buckets) { Tab() }
        var i = 0
        while (i < corpus) {
            while (i < corpus) {
                val c = text[i].toInt() and 0xff
                if (c in 'a'.code..'z'.code) break
                i++
            }
            val start = i
            while (i < corpus) {
                val c = text[i].toInt() and 0xff
                if (c !in 'a'.code..'z'.code) break
                i++
            }
            if (i > start) {
                val len = i - start
                bump(merged, hashBytes(text, start, len), 1)
                if (len >= 3) bump(merged, hashBytes(text, start, 3) xor 0xC0FFEE, 1)
            }
        }
        return merged
    }

    fun stats(merged: Array<Tab>): Triple<Int, Int, Int> {
        var total = 0
        var best = 0
        var bestKey = 0u
        for (slot in merged) {
            if (slot.count == 0) continue
            total += slot.count
            val key = slot.key.toUInt()
            if (slot.count > best || (slot.count == best && key < bestKey)) {
                best = slot.count
                bestKey = key
            }
        }
        return Triple(total, best, bestKey.toInt())
    }

    val text = ByteArray(corpus + 1)
    val lcgMod = 2147483648L
    var rng = 370945L
    var off = 0
    while (off + 16 < corpus) {
        rng = (rng * 1103515245L + 12345L) % lcgMod
        val w = words[(rng % 10).toInt()]
        val bytes = w.toByteArray()
        System.arraycopy(bytes, 0, text, off, bytes.size)
        off += bytes.size
        text[off++] = ' '.code.toByte()
    }
    while (off < corpus) text[off++] = 'x'.code.toByte()

    var nworkers = Runtime.getRuntime().availableProcessors()
    if (nworkers < 1) nworkers = 1
    if (nworkers > 32) nworkers = 32
    val chunk = corpus / nworkers
    val pool = Executors.newFixedThreadPool(nworkers)
    val futures = (0 until nworkers).map { wid ->
        val begin = wid * chunk
        val end = if (wid == nworkers - 1) corpus else begin + chunk
        pool.submit<Array<Tab>> { worker(text, begin, end) }
    }
    futures.forEach { it.get() }
    pool.shutdown()

    val (total, best, bestKey) = stats(scanCorpus(text))
    val cs = total.toLong() xor (best.toLong() * 1315423911L) xor (bestKey.toLong() and 0xFFFFFFFFL)
    println("CHECKSUM=$cs")
}