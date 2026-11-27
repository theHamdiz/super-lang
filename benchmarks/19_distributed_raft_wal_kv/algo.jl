# 19_distributed_raft_wal_kv — single-process Raft log replication (leader + 2 followers):
# CRC32-protected WAL records, lossy/corrupting virtual links, majority commit, KV state machine. (Julia 1.13)
# Run: julia -O3 --check-bounds=no algo.jl

const MOD = 1_000_000_007
const TICKS, DRAIN, REC, BATCH, KEYS = 6000, 200, 20, 4, 512
const DROP_PCT, ACK_DROP_PCT, CORRUPT_PCT = 35, 15, 3
const CAP = TICKS * REC

const CRC = let t = zeros(UInt32, 256)
    for i in 0:255
        c = UInt32(i)
        for _ in 1:8
            c = (c & 1) == 1 ? (c >> 1) ⊻ 0xEDB88320 : c >> 1
        end
        t[i+1] = c
    end
    t
end

# `o` is a 0-based byte offset into `d`.
function crc32(d::Vector{UInt8}, o::Int, n::Int)
    c = 0xFFFFFFFF
    @inbounds for i in (o+1):(o+n)
        c = CRC[Int((c ⊻ d[i]) & 0xFF)+1] ⊻ (c >> 8)
    end
    return Int(c ⊻ 0xFFFFFFFF)
end

function put32!(p::Vector{UInt8}, o::Int, v::Int)
    p[o+1] = UInt8((v >> 24) & 255)
    p[o+2] = UInt8((v >> 16) & 255)
    p[o+3] = UInt8((v >> 8) & 255)
    p[o+4] = UInt8(v & 255)
end
get32(p::Vector{UInt8}, o::Int) =
    (Int(p[o+1]) << 24) | (Int(p[o+2]) << 16) | (Int(p[o+3]) << 8) | Int(p[o+4])

mutable struct Node
    wal::Vector{UInt8}
    len::Int
    applied::Int
    kv::Vector{Int}
end
Node() = Node(zeros(UInt8, CAP), 0, 0, zeros(Int, KEYS))

function apply_upto!(n::Node, upto::Int)
    while n.applied < upto
        o = n.applied * REC
        if crc32(n.wal, o, 16) == get32(n.wal, o + 16)
            n.kv[get32(n.wal, o + 8)%KEYS+1] = get32(n.wal, o + 12)
        end
        n.applied += 1
    end
end

function compute()
    rng = 19019
    next_rand() = (rng = rng * 48271 % 2147483647)
    nodes = [Node(), Node(), Node()]   # index 1 = leader
    nxt = [1, 1, 1]
    mtch = [0, 0, 0]
    commit = sent = dropped = crc_rejects = acks_lost = 0
    msg = zeros(UInt8, BATCH * REC)
    for t in 0:(TICKS+DRAIN-1)
        if t < TICKS
            l = nodes[1]
            o = l.len * REC
            put32!(l.wal, o, l.len + 1)
            put32!(l.wal, o + 4, 1)
            put32!(l.wal, o + 8, next_rand() % KEYS)
            put32!(l.wal, o + 12, next_rand() % 1000000)
            put32!(l.wal, o + 16, crc32(l.wal, o, 16))
            l.len += 1
        end
        leader_commit = commit
        for f in 2:3
            n0 = nxt[f]
            n0 > nodes[1].len && continue
            cnt = min(nodes[1].len - n0 + 1, BATCH)
            r1 = next_rand() % 100
            r2 = next_rand() % 100
            r3 = next_rand() % 100
            sent += 1
            if r1 < DROP_PCT
                dropped += 1
                continue
            end
            copyto!(msg, 1, nodes[1].wal, (n0 - 1) * REC + 1, cnt * REC)
            if r2 < CORRUPT_PCT
                pos = next_rand() % (cnt * REC)
                msg[pos+1] ⊻= 0x5A
            end
            fol = nodes[f]
            for j in 0:(cnt-1)
                o = j * REC
                if crc32(msg, o, 16) != get32(msg, o + 16)
                    crc_rejects += 1
                    break
                end
                idx = get32(msg, o)
                if idx == fol.len + 1
                    copyto!(fol.wal, fol.len * REC + 1, msg, o + 1, REC)
                    fol.len += 1
                elseif idx > fol.len + 1
                    break
                end
            end
            apply_upto!(fol, min(leader_commit, fol.len))
            if r3 < ACK_DROP_PCT
                acks_lost += 1
                continue
            end
            mtch[f] = max(mtch[f], fol.len)
            nxt[f] = fol.len + 1
        end
        a, b, c = nodes[1].len, mtch[2], mtch[3]
        a > b && ((a, b) = (b, a))
        b > c && ((b, c) = (c, b))
        a > b && ((a, b) = (b, a))
        commit = max(commit, b)
        apply_upto!(nodes[1], commit)
    end
    cs = 0
    fold(v) = (cs = (cs * 131 + v) % MOD)
    for n in nodes
        fold(n.len)
    end
    fold(commit)
    for n in nodes
        fold(n.applied)
    end
    for v in (sent, dropped, crc_rejects, acks_lost)
        fold(v)
    end
    for n in nodes
        d = 0
        for v in n.kv
            d = (d * 131 + v) % MOD
        end
        fold(d)
    end
    for n in nodes[1:2]
        fold(crc32(n.wal, 0, n.len * REC))
    end
    return cs
end

function main()
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
