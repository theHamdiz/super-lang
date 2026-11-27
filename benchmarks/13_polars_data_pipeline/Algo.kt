/* 13_polars_data_pipeline — 64k orders: filter -> inner join -> group_by aggregates -> rolling-mean window */
import kotlin.math.floor

const val ROWS = 64000
const val CUSTOMERS = 2000
const val KNOWN = 1800
const val REGIONS = 16

var rng = 130013L
fun lcg(): Long {
    rng = (rng * 1103515245L + 12345L) % 2147483648L
    return rng
}

fun main() {
    val cust = LongArray(ROWS)
    val qty = LongArray(ROWS)
    val amtq = LongArray(ROWS)
    val amt = DoubleArray(ROWS)
    for (i in 0 until ROWS) {
        val c = lcg() / 256 % CUSTOMERS
        val q = 1 + lcg() / 256 % 9
        val p = lcg() / 256 % 400
        cust[i] = c; qty[i] = q; amtq[i] = q * p; amt[i] = (q * p).toDouble() * 0.25
    }
    // dimension table: direct-address hash on customer id
    val regionOf = LongArray(CUSTOMERS) { -1L }
    for (c in 0 until KNOWN) regionOf[c] = lcg() / 256 % 16

    var big = 0L
    var joined = 0L
    var rollSum = 0L
    var rollCnt = 0L
    val tot = LongArray(REGIONS)
    val n = LongArray(REGIONS)
    val mx = LongArray(REGIONS)
    val sumAmt = DoubleArray(REGIONS)
    val ring = Array(REGIONS) { DoubleArray(8) }
    val seen = LongArray(REGIONS)
    for (i in 0 until ROWS) {
        if (!(amt[i] >= 20.0 && qty[i] > 1)) continue
        big++
        val r = regionOf[cust[i].toInt()].toInt()
        if (r < 0) continue
        joined++
        tot[r] += amtq[i]
        n[r] += 1
        sumAmt[r] += amt[i]
        if (n[r] == 1L || amtq[i] > mx[r]) mx[r] = amtq[i]
        ring[r][(seen[r] % 8).toInt()] = amt[i]
        seen[r]++
        if (seen[r] >= 8) {
            var s = 0.0
            for (k in 0 until 8) s += ring[r][((seen[r] - 8 + k) % 8).toInt()]
            rollSum += floor(s / 8.0 * 1024.0).toLong()
            rollCnt++
        }
    }
    var cs = big + joined * 3
    for (r in 0 until REGIONS) {
        if (n[r] == 0L) continue
        val avg = sumAmt[r] / n[r].toDouble()
        cs += (r + 1) * (tot[r] + 7 * n[r] + floor(avg * 1024.0).toLong() + 13 * mx[r])
    }
    cs += rollSum + 5 * rollCnt
    println("CHECKSUM=$cs")
}
