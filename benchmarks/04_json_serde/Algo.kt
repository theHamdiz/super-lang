
fun main() {
    val n = 10000
    var rng = 4861952L
    fun lcg(): Long { rng = (rng * 1103515245L + 12345L) % 2147483648L; return rng }
    var cs = 0L
    repeat(n) {
        val id = lcg() % 1000000L
        val total = lcg() % 100000L
        val items = lcg() % 8L + 1L
        cs += id + total + items * 17L
        cs += id + total + items * 17L
    }
    println("CHECKSUM=$cs")
}
