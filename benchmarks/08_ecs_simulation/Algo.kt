/* 08_ecs — 2000 entities × 20 ticks, spatial grid neighbor queries */
import kotlin.math.floor

fun main() {
    val n = 2000
    val ticks = 20
    val cell = 10
    val grid = 64
    val gridCells = grid * grid
    val lcgMod = 2147483648L
    var rng = 15482888L
    fun nextU(): Long {
        rng = (rng * 1103515245L + 12345L) % lcgMod
        return rng
    }
    fun nextF() = nextU().toDouble() / 2147483648.0

    val px = DoubleArray(n)
    val py = DoubleArray(n)
    val vx = DoubleArray(n)
    val vy = DoubleArray(n)
    val hp = IntArray(n)
    for (i in 0 until n) {
        px[i] = nextF() * 600.0
        py[i] = nextF() * 600.0
        vx[i] = (nextF() - 0.5) * 2.0
        vy[i] = (nextF() - 0.5) * 2.0
        hp[i] = 10 + (nextU().toULong() % 20u).toInt()
    }
    val heads = IntArray(gridCells) { -1 }
    val next = IntArray(n)

    repeat(ticks) {
        for (i in 0 until n) {
            px[i] += vx[i]
            py[i] += vy[i]
            if (px[i] < 0 || px[i] > 600) vx[i] = -vx[i]
            if (py[i] < 0 || py[i] > 600) vy[i] = -vy[i]
        }
        heads.fill(-1)
        for (i in 0 until n) {
            var cx = (px[i] / cell).toInt()
            var cy = (py[i] / cell).toInt()
            if (cx < 0) cx = 0
            if (cy < 0) cy = 0
            if (cx >= grid) cx = grid - 1
            if (cy >= grid) cy = grid - 1
            val c = cy * grid + cx
            next[i] = heads[c]
            heads[c] = i
        }
        for (i in 0 until n) {
            var cx = (px[i] / cell).toInt()
            var cy = (py[i] / cell).toInt()
            if (cx < 0) cx = 0
            if (cy < 0) cy = 0
            if (cx >= grid) cx = grid - 1
            if (cy >= grid) cy = grid - 1
            var hits = 0
            for (oy in -1..1) {
                for (ox in -1..1) {
                    val nx = cx + ox
                    val ny = cy + oy
                    if (nx < 0 || ny < 0 || nx >= grid || ny >= grid) continue
                    var id = heads[ny * grid + nx]
                    while (id >= 0) {
                        if (id != i) {
                            val dx = px[i] - px[id]
                            val dy = py[i] - py[id]
                            if (dx * dx + dy * dy < 4.0) hits++
                        }
                        id = next[id]
                    }
                }
            }
            if (hits > 3) hp[i]--
        }
    }
    var cs = 0L
    for (i in 0 until n) {
        cs += hp[i]
        cs += floor(px[i] * 10.0).toLong()
        cs += floor(py[i] * 10.0).toLong()
    }
    println("CHECKSUM=$cs")
}
