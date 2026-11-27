# 05_parallel_mapreduce — 64 KiB corpus, word + 3-gram frequencies (Julia 1.13)
# Per-worker open-addressing tables built in parallel, merged, then the canonical
# single-scan checksum (identical protocol to algo.c).
# Run: julia -O3 --check-bounds=no -t auto algo.jl

const CORPUS = 65536
const BUCKETS = 65536
const MASK = UInt32(BUCKETS - 1)
const WORDS = ("alpha", "bravo", "charlie", "delta", "echo",
               "foxtrot", "golf", "hotel", "india", "juliet")

struct Table
    keys::Vector{UInt32}
    counts::Vector{Int32}
end
Table() = Table(zeros(UInt32, BUCKETS), zeros(Int32, BUCKETS))

@inline lcg(r::Int) = (r * 1103515245 + 12345) & 0x7fffffff

@inline function hash_bytes(text::Vector{UInt8}, start::Int, n::Int)::UInt32
    h = 0x811c9dc5
    @inbounds for i in start:start+n-1
        h = (h ⊻ text[i]) * 0x01000193
    end
    return h
end

@inline function bump!(t::Table, key::UInt32, add::Int)
    i = key & MASK
    @inbounds for probe in 0:BUCKETS-1
        j = Int((i + probe) & MASK) + 1
        c = t.counts[j]
        if c == 0
            t.keys[j] = key
            t.counts[j] = add % Int32
            return nothing
        elseif t.keys[j] == key
            t.counts[j] = c + add % Int32
            return nothing
        end
    end
    return nothing
end

@inline islower(b::UInt8) = UInt8('a') <= b <= UInt8('z')

# Tokenize text[lo:hi) (1-based, half-open on hi) into words and bump word + 3-gram keys.
function scan!(t::Table, text::Vector{UInt8}, lo::Int, hi::Int)
    i = lo
    @inbounds while i < hi
        while i < hi && !islower(text[i]); i += 1; end
        start = i
        while i < hi && islower(text[i]); i += 1; end
        if i > start
            len = i - start
            bump!(t, hash_bytes(text, start, len), 1)
            len >= 3 && bump!(t, hash_bytes(text, start, 3) ⊻ 0x00c0ffee, 1)
        end
    end
    return nothing
end

function make_corpus()::Vector{UInt8}
    text = fill(UInt8('x'), CORPUS)
    rng = 370945
    off = 0
    while off + 16 < CORPUS
        rng = lcg(rng)
        w = codeunits(WORDS[rng % 10 + 1])
        copyto!(text, off + 1, w, 1, length(w))
        off += length(w)
        text[off+1] = UInt8(' ')
        off += 1
    end
    return text
end

function summarize(t::Table)::Int
    total = 0
    best = 0
    best_key = UInt32(0)
    @inbounds for k in 1:BUCKETS
        c = Int(t.counts[k])
        c == 0 && continue
        total += c
        key = t.keys[k]
        if c > best || (c == best && key < best_key)
            best = c
            best_key = key
        end
    end
    return total ⊻ (best * 1315423911) ⊻ Int(best_key)
end

function compute()::Int
    text = make_corpus()

    # Parallel phase: one table per worker over its byte chunk.
    nworkers = clamp(Threads.nthreads(), 1, 32)
    tables = [Table() for _ in 1:nworkers]
    chunk = CORPUS ÷ nworkers
    Threads.@threads :static for w in 1:nworkers
        lo = (w - 1) * chunk + 1
        hi = w == nworkers ? CORPUS + 1 : lo + chunk
        scan!(tables[w], text, lo, hi)
    end
    merged = Table()
    for t in tables, k in 1:BUCKETS
        @inbounds c = t.counts[k]
        c != 0 && bump!(merged, t.keys[k], Int(c))
    end
    summarize(merged)  # worker-split words may differ at chunk edges (as in C)

    # Canonical result: full sequential scan (chunk-boundary independent).
    full = Table()
    scan!(full, text, 1, CORPUS + 1)
    return summarize(full)
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
