/* 20_end_to_end_lakehouse_service — in-process pipeline:
   ingest JSONL customers + CSV order batches -> filter/join/group -> fixed-point GD linear fit on the
   per-product aggregates -> serve HTTP-like request bytes (bearer-token check via SHA-256, Merkle root of
   the group table in every response) -> checksum of response bytes. */
import java.security.MessageDigest

const val MOD = 1_000_000_007L
const val NCUST = 500
const val NPROD = 50
const val NREG = 8
const val NTIER = 4
const val NGRP = NREG * NTIER
const val BATCHES = 6
const val ROWS_PB = 4000
const val NREQ = 3000
const val NCLIENT = 8
const val ITERS = 400

var rng = 20020L
fun nextRand(): Long { rng = rng * 48271 % 2147483647; return rng }
fun sdiv(a: Long, d: Long): Long = if (a >= 0) a / d else -((-a) / d)

val md: MessageDigest = MessageDigest.getInstance("SHA-256")
fun sha256(b: ByteArray): ByteArray = md.digest(b)
fun hex(d: ByteArray, n: Int): String {
    val sb = StringBuilder()
    for (i in 0 until n) sb.append("%02x".format(d[i].toInt() and 255))
    return sb.toString()
}

val CRC = IntArray(256).also { t ->
    for (i in 0 until 256) {
        var c = i
        repeat(8) { c = if ((c and 1) == 1) (c ushr 1) xor 0xEDB88320.toInt() else c ushr 1 }
        t[i] = c
    }
}
fun crc32(d: ByteArray): Long {
    var c = -1
    for (b in d) c = CRC[(c xor b.toInt()) and 255] xor (c ushr 8)
    return (c xor -1).toLong() and 0xFFFFFFFFL
}

/** Parse the decimal run at pos[0] (advancing it); -1 when there are no digits. */
fun scanInt(b: ByteArray, pos: IntArray): Long {
    var v = 0L
    var have = false
    while (pos[0] < b.size && b[pos[0]] >= 48 && b[pos[0]] <= 57) {
        v = v * 10 + (b[pos[0]] - 48); pos[0]++; have = true
    }
    return if (have) v else -1
}
fun isDigit(c: Byte) = c >= 48 && c <= 57

fun indexOf(b: ByteArray, blen: Int, pat: ByteArray, from: Int): Int {
    var i = from
    while (i + pat.size <= blen) {
        var ok = true
        for (j in pat.indices) if (b[i + j] != pat[j]) { ok = false; break }
        if (ok) return i
        i++
    }
    return -1
}

fun main() {
    var cs = 0L
    fun fold(v: Long) { cs = (cs * 131 + v) % MOD }

    // ---- ingest customers (JSONL) ----
    val sbc = StringBuilder()
    for (i in 0 until NCUST) {
        val id = i.toLong() * 7919 % NCUST; val r = nextRand() % NREG; val t = nextRand() % NTIER
        sbc.append("{\"id\":$id,\"region\":$r,\"tier\":$t}\n")
    }
    val cb = (sbc.toString() + "\u0000").toByteArray(Charsets.US_ASCII)
    val clen = cb.size - 1
    val regionOf = LongArray(NCUST) { -1 }; val tierOf = LongArray(NCUST) { -1 }
    var ncust = 0L
    val cur = IntArray(1)
    while (cur[0] < clen) {
        while (cur[0] < clen && !isDigit(cb[cur[0]])) cur[0]++
        val id = scanInt(cb, cur)
        while (!isDigit(cb[cur[0]])) cur[0]++
        val r = scanInt(cb, cur)
        while (!isDigit(cb[cur[0]])) cur[0]++
        val t = scanInt(cb, cur)
        regionOf[id.toInt()] = r; tierOf[id.toInt()] = t; ncust++
        while (cur[0] < clen && cb[cur[0]].toInt() != 10) cur[0]++
        cur[0]++
    }

    // ---- ingest + filter/join/group order batches (CSV) ----
    val grpCnt = LongArray(NGRP); val grpQty = LongArray(NGRP); val grpRev = LongArray(NGRP)
    val prodCnt = LongArray(NPROD); val prodQty = LongArray(NPROD); val prodRev = LongArray(NPROD); val prodPrice = LongArray(NPROD)
    val regProdRev = LongArray(NREG * NPROD)
    var oid = 0L; var rows = 0L; var nullCust = 0L; var badQty = 0L; var valid = 0L
    val f = LongArray(5)
    repeat(BATCHES) {
        val sb = StringBuilder("order_id,cust_id,product,qty,price_cents\n")
        repeat(ROWS_PB) {
            val nf = nextRand() % 100; val c = nextRand() % NCUST; val p = nextRand() % NPROD; val q = nextRand() % 9
            val pr = 100 + p * 37 + nextRand() % 200
            sb.append(oid++).append(',').append(if (nf != 0L) c.toString() else "").append(',')
                .append(p).append(',').append(q).append(',').append(pr).append('\n')
        }
        val ob = (sb.toString() + "\u0000").toByteArray(Charsets.US_ASCII)
        val olen = ob.size - 1
        val oc = IntArray(1)
        while (ob[oc[0]].toInt() != 10) oc[0]++
        oc[0]++
        while (oc[0] < olen) {
            for (k in 0 until 5) { f[k] = scanInt(ob, oc); oc[0]++ }
            rows++
            if (f[1] < 0) { nullCust++; continue }
            if (f[3] <= 0) { badQty++; continue }
            valid++
            val rev = f[3] * f[4]
            val g = (regionOf[f[1].toInt()] * NTIER + tierOf[f[1].toInt()]).toInt()
            grpCnt[g]++; grpQty[g] += f[3]; grpRev[g] += rev
            val pd = f[2].toInt()
            prodCnt[pd]++; prodQty[pd] += f[3]; prodRev[pd] += rev; prodPrice[pd] += f[4]
            regProdRev[(regionOf[f[1].toInt()] * NPROD).toInt() + pd] += rev
        }
    }

    // ---- fit y = w*x + b (w in Q16) by gradient descent on per-product aggregates ----
    val xs = ArrayList<Long>(); val ys = ArrayList<Long>()
    for (p in 0 until NPROD) if (prodCnt[p] > 0) {
        xs.add(prodPrice[p] / prodCnt[p] / 16); ys.add(prodRev[p] / prodCnt[p] / 16)
    }
    val n = xs.size
    var w = 0L; var bias = 0L
    repeat(ITERS) {
        var gw = 0L; var gb = 0L
        for (i in 0 until n) {
            val err = sdiv(w * xs[i], 65536) + bias - ys[i]
            gw += err * xs[i]; gb += err
        }
        w -= sdiv(gw * 65536, n.toLong() * 8000)
        bias -= sdiv(gb, n.toLong() * 4)
    }
    var loss = 0L
    for (i in 0 until n) {
        val err = sdiv(w * xs[i], 65536) + bias - ys[i]
        loss += err * err
    }
    val predCents = LongArray(NPROD)
    for (p in 0 until NPROD) {
        val x = if (prodCnt[p] > 0) prodPrice[p] / prodCnt[p] / 16 else 0L
        val y = sdiv(w * x, 65536) + bias
        predCents[p] = (if (y > 0) y else 0L) * 16
    }

    // ---- merkle root over the region x tier group table ----
    var level = Array(NGRP) { g ->
        val leaf = ByteArray(32)
        val vals = longArrayOf(g.toLong(), grpCnt[g], grpQty[g], grpRev[g])
        for (k in 0 until 4) for (j in 0 until 8) leaf[k * 8 + j] = (vals[k] ushr (56 - 8 * j)).toByte()
        sha256(leaf)
    }
    while (level.size > 1) {
        val cur0 = level
        level = Array(cur0.size / 2) { i -> sha256(cur0[2 * i] + cur0[2 * i + 1]) }
    }
    val root = level[0]
    val rootHex = hex(root, 32)

    // ---- client tokens ----
    val tokens = Array(NCLIENT) { k -> hex(sha256("key-$k".toByteArray()), 8).toByteArray(Charsets.US_ASCII) }

    // ---- serve ----
    var ok200 = 0L; var un401 = 0L; var nf404 = 0L; var totalBytes = 0L; var respCs = 0L
    val pGet = "GET /v1/product/".toByteArray(); val pXc = "X-Client: ".toByteArray(); val pBearer = "Bearer ".toByteArray()
    repeat(NREQ) {
        val k = (nextRand() % NCLIENT).toInt(); val p = nextRand() % 60; val rg = nextRand() % NREG
        val bad = nextRand() % 100; val at = (nextRand() % 16).toInt()
        val tok = tokens[k].copyOf()
        if (bad < 8) tok[at] = 122
        val req = "GET /v1/product/$p?region=$rg HTTP/1.1\r\nHost: lake\r\nX-Client: $k\r\nAuthorization: Bearer "
            .toByteArray() + tok + "\r\n\r\n".toByteArray()
        val rl = req.size
        // server side: parse request bytes
        var status = 400; var pp = -1L; var rr = -1L
        if (rl > 16 && indexOf(req, 16, pGet, 0) == 0) {
            val q = intArrayOf(16)
            pp = scanInt(req, q)
            q[0] += 8
            rr = scanInt(req, q)
            val xc = indexOf(req, rl, pXc, q[0]); val bt = indexOf(req, rl, pBearer, q[0])
            if (xc >= 0 && bt >= 0) {
                val a = intArrayOf(xc + 10)
                val kk = scanInt(req, a)
                var good = kk >= 0 && kk < NCLIENT && bt + 7 + 16 <= rl
                if (good) for (j in 0 until 16) if (req[bt + 7 + j] != tokens[kk.toInt()][j]) good = false
                status = if (good) {
                    if (pp >= 0 && pp < NPROD && rr >= 0 && rr < NREG && prodCnt[pp.toInt()] > 0) 200 else 404
                } else 401
            }
        }
        val body: String; val head: String
        when (status) {
            200 -> {
                val pi = pp.toInt()
                body = "{\"product\":$pp,\"region\":$rr,\"orders\":${prodCnt[pi]},\"revenue\":${prodRev[pi]}," +
                    "\"region_revenue\":${regProdRev[(rr * NPROD).toInt() + pi]},\"forecast\":${predCents[pi]},\"root\":\"$rootHex\"}"
                ok200++; head = "HTTP/1.1 200 OK"
            }
            404 -> { body = "{\"error\":\"not found\"}"; nf404++; head = "HTTP/1.1 404 Not Found" }
            401 -> { body = "{\"error\":\"unauthorized\"}"; un401++; head = "HTTP/1.1 401 Unauthorized" }
            else -> { body = "{\"error\":\"bad request\"}"; head = "HTTP/1.1 400 Bad Request" }
        }
        val resp = "$head\r\nContent-Length: ${body.length}\r\n\r\n$body".toByteArray(Charsets.US_ASCII)
        totalBytes += resp.size
        respCs = (respCs * 131 + crc32(resp)) % MOD
    }

    // ---- checksum ----
    for (v in longArrayOf(ncust, rows, nullCust, badQty, valid)) fold(v)
    for (g in 0 until NGRP) { fold(grpCnt[g]); fold(grpQty[g]); fold(grpRev[g]) }
    for (p in 0 until NPROD) { fold(prodCnt[p]); fold(prodRev[p]) }
    for (v in longArrayOf(w, bias, loss)) fold(v)
    for (b in root) fold((b.toInt() and 255).toLong())
    for (v in longArrayOf(ok200, un401, nf404, totalBytes, respCs)) fold(v)
    println("CHECKSUM=$cs")
}
