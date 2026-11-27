/* 18_stdlib_core_algorithms — one checksum from four std-style kernels:
   (A) KMP-DFA "regex" field extraction over 3000 generated log lines,
   (B) strict UTF-8 validation/counting over 60 documents (some corrupted),
   (C) SQL-ish filter / group-by / order-by-limit / join over 6000 rows x 24 queries,
   (D) HM-style type unification (union-find, iterative) over 8504 type nodes. */
const val MOD = 1_000_000_007L
const val NL = 3000
const val NDOC = 60
const val PERDOC = 200
const val NR = 6000
const val NQ = 24
const val NVARS = 2500
const val NCOMP = 6000
const val NCON = 5000
const val NTOT = NVARS + 4 + NCOMP

var rng = 18018L
fun nextRand(): Long {
    rng = rng * 48271L % 2147483647L
    return rng
}

var cs = 0L
fun fold(v: Long) { cs = (cs * 131 + v) % MOD }

// ---------- (A) log extraction ----------
fun buildDfa(needle: ByteArray): IntArray {
    val n = needle.size
    val dfa = IntArray(n * 256)
    dfa[needle[0].toInt()] = 1
    var x = 0
    for (j in 1 until n) {
        System.arraycopy(dfa, x * 256, dfa, j * 256, 256)
        dfa[j * 256 + needle[j]] = j + 1
        x = dfa[x * 256 + needle[j]]
    }
    return dfa
}

fun extract(line: ByteArray, dfa: IntArray, n: Int): Long {
    var st = 0
    var i = 0
    val e = line.size
    while (i < e) {
        st = dfa[st * 256 + (line[i].toInt() and 0xff)]
        i++
        if (st == n) {
            var v = 0L
            var nd = 0
            while (i < e && line[i] in 48..57) { v = v * 10 + (line[i] - 48); nd++; i++ }
            if (nd > 0) return v
            st = 0
        }
    }
    return -1
}

fun partA() {
    val levels = arrayOf("INFO", "WARN", "ERROR", "DEBUG")
    val codes = intArrayOf(200, 201, 204, 301, 400, 401, 404, 500, 503)
    val dStatus = buildDfa("status=".toByteArray()); val dTook = buildDfa("took=".toByteArray()); val dUser = buildDfa("user=".toByteArray())
    val cls = LongArray(8)
    var tookSum = 0L; var tookMax = 0L; var tookN = 0L; var userSum = 0L; var bad = 0L; var total = 0L
    fun p2(v: Long) = v.toString().padStart(2, '0')
    for (i in 0 until NL) {
        val mo = 1 + nextRand() % 12; val dd = 1 + nextRand() % 28; val hh = nextRand() % 24
        val mi = nextRand() % 60; val ss = nextRand() % 60; val lv = (nextRand() % 4).toInt()
        val svc = nextRand() % 100; val uid = nextRand() % 100000; val sidx = (nextRand() % 9).toInt(); val ms = nextRand() % 3000
        val sb = StringBuilder("2026-${p2(mo)}-${p2(dd)} ${p2(hh)}:${p2(mi)}:${p2(ss)} ${levels[lv]} svc-${p2(svc)} user=$uid")
        sb.append(if (i % 13 == 5) " status=abc" else " status=${codes[sidx]}")
        if (i % 17 != 3) sb.append(" took=${ms}ms")
        val line = sb.toString().toByteArray()
        val st = extract(line, dStatus, 7); val tk = extract(line, dTook, 5); val us = extract(line, dUser, 5)
        if (st < 0) bad++ else cls[(st / 100).toInt()]++
        if (tk >= 0) { tookSum += tk; tookN++; if (tk > tookMax) tookMax = tk }
        userSum += us
        total += line.size + 1
    }
    for (c in cls) fold(c)
    for (v in longArrayOf(tookSum, tookMax, tookN, userSum, bad, total)) fold(v)
}

// ---------- (B) UTF-8 ----------
fun cont(b: Int) = b and 0xC0 == 0x80

fun utf8Valid(d: IntArray, n: Int): Boolean {
    var i = 0
    while (i < n) {
        val b0 = d[i]
        if (b0 < 128) { i++; continue }
        if (b0 < 194) return false
        if (b0 < 224) {
            if (i + 1 >= n || !cont(d[i + 1])) return false
            i += 2
        } else if (b0 < 240) {
            if (i + 2 >= n || !cont(d[i + 1]) || !cont(d[i + 2])) return false
            val cp = (b0 - 224) * 4096 + (d[i + 1] - 128) * 64 + (d[i + 2] - 128)
            if (cp < 2048 || cp in 55296..57343) return false
            i += 3
        } else if (b0 < 245) {
            if (i + 3 >= n || !cont(d[i + 1]) || !cont(d[i + 2]) || !cont(d[i + 3])) return false
            val cp = (b0 - 240) * 262144 + (d[i + 1] - 128) * 4096 + (d[i + 2] - 128) * 64 + (d[i + 3] - 128)
            if (cp < 65536 || cp > 1114111) return false
            i += 4
        } else return false
    }
    return true
}

fun utf8Count(d: IntArray, n: Int): Int {
    var c = 0
    for (i in 0 until n) if (!cont(d[i])) c++
    return c
}

fun encode(cp: Long, o: IntArray, at: Int): Int {
    val c = cp.toInt()
    if (c < 128) { o[at] = c; return 1 }
    if (c < 2048) { o[at] = 192 + c / 64; o[at + 1] = 128 + c % 64; return 2 }
    if (c < 65536) { o[at] = 224 + c / 4096; o[at + 1] = 128 + c / 64 % 64; o[at + 2] = 128 + c % 64; return 3 }
    o[at] = 240 + c / 262144; o[at + 1] = 128 + c / 4096 % 64; o[at + 2] = 128 + c / 64 % 64; o[at + 3] = 128 + c % 64
    return 4
}

fun partB() {
    val badb = intArrayOf(0x80, 0xC0, 0xC1, 0xF5, 0xFF, 0xE0)
    val doc = IntArray(PERDOC * 4)
    var validDocs = 0L; var totalCp = 0L
    for (d in 0 until NDOC) {
        var n = 0
        for (k in 0 until PERDOC) {
            val cat = (nextRand() % 4).toInt(); val r = nextRand()
            val cp = when (cat) { 0 -> r % 128; 1 -> 128 + r % 1920; 2 -> 2048 + r % 53248; else -> 65536 + r % 1048576 }
            n += encode(cp, doc, n)
        }
        if (d % 5 == 2) {
            val p = (nextRand() % n).toInt(); val b = (nextRand() % 6).toInt()
            doc[p] = badb[b]
        }
        val ok = if (utf8Valid(doc, n)) 1L else 0L
        val cnt = if (ok == 1L) utf8Count(doc, n).toLong() else 0L
        fold(ok); fold(cnt); fold(n.toLong())
        validDocs += ok
        totalCp += cnt
    }
    fold(validDocs); fold(totalCp)
}

// ---------- (C) SQL-ish ----------
fun partC() {
    val dept = IntArray(NR); val age = IntArray(NR); val sal = IntArray(NR); val act = IntArray(NR)
    for (i in 0 until NR) {
        dept[i] = (nextRand() % 16).toInt()
        age[i] = (18 + nextRand() % 50).toInt()
        sal[i] = (30000 + nextRand() % 90000).toInt()
        act[i] = if (nextRand() % 10 != 0L) 1 else 0
    }
    repeat(NQ) {
        val ageLo = 18 + nextRand() % 30; val salMax = 40000 + nextRand() % 80000; val need = nextRand() % 2
        val cnt = LongArray(16); val sum = LongArray(16); val mx = LongArray(16); val taken = BooleanArray(16)
        for (i in 0 until NR) {
            if (age[i] >= ageLo && sal[i] < salMax && act[i] >= need) {
                val g = dept[i]
                cnt[g]++
                sum[g] += sal[i]
                if (age[i] > mx[g]) mx[g] = age[i].toLong()
            }
        }
        repeat(3) {
            var best = -1
            for (g in 0 until 16) if (cnt[g] >= 5 && !taken[g] && (best < 0 || sum[g] > sum[best])) best = g
            if (best >= 0) {
                taken[best] = true
                fold(best.toLong()); fold(cnt[best]); fold(sum[best]); fold(mx[best])
                fold(sum[best] / ((best + 1) * 1000))
            } else fold(999)
        }
    }
}

// ---------- (D) HM unification ----------
fun partD() {
    val kind = IntArray(NTOT); val na = IntArray(NTOT) { -1 }; val nb = IntArray(NTOT) { -1 }
    val parent = IntArray(NTOT) { it }
    for (c in 0 until 4) { kind[NVARS + c] = 1; na[NVARS + c] = c }
    for (i in NVARS + 4 until NTOT) { kind[i] = 2; na[i] = (nextRand() % i).toInt(); nb[i] = (nextRand() % i).toInt() }
    var unions = 0L
    fun find(x0: Int): Int {
        var x = x0
        while (parent[x] != x) { parent[x] = parent[parent[x]]; x = parent[x] }
        return x
    }
    val stkA = IntArray(2 * NTOT + 16); val stkB = IntArray(2 * NTOT + 16)
    fun unify(x: Int, y: Int): Boolean {
        var sp = 0
        stkA[sp] = x; stkB[sp] = y; sp++
        while (sp > 0) {
            sp--
            val rx = find(stkA[sp]); val ry = find(stkB[sp])
            if (rx == ry) continue
            val kx = kind[rx]; val ky = kind[ry]
            if (kx == 0) { parent[rx] = ry; unions++ }
            else if (ky == 0) { parent[ry] = rx; unions++ }
            else if (kx == 1 || ky == 1) {
                if (kx == 1 && ky == 1 && na[rx] == na[ry]) { parent[ry] = rx; unions++ } else return false
            } else {
                parent[ry] = rx; unions++
                stkA[sp] = na[rx]; stkB[sp] = na[ry]; sp++
                stkA[sp] = nb[rx]; stkB[sp] = nb[ry]; sp++
            }
        }
        return true
    }
    var fails = 0L
    repeat(NCON) {
        val t = nextRand() % 2
        val x = (if (t == 0L) nextRand() % NVARS else nextRand() % NTOT).toInt()
        val y = (nextRand() % NTOT).toInt()
        if (!unify(x, y)) fails++
    }
    fold(fails); fold(unions)
    var classes = 0L
    for (i in 0 until NTOT) {
        val r = find(i)
        if (r == i) classes++
        fold(r.toLong() * 3 + kind[r])
    }
    fold(classes)
}

fun main() {
    partA(); partB(); partC(); partD()
    println("CHECKSUM=$cs")
}
