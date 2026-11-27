/* 03_astar — 64×64 grid, 40 queries, binary min-heap */
fun main() {
    val s = 64
    val cells = s * s
    val queries = 40
    val inf = 0x3fffffff
    val lcgMod = 2147483648L
    var rng = 10846167L
    fun nextU(): Long {
        rng = (rng * 1103515245L + 12345L) % lcgMod
        return rng
    }
    val dirs = arrayOf(intArrayOf(1, 0), intArrayOf(-1, 0), intArrayOf(0, 1), intArrayOf(0, -1))

    fun heur(a: Int, b: Int): Int {
        val ax = a % s
        val ay = a / s
        val bx = b % s
        val by = b / s
        var dx = ax - bx
        if (dx < 0) dx = -dx
        var dy = ay - by
        if (dy < 0) dy = -dy
        return dx + dy
    }

    class MinHeap {
        val heap = IntArray(cells)
        val hprio = IntArray(cells)
        var hn = 0
        fun push(node: Int, prio: Int) {
            heap[hn] = node
            hprio[hn] = prio
            var i = hn++
            while (i > 0) {
                val p = (i - 1) / 2
                if (hprio[i] >= hprio[p]) break
                val tn = heap[i]
                heap[i] = heap[p]
                heap[p] = tn
                val tp = hprio[i]
                hprio[i] = hprio[p]
                hprio[p] = tp
                i = p
            }
        }
        fun pop(): Int {
            val top = heap[0]
            hn--
            heap[0] = heap[hn]
            hprio[0] = hprio[hn]
            var i = 0
            while (true) {
                val l = 2 * i + 1
                val r = l + 1
                var sm = i
                if (l < hn && hprio[l] < hprio[sm]) sm = l
                if (r < hn && hprio[r] < hprio[sm]) sm = r
                if (sm == i) break
                val tn = heap[i]
                heap[i] = heap[sm]
                heap[sm] = tn
                val tp = hprio[i]
                hprio[i] = hprio[sm]
                hprio[sm] = tp
                i = sm
            }
            return top
        }
    }

    fun astar(walkable: BooleanArray, start: Int, goal: Int): Int {
        val gscore = IntArray(cells) { inf }
        val came = IntArray(cells) { -1 }
        val openf = BooleanArray(cells)
        val heap = MinHeap()
        gscore[start] = 0
        heap.push(start, heur(start, goal))
        openf[start] = true
        while (heap.hn > 0) {
            val cur = heap.pop()
            openf[cur] = false
            if (cur == goal) {
                var len = 0
                var c = goal
                while (c != start && c >= 0) {
                    len++
                    c = came[c]
                }
                return len
            }
            val cx = cur % s
            val cy = cur / s
            for (d in dirs) {
                val nx = cx + d[0]
                val ny = cy + d[1]
                if (nx < 0 || ny < 0 || nx >= s || ny >= s) continue
                val ni = ny * s + nx
                if (!walkable[ni]) continue
                val tent = gscore[cur] + 1
                if (tent < gscore[ni]) {
                    came[ni] = cur
                    gscore[ni] = tent
                    val f = tent + heur(ni, goal)
                    heap.push(ni, f)
                    openf[ni] = true
                }
            }
        }
        return -1
    }

    val walkable = BooleanArray(cells) { true }
    repeat(cells / 5) {
        var cell = (nextU().toULong() % cells.toUInt()).toInt()
        if (cell != 0 && cell != cells - 1) walkable[cell] = false
    }
    var cs = 0L
    repeat(queries) {
        var a = (nextU().toULong() % cells.toUInt()).toInt()
        var b = (nextU().toULong() % cells.toUInt()).toInt()
        while (!walkable[a]) a = (nextU().toULong() % cells.toUInt()).toInt()
        while (!walkable[b]) b = (nextU().toULong() % cells.toUInt()).toInt()
        cs += astar(walkable, a, b)
    }
    println("CHECKSUM=$cs")
}