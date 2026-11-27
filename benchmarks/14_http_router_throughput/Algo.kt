/* 14_http_router_throughput - 30k simulated HTTP requests: parse bytes, radix route match,
   HMAC-SHA256 bearer-token check, JSON body validation, status-code checksum */
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

const val NREQ = 30000
const val NOW = 1700000000L
const val HEX = "0123456789abcdef"

val KEY = ByteArray(32) { ((it * 37 + 11) % 256).toByte() }
val MAC: Mac = Mac.getInstance("HmacSHA256").also { it.init(SecretKeySpec(KEY, "HmacSHA256")) }

var rng = 140014L
fun draw(m: Long): Long {
    rng = (rng * 1103515245L + 12345L) % 2147483648L
    return rng / 256 % m
}

fun macOf(msg: ByteArray, off: Int, len: Int): ByteArray {
    MAC.update(msg, off, len)
    return MAC.doFinal()
}

class Bytes(cap: Int = 512) {
    var a = ByteArray(cap)
    var n = 0
    fun lit(s: String) { for (c in s) a[n++] = c.code.toByte() }
    fun byte(c: Int) { a[n++] = c.toByte() }
    fun dec(v: Long) = lit(v.toString())
    fun chars(cnt: Long) { for (i in 0 until cnt) byte('a'.code + draw(26).toInt()) }
    fun add(o: Bytes) { System.arraycopy(o.a, 0, a, n, o.n); n += o.n }
    fun id() {
        val bad = draw(50)
        val idv = 1 + draw(99999)
        if (bad == 0L) lit("abc") else dec(idv)
    }
}

val METHODS = arrayOf("GET ", "POST ", "PUT ", "DELETE ")

fun buildRequest(r: Bytes) {
    r.n = 0
    val path = Bytes(); val body = Bytes(); val tok = Bytes()
    var method = 0
    var hasBody = false
    val a = draw(100)
    when {
        a < 5 -> path.lit("/health")
        a < 15 -> path.lit("/users")
        a < 35 -> { path.lit("/users/"); path.id() }
        a < 45 -> { path.lit("/users/"); path.id(); path.lit("/orders") }
        a < 55 -> { method = 1; hasBody = true; path.lit("/orders") }
        a < 65 -> { path.lit("/orders/"); path.id() }
        a < 70 -> { method = 2; hasBody = true; path.lit("/orders/"); path.id() }
        a < 75 -> { method = 3; path.lit("/orders/"); path.id() }
        a < 85 -> {
            path.lit("/static/a"); path.dec(draw(10))
            path.lit("/f"); path.dec(draw(100)); path.lit(".js")
        }
        a < 90 -> { method = 1; hasBody = true; path.lit("/auth/login") }
        a < 95 -> {
            path.lit("/v1/items/"); path.id()
            path.lit("/tags/t"); path.dec(draw(50))
        }
        a < 98 -> { path.lit("/nope/"); path.id() }
        else -> { method = 3; path.lit("/users") }
    }

    val t = draw(20)
    val uid = 1 + draw(9999)
    val ex = draw(1000)
    val hasTok = t >= 2
    if (hasTok) {
        val exp = if (t == 4L) NOW - 1 - ex else NOW + 1000 + ex
        val sb = Bytes()
        sb.dec(uid); sb.byte('.'.code); sb.dec(exp)
        val mac = macOf(sb.a, 0, sb.n)
        if (t < 4) mac[0] = (mac[0].toInt() xor 1).toByte()
        tok.add(sb); tok.byte('.'.code)
        for (i in 0 until 4) {
            val m = mac[i].toInt() and 255
            tok.byte(HEX[m shr 4].code)
            tok.byte(HEX[m and 15].code)
        }
    }

    if (hasBody) {
        val v = draw(10)
        val ws = draw(2)
        val sp = if (ws == 1L) " " else ""
        if (a >= 45 && a < 55) {
            var user = 1 + draw(9999)
            var qty = 1 + draw(100)
            if (v == 0L) qty = 0
            if (v == 3L) user = 10000
            body.lit("{\"user\":"); body.lit(sp); body.dec(user)
            body.lit(","); body.lit(sp)
            body.lit("\"qty\":"); body.lit(sp); body.dec(qty)
            val sku = Bytes(); sku.chars(4)
            if (v != 1L) {
                body.lit(","); body.lit(sp)
                body.lit("\"sku\":"); body.lit(sp); body.byte('"'.code); body.add(sku); body.byte('"'.code)
            }
            body.byte('}'.code)
            if (v == 2L) body.n--
        } else if (a >= 65 && a < 70) {
            var qty = 1 + draw(100)
            val nl = 3 + draw(5)
            if (v == 0L) qty = 101
            body.lit("{")
            if (v != 1L) {
                body.lit("\"qty\":"); body.lit(sp); body.dec(qty)
                body.lit(","); body.lit(sp)
            }
            body.lit("\"note\":"); body.lit(sp); body.byte('"'.code)
            body.chars(nl)
            body.lit("\"}")
            if (v == 2L) body.n--
        } else {
            val ul = 4 + draw(6)
            val un = Bytes(); un.chars(ul)
            val pwl = 6 + draw(7)
            val pw = Bytes(); pw.chars(pwl)
            if (v == 0L) pw.n = 0
            body.lit("{")
            if (v != 1L) {
                body.lit("\"user\":"); body.lit(sp); body.byte('"'.code); body.add(un)
                body.lit("\","); body.lit(sp)
            }
            body.lit("\"pass\":"); body.lit(sp); body.byte('"'.code); body.add(pw); body.lit("\"}")
            if (v == 2L) body.byte('x'.code)
        }
    }

    r.lit(METHODS[method]); r.add(path)
    r.lit(" HTTP/1.1\r\nHost: api.local\r\n")
    if (hasTok) { r.lit("Authorization: Bearer "); r.add(tok); r.lit("\r\n") }
    if (hasBody) { r.lit("Content-Length: "); r.dec(body.n.toLong()); r.lit("\r\n") }
    r.lit("\r\n"); r.add(body)
}

// ---- radix router ----
class Node(val kind: Int, val lit: String, var first: Int, val next: Int) {
    val h = intArrayOf(-1, -1, -1, -1)
}

val nodes = ArrayList<Node>().also { it.add(Node(0, "", -1, -1)) }

fun addRoute(tpl: String, method: Int, rid: Int) {
    var cur = 0
    for (seg in tpl.substring(1).split("/")) {
        val kind = when (seg[0]) { ':' -> 1; '*' -> 2; else -> 0 }
        val lit = if (kind == 0) seg else ""
        var found = -1
        var c = nodes[cur].first
        while (c >= 0 && found < 0) {
            val nd = nodes[c]
            if (nd.kind == kind && (kind != 0 || nd.lit == lit)) found = c
            c = nd.next
        }
        if (found < 0) {
            nodes.add(Node(kind, lit, -1, nodes[cur].first))
            found = nodes.size - 1
            nodes[cur].first = found
        }
        cur = found
    }
    nodes[cur].h[method] = rid
}

val AUTH = booleanArrayOf(false, true, true, true, true, true, true, true, false, false, true)

fun parseDigits(b: ByteArray, p: Int, e: Int): Long {
    if (e <= p || e - p > 10) return -1
    var v = 0L
    for (i in p until e) {
        val c = b[i].toInt()
        if (c < 48 || c > 57) return -1
        v = v * 10 + (c - 48)
    }
    return v
}

fun startsAt(b: ByteArray, n: Int, p: Int, lit: String): Boolean {
    if (p + lit.length > n) return false
    for (i in lit.indices) if (b[p + i].toInt() != lit[i].code) return false
    return true
}

fun verifyToken(b: ByteArray, off: Int, len: Int): Long {
    val e = off + len
    var d1 = -1; var d2 = -1
    for (i in off until e) {
        if (b[i].toInt() == 46) {
            if (d1 < 0) d1 = i else if (d2 < 0) d2 = i else return -1
        }
    }
    if (d1 < 0 || d2 < 0 || e - d2 - 1 != 8) return -1
    val u = parseDigits(b, off, d1)
    val ex = parseDigits(b, d1 + 1, d2)
    if (u < 0 || ex < 0) return -1
    val mac = macOf(b, off, d2 - off)
    for (i in 0 until 4) {
        val m = mac[i].toInt() and 255
        if (b[d2 + 1 + 2 * i].toInt() != HEX[m shr 4].code || b[d2 + 2 + 2 * i].toInt() != HEX[m and 15].code) return -1
    }
    if (ex <= NOW) return -1
    return u
}

fun skipWs(b: ByteArray, p0: Int, e: Int): Int {
    var p = p0
    while (p < e && (b[p].toInt() == 32 || b[p].toInt() == 9 || b[p].toInt() == 10 || b[p].toInt() == 13)) p++
    return p
}

fun keyId(b: ByteArray, p: Int, e: Int): Int {
    val k = String(b, p, e - p, Charsets.ISO_8859_1)
    return when (k) { "user" -> 1; "qty" -> 2; "sku" -> 3; "note" -> 4; "pass" -> 5; else -> 0 }
}

// f[id*3] present, f[id*3+1] type (1 int, 2 str), f[id*3+2] value / string length
fun parseJson(b: ByteArray, off: Int, len: Int, f: LongArray): Boolean {
    val e = off + len
    f.fill(0)
    var p = skipWs(b, off, e)
    if (p >= e || b[p].toInt() != 123) return false
    p = skipWs(b, p + 1, e)
    if (p < e && b[p].toInt() == 125) {
        p = skipWs(b, p + 1, e)
        return p == e
    }
    while (true) {
        if (p >= e || b[p].toInt() != 34) return false
        p++
        val ks = p
        while (p < e && b[p].toInt() != 34) {
            if (b[p].toInt() == 92) return false
            p++
        }
        if (p >= e) return false
        val id = keyId(b, ks, p)
        p = skipWs(b, p + 1, e)
        if (p >= e || b[p].toInt() != 58) return false
        p = skipWs(b, p + 1, e)
        if (p >= e) return false
        val ty: Long
        val value: Long
        if (b[p].toInt() == 34) {
            p++
            val vs = p
            while (p < e && b[p].toInt() != 34) {
                if (b[p].toInt() == 92) return false
                p++
            }
            if (p >= e) return false
            ty = 2; value = (p - vs).toLong()
            p++
        } else {
            var neg = false
            if (b[p].toInt() == 45) { neg = true; p++ }
            val ds = p
            while (p < e && b[p].toInt() in 48..57) p++
            val v = parseDigits(b, ds, p)
            if (v < 0) return false
            ty = 1; value = if (neg) -v else v
        }
        if (id > 0) { f[id * 3] = 1; f[id * 3 + 1] = ty; f[id * 3 + 2] = value }
        p = skipWs(b, p, e)
        if (p >= e) return false
        if (b[p].toInt() == 44) { p = skipWs(b, p + 1, e); continue }
        if (b[p].toInt() == 125) { p++; break }
        return false
    }
    p = skipWs(b, p, e)
    return p == e
}

fun hasInt(f: LongArray, id: Int, lo: Long, hi: Long) = f[id * 3] != 0L && f[id * 3 + 1] == 1L && f[id * 3 + 2] in lo..hi
fun hasStr(f: LongArray, id: Int, lo: Long, hi: Long) = f[id * 3] != 0L && f[id * 3 + 1] == 2L && f[id * 3 + 2] in lo..hi

val FIELDS = LongArray(18)
fun bodyValid(rid: Int, b: ByteArray, off: Int, len: Int): Boolean {
    val f = FIELDS
    if (!parseJson(b, off, len, f)) return false
    if (rid == 4) return hasInt(f, 1, 1, 9999) && hasInt(f, 2, 1, 100) && hasStr(f, 3, 4, 4)
    if (rid == 6) return hasInt(f, 2, 1, 100) && (f[12] == 0L || hasStr(f, 4, 0, 1000))
    return hasStr(f, 1, 1, 64) && hasStr(f, 5, 1, 64)
}

var pidOut = 0L

fun handle(b: ByteArray, n: Int): Int {
    pidOut = 0
    val method: Int
    var p: Int
    if (startsAt(b, n, 0, "GET ")) { method = 0; p = 4 }
    else if (startsAt(b, n, 0, "POST ")) { method = 1; p = 5 }
    else if (startsAt(b, n, 0, "PUT ")) { method = 2; p = 4 }
    else if (startsAt(b, n, 0, "DELETE ")) { method = 3; p = 7 }
    else return 400
    val po = p
    while (p < n && b[p].toInt() != 32) p++
    val pe = p
    while (p < n && b[p].toInt() != 13) p++
    p += 2

    var tokOff = -1; var tokLen = 0; var clen = -1L
    while (p < n) {
        if (b[p].toInt() == 13) { p += 2; break }
        var le = p
        while (le < n && b[le].toInt() != 13) le++
        if (startsAt(b, n, p, "Authorization: Bearer ")) { tokOff = p + 22; tokLen = le - (p + 22) }
        else if (startsAt(b, n, p, "Content-Length: ")) clen = parseDigits(b, p + 16, le)
        p = le + 2
    }
    val bodyOff = p; val bodyLen = n - p
    if (clen >= 0 && clen != bodyLen.toLong()) return 400

    val plen = pe - po
    var cur = 0; var found = true; var nparam = 0; var badId = false; var pid = -1L
    if (plen < 1 || b[po].toInt() != 47) found = false
    var i = 1
    while (found && i <= plen) {
        var e = i
        while (e < plen && b[po + e].toInt() != 47) e++
        val sl = e - i
        if (sl == 0) { found = false; break }
        var st = -1; var pm = -1; var wc = -1
        var c = nodes[cur].first
        while (c >= 0) {
            val nd = nodes[c]
            if (nd.kind == 0) {
                if (nd.lit.length == sl) {
                    var same = true
                    for (q in 0 until sl) if (nd.lit[q].code != b[po + i + q].toInt()) { same = false; break }
                    if (same) { st = c; break }
                }
            } else if (nd.kind == 1) pm = c
            else wc = c
            c = nd.next
        }
        if (st >= 0) cur = st
        else if (pm >= 0) {
            cur = pm
            if (nparam == 0) {
                pid = parseDigits(b, po + i, po + e)
                if (pid < 0) badId = true
            }
            nparam++
        } else if (wc >= 0) { cur = wc; break }
        else { found = false; break }
        i = e + 1
    }
    if (!found) return 404
    val hs = nodes[cur].h
    val rid = hs[method]
    if (rid < 0) return if (hs.any { it >= 0 }) 405 else 404
    pidOut = if (badId || pid < 0) 0 else pid

    if (AUTH[rid]) {
        if (tokOff < 0) return 401
        val uid = verifyToken(b, tokOff, tokLen)
        if (uid < 0) return 401
        if ((rid == 6 || rid == 7) && uid % 13 == 0L) return 403
    }
    if (badId) return 400
    if ((rid == 4 || rid == 6 || rid == 9) && !bodyValid(rid, b, bodyOff, bodyLen)) return 400
    if (rid == 2 && pid > 90000) return 404
    if (rid == 4) return 201
    if (rid == 5 && pid % 7 == 0L) return 404
    if (rid == 7) return 204
    return 200
}

fun main() {
    addRoute("/health", 0, 0)
    addRoute("/users", 0, 1)
    addRoute("/users/:id", 0, 2)
    addRoute("/users/:id/orders", 0, 3)
    addRoute("/orders", 1, 4)
    addRoute("/orders/:id", 0, 5)
    addRoute("/orders/:id", 2, 6)
    addRoute("/orders/:id", 3, 7)
    addRoute("/static/*path", 0, 8)
    addRoute("/auth/login", 1, 9)
    addRoute("/v1/items/:id/tags/:tag", 0, 10)

    var cs = 0L; var sumPid = 0L; var ok2xx = 0L
    val byStatus = LongArray(600)
    val req = Bytes()
    for (i in 0 until NREQ) {
        buildRequest(req)
        val st = handle(req.a, req.n)
        byStatus[st]++
        cs += st.toLong() * (i % 97 + 1)
        sumPid += pidOut
        if (st < 300) ok2xx++
    }
    var mix = 0L
    for (s in 0 until 600) mix += byStatus[s] * (s % 53 + 1) * (s / 100 + 1)
    cs += sumPid + 5 * ok2xx + 7 * mix
    println("CHECKSUM=$cs")
}
