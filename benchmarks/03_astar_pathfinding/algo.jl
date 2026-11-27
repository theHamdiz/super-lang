# 03_astar_pathfinding — 64×64 grid, 40 queries, binary min-heap (Julia 1.13)
# Run: julia -O3 --check-bounds=no algo.jl

const S = 64
const CELLS = S * S
const QUERIES = 40
const INF = 0x3fffffff
const DX = (1, -1, 0, 0)
const DY = (0, 0, 1, -1)

@inline lcg(r::Int) = (r * 1103515245 + 12345) & 0x7fffffff

mutable struct Heap
    node::Vector{Int32}
    prio::Vector{Int32}
    n::Int
end
Heap(cap::Int) = Heap(Vector{Int32}(undef, cap), Vector{Int32}(undef, cap), 0)

@inline function heap_push!(h::Heap, node::Int, prio::Int)
    nodes, prios = h.node, h.prio
    h.n += 1
    i = h.n
    @inbounds begin
        nodes[i] = node % Int32
        prios[i] = prio % Int32
        while i > 1
            p = i >> 1
            prios[i] >= prios[p] && break
            nodes[i], nodes[p] = nodes[p], nodes[i]
            prios[i], prios[p] = prios[p], prios[i]
            i = p
        end
    end
    return nothing
end

@inline function heap_pop!(h::Heap)::Int
    nodes, prios = h.node, h.prio
    @inbounds begin
        top = Int(nodes[1])
        n = h.n
        nodes[1] = nodes[n]
        prios[1] = prios[n]
        n -= 1
        h.n = n
        i = 1
        while true
            l = 2i
            r = l + 1
            sm = i
            l <= n && prios[l] < prios[sm] && (sm = l)
            r <= n && prios[r] < prios[sm] && (sm = r)
            sm == i && break
            nodes[i], nodes[sm] = nodes[sm], nodes[i]
            prios[i], prios[sm] = prios[sm], prios[i]
            i = sm
        end
    end
    return top
end

@inline function heur(a::Int, b::Int)::Int
    abs(a % S - b % S) + abs(a ÷ S - b ÷ S)
end

# Nodes are 0-based cell ids (as in the C/Super versions); arrays are indexed id+1.
function astar!(gscore::Vector{Int32}, came::Vector{Int32}, heap::Heap,
                walkable::Vector{Bool}, start::Int, goal::Int)::Int
    fill!(gscore, Int32(INF))
    fill!(came, Int32(-1))
    heap.n = 0
    @inbounds gscore[start+1] = 0
    heap_push!(heap, start, heur(start, goal))
    @inbounds while heap.n > 0
        cur = heap_pop!(heap)
        if cur == goal
            len = 0
            c = goal
            while c != start && c >= 0
                len += 1
                c = Int(came[c+1])
            end
            return len
        end
        cx = cur % S
        cy = cur ÷ S
        for d in 1:4
            nx = cx + DX[d]
            ny = cy + DY[d]
            (nx < 0 || ny < 0 || nx >= S || ny >= S) && continue
            ni = ny * S + nx
            walkable[ni+1] || continue
            tent = Int(gscore[cur+1]) + 1
            if tent < gscore[ni+1]
                came[ni+1] = cur % Int32
                gscore[ni+1] = tent % Int32
                heap_push!(heap, ni, tent + heur(ni, goal))
            end
        end
    end
    return -1
end

function compute()::Int
    walkable = fill(true, CELLS)
    gscore = Vector{Int32}(undef, CELLS)
    came = Vector{Int32}(undef, CELLS)
    heap = Heap(4 * CELLS + 8)   # every push is a strict g-improvement: <= 4 per cell
    rng = 10846167
    for _ in 1:(CELLS ÷ 5)
        rng = lcg(rng)
        cell = rng % CELLS
        (cell != 0 && cell != CELLS - 1) && (walkable[cell+1] = false)
    end
    cs = 0
    for _ in 1:QUERIES
        rng = lcg(rng); a = rng % CELLS
        rng = lcg(rng); b = rng % CELLS
        while !walkable[a+1]
            rng = lcg(rng); a = rng % CELLS
        end
        while !walkable[b+1]
            rng = lcg(rng); b = rng % CELLS
        end
        cs += astar!(gscore, came, heap, walkable, a, b)
    end
    return cs
end

function main()
    # In-process wall time (excludes Julia process startup / sysimage load).
    t0 = time_ns()
    cs = compute()
    elapsed = (time_ns() - t0) / 1e9
    println("CHECKSUM=", cs)
    println("ELAPSED_S=", elapsed)
end

if abspath(PROGRAM_FILE) == @__FILE__
    # Default 2 warmups so timed ELAPSED_S reflects steady-state, not first-compile.
    warm = parse(Int, get(ENV, "JULIA_BENCH_WARM", "2"))
    for _ in 1:warm
        compute()
    end
    main()
end
