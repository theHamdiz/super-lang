/* 07_crc32 — IEEE CRC32 on expanded LCG-scrambled block (parity with py/js/sr/c) */
fun main() {
    fun crc32(data: ByteArray): Long {
        val table = IntArray(256)
        for (i in 0 until 256) {
            var c = i
            repeat(8) {
                c = if (c and 1 != 0) (0xEDB88320.toInt() xor (c ushr 1)) else (c ushr 1)
            }
            table[i] = c
        }
        var crc = -1
        for (b in data) {
            val ub = b.toInt() and 0xff
            crc = table[(crc xor ub) and 0xff] xor (crc ushr 8)
        }
        return (crc.inv().toLong()) and 0xffffffffL
    }

    var seed = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.."
    var block = seed
    repeat(6) { block = block + block }

    var rng = 8137778L
    var acc = 0L
    for (i in 0 until 4096) {
        rng = (rng * 1103515245L + 12345L) % 2147483648L
        acc += (rng % 255L) + block[i % block.length].code.toLong()
    }
    val payload = block.repeat(9).toByteArray(Charsets.ISO_8859_1)
    val blockBytes = block.toByteArray(Charsets.ISO_8859_1)
    val c1 = crc32(payload)
    val c2 = crc32(blockBytes)
    val cs = c1 + c2 + (acc % 1000L)
    println("CHECKSUM=$cs")
}
