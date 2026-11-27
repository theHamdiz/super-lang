/* 19_distributed_raft_wal_kv — single-process Raft log replication (leader + 2 followers):
   CRC32-protected WAL records, lossy/corrupting virtual links, majority commit, KV state machine. */
const val MOD = 1_000_000_007L
const val TICKS = 6000
const val DRAIN = 200
const val REC = 20
const val BATCH = 4
const val KEYS = 512
const val DROP_PCT = 35
const val ACK_DROP_PCT = 15
const val CORRUPT_PCT = 3
const val CAP = TICKS * REC

val CRC = IntArray(256).also { t ->
    for (i in 0 until 256) {
        var c = i
        repeat(8) { c = if ((c and 1) == 1) (c ushr 1) xor 0xEDB88320.toInt() else c ushr 1 }
        t[i] = c
    }
}

fun crc32(d: ByteArray, off: Int, n: Int): Long {
    var c = -1
    for (i in off until off + n) c = CRC[(c xor d[i].toInt()) and 255] xor (c ushr 8)
    return (c xor -1).toLong() and 0xFFFFFFFFL
}

fun put32(p: ByteArray, o: Int, v: Long) {
    p[o] = (v shr 24).toByte(); p[o + 1] = (v shr 16).toByte(); p[o + 2] = (v shr 8).toByte(); p[o + 3] = v.toByte()
}

fun get32(p: ByteArray, o: Int): Long =
    ((p[o].toLong() and 255) shl 24) or ((p[o + 1].toLong() and 255) shl 16) or
        ((p[o + 2].toLong() and 255) shl 8) or (p[o + 3].toLong() and 255)

var rng = 19019L
fun nextRand(): Long { rng = rng * 48271 % 2147483647; return rng }

val wal = Array(3) { ByteArray(CAP) }
val kv = Array(3) { LongArray(KEYS) }
val lens = IntArray(3)
val applied = IntArray(3)
val match = IntArray(3)
val nxt = intArrayOf(1, 1, 1)

fun applyUpto(n: Int, upto: Int) {
    val w = wal[n]
    while (applied[n] < upto) {
        val o = applied[n] * REC
        if (crc32(w, o, 16) == get32(w, o + 16)) kv[n][(get32(w, o + 8) % KEYS).toInt()] = get32(w, o + 12)
        applied[n]++
    }
}

fun main() {
    var commit = 0
    var sent = 0L; var dropped = 0L; var crcRejects = 0L; var acksLost = 0L
    val msg = ByteArray(BATCH * REC)
    for (t in 0 until TICKS + DRAIN) {
        if (t < TICKS) {
            val w = wal[0]; val o = lens[0] * REC
            put32(w, o, lens[0] + 1L)
            put32(w, o + 4, 1)
            put32(w, o + 8, nextRand() % KEYS)
            put32(w, o + 12, nextRand() % 1000000)
            put32(w, o + 16, crc32(w, o, 16))
            lens[0]++
        }
        val leaderCommit = commit
        for (f in 1..2) {
            val n0 = nxt[f]
            if (n0 > lens[0]) continue
            val cnt = minOf(lens[0] - n0 + 1, BATCH)
            val r1 = nextRand() % 100; val r2 = nextRand() % 100; val r3 = nextRand() % 100
            sent++
            if (r1 < DROP_PCT) { dropped++; continue }
            System.arraycopy(wal[0], (n0 - 1) * REC, msg, 0, cnt * REC)
            if (r2 < CORRUPT_PCT) {
                val pos = (nextRand() % (cnt * REC)).toInt()
                msg[pos] = (msg[pos].toInt() xor 0x5A).toByte()
            }
            for (j in 0 until cnt) {
                val o = j * REC
                if (crc32(msg, o, 16) != get32(msg, o + 16)) { crcRejects++; break }
                val idx = get32(msg, o)
                if (idx == lens[f] + 1L) {
                    System.arraycopy(msg, o, wal[f], lens[f] * REC, REC)
                    lens[f]++
                } else if (idx > lens[f] + 1L) break
            }
            applyUpto(f, minOf(leaderCommit, lens[f]))
            if (r3 < ACK_DROP_PCT) { acksLost++; continue }
            if (lens[f] > match[f]) match[f] = lens[f]
            nxt[f] = lens[f] + 1
        }
        val med = intArrayOf(lens[0], match[1], match[2]).sorted()[1]
        if (med > commit) commit = med
        applyUpto(0, commit)
    }
    var cs = 0L
    fun fold(v: Long) { cs = (cs * 131 + v) % MOD }
    for (v in longArrayOf(lens[0].toLong(), lens[1].toLong(), lens[2].toLong(), commit.toLong(),
        applied[0].toLong(), applied[1].toLong(), applied[2].toLong(), sent, dropped, crcRejects, acksLost)) fold(v)
    for (n in 0 until 3) {
        var d = 0L
        for (k in 0 until KEYS) d = (d * 131 + kv[n][k]) % MOD
        fold(d)
    }
    for (n in 0 until 2) fold(crc32(wal[n], 0, lens[n] * REC))
    println("CHECKSUM=$cs")
}
