/* 10_sha256_merkle — 1024 leaves × 64 bytes, SHA-256 Merkle root */
import java.security.MessageDigest

fun main() {
    val leaves = 1024
    val leafSz = 64
    val md = MessageDigest.getInstance("SHA-256")

    var level = Array(leaves) { ByteArray(32) }
    val leaf = ByteArray(leafSz)
    for (i in 0 until leaves) {
        for (b in 0 until leafSz) leaf[b] = ((i * 131 + b * 17) and 255).toByte()
        level[i] = md.digest(leaf)
    }

    var n = leaves
    while (n > 1) {
        val m = (n + 1) / 2
        val next = Array(m) { ByteArray(32) }
        for (i in 0 until m) {
            val cat = ByteArray(64)
            System.arraycopy(level[2 * i], 0, cat, 0, 32)
            if (2 * i + 1 < n) System.arraycopy(level[2 * i + 1], 0, cat, 32, 32)
            else System.arraycopy(level[2 * i], 0, cat, 32, 32)
            next[i] = md.digest(cat)
        }
        level = next
        n = m
    }

    var cs = 0L
    for (b in level[0]) cs = cs * 131 + (b.toLong() and 0xff)
    println("CHECKSUM=$cs")
}