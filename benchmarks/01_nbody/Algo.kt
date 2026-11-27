/* 01_nbody — 64 bodies × 500 steps, soft gravity, SoA */
fun main() {
    val n = 64
    val steps = 500
    val dt = 0.001
    val soft = 0.01
    val lcgMod = 2147483648L
    var rng = 12648430L
    fun nextU(): Long {
        rng = (rng * 1103515245L + 12345L) % lcgMod
        return rng
    }
    fun nextF() = nextU().toDouble() / 2147483648.0

    val x = DoubleArray(n)
    val y = DoubleArray(n)
    val vx = DoubleArray(n)
    val vy = DoubleArray(n)
    val m = DoubleArray(n)
    for (i in 0 until n) {
        x[i] = nextF() * 2.0 - 1.0
        y[i] = nextF() * 2.0 - 1.0
        vx[i] = (nextF() - 0.5) * 0.01
        vy[i] = (nextF() - 0.5) * 0.01
        m[i] = 0.5 + nextF()
    }

    repeat(steps) {
        for (i in 0 until n) {
            var ax = 0.0
            var ay = 0.0
            for (j in 0 until n) {
                if (i == j) continue
                val dx = x[j] - x[i]
                val dy = y[j] - y[i]
                val d2 = dx * dx + dy * dy + soft
                val inv = 1.0 / kotlin.math.sqrt(d2)
                val inv3 = inv * inv * inv
                ax += m[j] * dx * inv3
                ay += m[j] * dy * inv3
            }
            vx[i] += ax * dt
            vy[i] += ay * dt
        }
        for (i in 0 until n) {
            x[i] += vx[i] * dt
            y[i] += vy[i] * dt
        }
    }

    var cs = 0L
    for (i in 0 until n) {
        cs += kotlin.math.floor(x[i] * 1000.0).toLong()
        cs += kotlin.math.floor(y[i] * 1000.0).toLong()
        cs += kotlin.math.floor(vx[i] * 1e6).toLong()
    }
    println("CHECKSUM=$cs")
}