/* 15_physics2d_broadphase_solver - 6k circles/boxes: uniform spatial-hash broadphase,
   circle/box manifolds, sequential-impulse solver (4 iterations x 10 steps).
   Mirrors std.game.physics2d (grid build, candidate order, narrowphase, resolve). */
import kotlin.math.abs
import kotlin.math.floor
import kotlin.math.sqrt

const val COLS = 100
const val ROWS = 60
const val N_DYN = COLS * ROWS
const val FLOOR_TILES = 62
const val WALL_TILES = 40
const val STEPS = 10
const val ITERS = 4
const val MOD = 1000000007L

var rng = 150015L
fun draw(m: Long): Long {
    rng = (rng * 1103515245L + 12345L) % 2147483648L
    return rng / 256 % m
}

enum class Shape { CIRCLE, BOX }

class Body(
    var x: Double, var y: Double, var vx: Double, var vy: Double, val invMass: Double,
    val shape: Shape, val r: Double, val hw: Double, val hh: Double,
    val e: Double = 0.2, val mu: Double = 0.4,
)

fun inv(mass: Double) = if (mass <= 0.0) 0.0 else 1.0 / mass
fun circle(x: Double, y: Double, r: Double, mass: Double, vx: Double, vy: Double) =
    Body(x, y, vx, vy, inv(mass), Shape.CIRCLE, r, r, r)
fun aabb(x: Double, y: Double, hw: Double, hh: Double, mass: Double, vx: Double, vy: Double) =
    Body(x, y, vx, vy, inv(mass), Shape.BOX, 0.0, hw, hh)

fun sign(v: Double) = if (v < 0.0) -1.0 else 1.0
fun clamp(v: Double, lo: Double, hi: Double) = if (v < lo) lo else if (v > hi) hi else v
fun pmax(a: Double, b: Double) = if (a > b) a else b
fun pmin(a: Double, b: Double) = if (a < b) a else b

// narrowphase result lands in these (hit flag returned)
var cnx = 0.0
var cny = 0.0
var cdepth = 0.0

fun circleCircle(a: Body, b: Body): Boolean {
    val dx = b.x - a.x
    val dy = b.y - a.y
    val rr = a.r + b.r
    val d2 = dx * dx + dy * dy
    if (d2 >= rr * rr) return false
    if (d2 == 0.0) { cnx = 1.0; cny = 0.0; cdepth = rr; return true }
    val d = sqrt(d2)
    cnx = dx / d; cny = dy / d; cdepth = rr - d
    return true
}

fun boxBox(a: Body, b: Body): Boolean {
    val dx = b.x - a.x
    val dy = b.y - a.y
    val ox = a.hw + b.hw - abs(dx)
    val oy = a.hh + b.hh - abs(dy)
    if (ox <= 0.0 || oy <= 0.0) return false
    if (ox < oy) { cnx = sign(dx); cny = 0.0; cdepth = ox; return true }
    cnx = 0.0; cny = sign(dy); cdepth = oy
    return true
}

fun circleBox(a: Body, b: Body): Boolean {
    val lx = a.x - b.x
    val ly = a.y - b.y
    val cx = clamp(lx, 0.0 - b.hw, b.hw)
    val cy = clamp(ly, 0.0 - b.hh, b.hh)
    val fx = lx - cx
    val fy = ly - cy
    val d2 = fx * fx + fy * fy
    if (d2 > 0.0) {
        if (d2 >= a.r * a.r) return false
        val d = sqrt(d2)
        cnx = (0.0 - fx) / d; cny = (0.0 - fy) / d; cdepth = a.r - d
        return true
    }
    val px = b.hw - abs(lx)
    val py = b.hh - abs(ly)
    if (px < py) { cnx = 0.0 - sign(lx); cny = 0.0; cdepth = px + a.r; return true }
    cnx = 0.0; cny = 0.0 - sign(ly); cdepth = py + a.r
    return true
}

fun collide(a: Body, b: Body): Boolean = when {
    a.shape == Shape.CIRCLE && b.shape == Shape.CIRCLE -> circleCircle(a, b)
    a.shape == Shape.BOX && b.shape == Shape.BOX -> boxBox(a, b)
    a.shape == Shape.CIRCLE -> circleBox(a, b)
    else -> if (circleBox(b, a)) { cnx = 0.0 - cnx; cny = 0.0 - cny; true } else false
}

fun resolve(a: Body, b: Body, nx: Double, ny: Double, depth: Double) {
    val ia = a.invMass
    val ib = b.invMass
    val isum = ia + ib
    if (isum == 0.0) return
    val rvx = b.vx - a.vx
    val rvy = b.vy - a.vy
    val vn = rvx * nx + rvy * ny
    if (vn < 0.0) {
        val e = pmin(a.e, b.e)
        val j = (0.0 - (1.0 + e)) * vn / isum
        a.vx = a.vx - j * ia * nx
        a.vy = a.vy - j * ia * ny
        b.vx = b.vx + j * ib * nx
        b.vy = b.vy + j * ib * ny
        var tx = rvx - vn * nx
        var ty = rvy - vn * ny
        val tl = sqrt(tx * tx + ty * ty)
        if (tl > 0.000001) {
            tx /= tl
            ty /= tl
            var jt = (0.0 - (rvx * tx + rvy * ty)) / isum
            val mu = sqrt(a.mu * b.mu)
            val lim = mu * j
            jt = clamp(jt, 0.0 - lim, lim)
            a.vx = a.vx - jt * ia * tx
            a.vy = a.vy - jt * ia * ty
            b.vx = b.vx + jt * ib * tx
            b.vy = b.vy + jt * ib * ty
        }
    }
    val corr = pmax(depth - 0.01, 0.0) / isum * 0.8
    a.x = a.x - corr * ia * nx
    a.y = a.y - corr * ia * ny
    b.x = b.x + corr * ib * nx
    b.y = b.y + corr * ib * ny
}

// ---- broadphase: uniform spatial hash (per-bucket entry chains) ----
fun bucket(cx: Long, cy: Long, mask: Long): Int = (((cx * 73856093L) xor (cy * 19349663L)) and mask).toInt()

class Grid {
    var heads = IntArray(0)
    var eb = IntArray(1 shl 16)
    var ex = LongArray(1 shl 16)
    var ey = LongArray(1 shl 16)
    var en = IntArray(1 shl 16)
    var len = 0
    var inv = 1.0
    var mask = 15L

    fun push(head: Int, i: Int, cx: Long, cy: Long) {
        if (len == eb.size) {
            eb = eb.copyOf(len * 2); ex = ex.copyOf(len * 2); ey = ey.copyOf(len * 2); en = en.copyOf(len * 2)
        }
        en[len] = head; eb[len] = i; ex[len] = cx; ey[len] = cy
        len++
    }

    fun build(bodies: Array<Body>) {
        val n = bodies.size
        var sum = 0.0
        var big = 0.0
        for (b in bodies) {
            val e = pmax(b.hw, b.hh)
            sum += e
            if (e > big) big = e
        }
        var cell = pmax(2.0 * sum / n.toDouble(), big / 8.0)
        if (cell < 0.000001) cell = 1.0
        inv = 1.0 / cell
        val pad = cell * 0.1
        var size = 16
        while (size < n * 2) size *= 2
        mask = (size - 1).toLong()
        if (heads.size != size) heads = IntArray(size)
        heads.fill(-1)
        len = 0
        for (i in 0 until n) {
            val b = bodies[i]
            val cx0 = floor((b.x - b.hw - pad) * inv).toLong()
            val cx1 = floor((b.x + b.hw + pad) * inv).toLong()
            val cy0 = floor((b.y - b.hh - pad) * inv).toLong()
            val cy1 = floor((b.y + b.hh + pad) * inv).toLong()
            for (cy in cy0 until cy1 + 1) for (cx in cx0 until cx1 + 1) {
                val slot = bucket(cx, cy, mask)
                push(heads[slot], i, cx, cy)
                heads[slot] = len - 1
            }
        }
    }

    // bodies j > i whose registered cells overlap body i's padded AABB: ascending, unique
    fun candidates(b: Body, i: Int, mark: IntArray, out: IntArray): Int {
        val pad = 0.1 / inv
        val cx0 = floor((b.x - b.hw - pad) * inv).toLong()
        val cx1 = floor((b.x + b.hw + pad) * inv).toLong()
        val cy0 = floor((b.y - b.hh - pad) * inv).toLong()
        val cy1 = floor((b.y + b.hh + pad) * inv).toLong()
        var cnt = 0
        for (cy in cy0 until cy1 + 1) for (cx in cx0 until cx1 + 1) {
            var e = heads[bucket(cx, cy, mask)]
            while (e >= 0) {
                if (ex[e] == cx && ey[e] == cy) {
                    val j = eb[e]
                    if (j > i && mark[j] != i + 1) {
                        mark[j] = i + 1
                        out[cnt++] = j
                    }
                }
                e = en[e]
            }
        }
        java.util.Arrays.sort(out, 0, cnt)
        return cnt
    }
}

fun solvePair(bodies: Array<Body>, i: Int, j: Int): Int {
    val a = bodies[i]
    val b = bodies[j]
    if (a.invMass + b.invMass > 0.0 && !(abs(b.x - a.x) >= a.hw + b.hw) && !(abs(b.y - a.y) >= a.hh + b.hh)) {
        if (collide(a, b)) {
            resolve(a, b, cnx, cny, cdepth)
            return 1
        }
    }
    return 0
}

fun solveGrid(bodies: Array<Body>, g: Grid, mark: IntArray, cands: IntArray): Int {
    g.build(bodies)
    mark.fill(0)
    var last = 0
    for (i in bodies.indices) {
        val cnt = g.candidates(bodies[i], i, mark, cands)
        for (k in 0 until cnt) last += solvePair(bodies, i, cands[k])
    }
    return last
}

fun posmod(v: Long) = ((v % MOD) + MOD) % MOD
fun fl(v: Double) = floor(v).toLong()

fun main() {
    val list = ArrayList<Body>()
    for (k in 0 until FLOOR_TILES) list.add(aabb(1.0 + 2.0 * k, -1.0, 1.0, 1.0, 0.0, 0.0, 0.0))
    for (k in 0 until WALL_TILES) list.add(aabb(-1.0, 1.0 + 2.0 * k, 1.0, 1.0, 0.0, 0.0, 0.0))
    for (k in 0 until WALL_TILES) list.add(aabb(122.0, 1.0 + 2.0 * k, 1.0, 1.0, 0.0, 0.0, 0.0))
    for (i in 0 until N_DYN) {
        val col = (i % COLS).toDouble()
        val row = (i / COLS).toDouble()
        val kind = draw(3)
        val sw = 0.35 + draw(400001).toDouble() * 0.000001
        val sh = 0.35 + draw(400001).toDouble() * 0.000001
        val mass = 1.0 + draw(4).toDouble()
        val jx = (draw(600001) - 300000).toDouble() * 0.000001
        val jy = (draw(600001) - 300000).toDouble() * 0.000001
        val vx = (draw(8000001) - 4000000).toDouble() * 0.000001
        val vy = (draw(8000001) - 4000000).toDouble() * 0.000001
        val x = 0.75 + col * 1.2 + jx
        val y = 1.75 + row * 1.2 + jy
        list.add(if (kind == 0L) aabb(x, y, sw, sh, mass, vx, vy) else circle(x, y, sw, mass, vx, vy))
    }
    val bodies = list.toTypedArray()
    val n = bodies.size
    val g = Grid()
    val mark = IntArray(n)
    val cands = IntArray(n)
    var cs = 0L
    val dt = 1.0 / 60.0
    val damp = 1.0 / (1.0 + dt * 0.0)
    for (s in 0 until STEPS) {
        for (b in bodies) {
            if (b.invMass > 0.0) {
                b.vx = (b.vx + 0.0 * dt) * damp
                b.vy = (b.vy + -9.81 * dt) * damp
                b.x = b.x + b.vx * dt
                b.y = b.y + b.vy * dt
            }
        }
        var last = 0
        repeat(ITERS) { last = solveGrid(bodies, g, mark, cands) }
        cs += last.toLong() * (s + 1)
    }
    var h = 0L
    var ke = 0.0
    for (b in bodies) {
        h = (h * 131 + posmod(fl(b.x * 1000.0))) % MOD
        h = (h * 131 + posmod(fl(b.y * 1000.0))) % MOD
        h = (h * 131 + posmod(fl(b.vx * 1000.0))) % MOD
        h = (h * 131 + posmod(fl(b.vy * 1000.0))) % MOD
        if (b.invMass > 0.0) ke += 0.5 * (b.vx * b.vx + b.vy * b.vy) / b.invMass
    }
    cs += h + fl(ke * 10.0)
    println("CHECKSUM=$cs")
}
