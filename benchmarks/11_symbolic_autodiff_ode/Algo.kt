/* 11_symbolic_autodiff_ode — 20k random expressions: simplify + forward dual AD,
   then dual-RK4 sensitivities of a stiff-ish ODE */
import kotlin.math.abs
import kotlin.math.cos
import kotlin.math.floor
import kotlin.math.sin

const val OP_CONST = 0
const val OP_VAR = 1
const val OP_ADD = 2
const val OP_SUB = 3
const val OP_MUL = 4
const val OP_NEG = 5
const val OP_SIN = 6
const val EXPRS = 20000
const val POINTS = 7
const val TRAJ = 1024
const val STEPS = 1000
const val MAXN = 64
val LEAF = doubleArrayOf(0.0, 1.0, 2.0, 0.5, -1.0)

var rng = 20240611L
fun lcg(): Long {
    rng = (rng * 1103515245L + 12345L) % 2147483648L
    return rng
}

class Tree {
    val op = IntArray(MAXN)
    val a = IntArray(MAXN)
    val b = IntArray(MAXN)
    val cv = DoubleArray(MAXN)
    var cnt = 0
}

fun gen(t: Tree, depth: Int): Int {
    val k = (lcg() / 256 % 10).toInt()
    if (k < 2 || depth == 0) {
        val m = (lcg() / 256 % 8).toInt()
        val n = t.cnt++
        if (m < 3) { t.op[n] = OP_VAR; t.a[n] = 0; t.b[n] = 0; t.cv[n] = 0.0 }
        else { t.op[n] = OP_CONST; t.a[n] = 0; t.b[n] = 0; t.cv[n] = LEAF[m - 3] }
        return n
    }
    val a = gen(t, depth - 1)
    var kind = OP_SIN
    if (k == 8) kind = OP_NEG
    if (k >= 5 && k < 8) kind = OP_MUL
    if (k == 4) kind = OP_SUB
    if (k < 4) kind = OP_ADD
    var b = 0
    if (kind == OP_ADD || kind == OP_SUB || kind == OP_MUL) b = gen(t, depth - 1)
    val n = t.cnt++
    t.op[n] = kind; t.a[n] = a; t.b[n] = b; t.cv[n] = 0.0
    return n
}

fun isConst(s: Tree, i: Int, c: Double) = s.op[i] == OP_CONST && s.cv[i] == c

fun push(s: Tree, kind: Int, a: Int, b: Int, c: Double): Int {
    val n = s.cnt++
    s.op[n] = kind; s.a[n] = a; s.b[n] = b; s.cv[n] = c
    return n
}

fun simpNode(s: Tree, kind: Int, cv: Double, l: Int, r: Int): Int {
    when (kind) {
        OP_CONST -> return push(s, OP_CONST, 0, 0, cv)
        OP_VAR -> return push(s, OP_VAR, 0, 0, 0.0)
        OP_ADD -> {
            if (isConst(s, l, 0.0)) return r
            if (isConst(s, r, 0.0)) return l
            if (s.op[l] == OP_CONST && s.op[r] == OP_CONST) return push(s, OP_CONST, 0, 0, s.cv[l] + s.cv[r])
            return push(s, OP_ADD, l, r, 0.0)
        }
        OP_SUB -> {
            if (isConst(s, r, 0.0)) return l
            if (s.op[l] == OP_CONST && s.op[r] == OP_CONST) return push(s, OP_CONST, 0, 0, s.cv[l] - s.cv[r])
            if (l == r) return push(s, OP_CONST, 0, 0, 0.0)
            return push(s, OP_SUB, l, r, 0.0)
        }
        OP_MUL -> {
            if (isConst(s, l, 0.0) || isConst(s, r, 0.0)) return push(s, OP_CONST, 0, 0, 0.0)
            if (isConst(s, l, 1.0)) return r
            if (isConst(s, r, 1.0)) return l
            if (s.op[l] == OP_CONST && s.op[r] == OP_CONST) return push(s, OP_CONST, 0, 0, s.cv[l] * s.cv[r])
            return push(s, OP_MUL, l, r, 0.0)
        }
        OP_NEG -> {
            if (s.op[l] == OP_CONST) return push(s, OP_CONST, 0, 0, -s.cv[l])
            if (s.op[l] == OP_NEG) return s.a[l]
            return push(s, OP_NEG, l, 0, 0.0)
        }
        else -> {
            if (isConst(s, l, 0.0)) return push(s, OP_CONST, 0, 0, 0.0)
            return push(s, OP_SIN, l, 0, 0.0)
        }
    }
}

fun simplify(t: Tree, s: Tree, rep: IntArray) {
    s.cnt = 0
    for (i in 0 until t.cnt) rep[i] = simpNode(s, t.op[i], t.cv[i], rep[t.a[i]], rep[t.b[i]])
}

fun evalDual(t: Tree, x: Double, ev: DoubleArray, ed: DoubleArray) {
    for (i in 0 until t.cnt) {
        val ia = t.a[i]
        val ib = t.b[i]
        val rv: Double
        val rd: Double
        when (t.op[i]) {
            OP_CONST -> { rv = t.cv[i]; rd = 0.0 }
            OP_VAR -> { rv = x; rd = 1.0 }
            OP_ADD -> { rv = ev[ia] + ev[ib]; rd = ed[ia] + ed[ib] }
            OP_SUB -> { rv = ev[ia] - ev[ib]; rd = ed[ia] - ed[ib] }
            OP_MUL -> { rv = ev[ia] * ev[ib]; rd = ev[ia] * ed[ib] + ed[ia] * ev[ib] }
            OP_NEG -> { rv = -ev[ia]; rd = -ed[ia] }
            else -> { rv = sin(ev[ia]); rd = cos(ev[ia]) * ed[ia] }
        }
        ev[i] = rv; ed[i] = rd
    }
}

var rv = 0.0
var rd = 0.0
fun rhs(lam: Double, t: Double, v: Double, d: Double) {
    val s = sin(t)
    val c = cos(t)
    val dv = v - s
    val pv = lam * dv
    val pd = lam * d + 1.0 * dv
    rv = -pv + 0.5 * c
    rd = -pd
}

fun rk4Final(lam: Double, y0: Double, h: Double, steps: Int, out: DoubleArray) {
    var yv = y0
    var yd = 0.0
    val hh = 0.5 * h
    val h6 = h / 6.0
    for (k in 0 until steps) {
        val t = k.toDouble() * h
        rhs(lam, t, yv, yd); val k1v = rv; val k1d = rd
        rhs(lam, t + hh, yv + k1v * hh, yd + k1d * hh); val k2v = rv; val k2d = rd
        rhs(lam, t + hh, yv + k2v * hh, yd + k2d * hh); val k3v = rv; val k3d = rd
        rhs(lam, t + h, yv + k3v * h, yd + k3d * h); val k4v = rv; val k4d = rd
        yv = yv + (((k1v + k2v * 2.0) + k3v * 2.0) + k4v) * h6
        yd = yd + (((k1d + k2d * 2.0) + k3d * 2.0) + k4d) * h6
    }
    out[0] = yv
    out[1] = yd
}

fun main() {
    val t = Tree()
    val s = Tree()
    val rep = IntArray(MAXN)
    val ev = DoubleArray(MAXN)
    val ed = DoubleArray(MAXN)
    val sev = DoubleArray(MAXN)
    val sed = DoubleArray(MAXN)
    var origNodes = 0L
    var simpNodes = 0L
    var mismatch = 0L
    var accV = 0.0
    var accD = 0.0
    for (e in 0 until EXPRS) {
        t.cnt = 0
        gen(t, 4)
        val n = t.cnt
        simplify(t, s, rep)
        val m = s.cnt
        origNodes += n
        simpNodes += m
        for (p in 0 until POINTS) {
            val x = (p - 3) / 4.0
            evalDual(t, x, ev, ed)
            evalDual(s, x, sev, sed)
            val vo = ev[n - 1]
            val dro = ed[n - 1]
            val vs = sev[rep[n - 1]]
            val ds = sed[rep[n - 1]]
            if (abs(vo - vs) > 0.000000001 * (1.0 + abs(vo))) mismatch++
            if (abs(dro - ds) > 0.000000001 * (1.0 + abs(dro))) mismatch++
            accV += vs
            accD += ds
        }
    }

    var sumY = 0.0
    var sumDy = 0.0
    val out = DoubleArray(2)
    for (j in 0 until TRAJ) {
        rk4Final(10.0 + j * 0.25, 1.0, 0.002, STEPS, out)
        sumY += out[0]
        sumDy += out[1]
    }

    val q = floor(accV * 10000.0).toLong()
    val qd = floor(accD * 10000.0).toLong()
    val y1 = floor(sumY * 1000000.0).toLong()
    val y2 = floor(sumDy * 1000000000.0).toLong()
    val cs = origNodes + simpNodes * 7 + mismatch * 13 + q * 3 + qd * 5 + y1 * 11 + y2 * 17
    println("CHECKSUM=$cs")
}
