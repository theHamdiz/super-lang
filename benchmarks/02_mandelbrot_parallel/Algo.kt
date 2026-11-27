/* 02_mandelbrot — 1024×1024 maxiter 128, parallel by row */
import java.util.concurrent.Executors
import java.util.concurrent.Future

fun main() {
    val w = 1024
    val h = 1024
    val maxIt = 128

  fun mandelbrotRow(y: Int): Long {
        var local = 0L
        val cy = (y.toDouble() / h) * 2.0 - 1.0
        for (x in 0 until w) {
            val cx = (x.toDouble() / w) * 3.5 - 2.5
            var zx = 0.0
            var zy = 0.0
            var it = 0
            while (it < maxIt) {
                val zx2 = zx * zx
                val zy2 = zy * zy
                if (zx2 + zy2 > 4.0) break
                val nzy = 2.0 * zx * zy + cy
                zx = zx2 - zy2 + cx
                zy = nzy
                it++
            }
            local += it
        }
        return local
    }

    val workers = Runtime.getRuntime().availableProcessors().coerceAtLeast(1)
    val pool = Executors.newFixedThreadPool(workers)
  val futures = ArrayList<Future<Long>>(h)
    for (y in 0 until h) {
    futures.add(pool.submit<Long> { mandelbrotRow(y) })
    }
    var cs = 0L
  for (f in futures) cs += f.get()
    pool.shutdown()
    println("CHECKSUM=$cs")
}