//! 05_parallel_mapreduce — word + 3-gram frequencies over a generated corpus.
//! Worker threads (std::thread) build per-thread tables; the final checksum is the
//! single-pass rescan, exactly as in algo.c (chunk boundaries may split words).
use std::thread;

const CORPUS: usize = 65536;
const BUCKETS: usize = 65536;
const WORDS: [&str; 10] = [
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet",
];

#[derive(Clone, Copy, Default)]
struct Pair {
    key: u32,
    count: i32,
}

#[inline(always)]
fn hash_bytes(s: &[u8]) -> u32 {
    s.iter()
        .fold(2166136261u32, |h, &b| (h ^ b as u32).wrapping_mul(16777619))
}

#[inline]
fn bump(tab: &mut [Pair], key: u32, add: i32) {
    let i = key as usize & (BUCKETS - 1);
    for probe in 0..BUCKETS {
        let slot = &mut tab[(i + probe) & (BUCKETS - 1)];
        if slot.count == 0 {
            *slot = Pair { key, count: add };
            return;
        }
        if slot.key == key {
            slot.count += add;
            return;
        }
    }
}

fn scan(text: &[u8], tab: &mut [Pair]) {
    let end = text.len();
    let mut i = 0;
    while i < end {
        while i < end && !text[i].is_ascii_lowercase() {
            i += 1;
        }
        let start = i;
        while i < end && text[i].is_ascii_lowercase() {
            i += 1;
        }
        if i > start {
            let w = &text[start..i];
            bump(tab, hash_bytes(w), 1);
            if w.len() >= 3 {
                bump(tab, hash_bytes(&w[..3]) ^ 0xC0FFEE, 1);
            }
        }
    }
}

fn summarize(tab: &[Pair]) -> (i64, i32, u32) {
    let (mut total, mut best, mut best_key) = (0i64, 0i32, 0u32);
    for p in tab.iter().filter(|p| p.count != 0) {
        total += p.count as i64;
        if p.count > best || (p.count == best && p.key < best_key) {
            best = p.count;
            best_key = p.key;
        }
    }
    (total, best, best_key)
}

fn main() {
    let nworkers = thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 32);

    let mut text = Vec::with_capacity(CORPUS);
    let mut rng: u64 = 370945;
    while text.len() + 16 < CORPUS {
        rng = (rng.wrapping_mul(1103515245).wrapping_add(12345)) % 2147483648;
        text.extend_from_slice(WORDS[(rng % 10) as usize].as_bytes());
        text.push(b' ');
    }
    text.resize(CORPUS, b'x');

    // Parallel phase: per-worker tables, merged afterwards.
    let chunk = CORPUS / nworkers;
    let mut merged = vec![Pair::default(); BUCKETS];
    let tables: Vec<Vec<Pair>> = thread::scope(|s| {
        let text = &text[..];
        let hs: Vec<_> = (0..nworkers)
            .map(|id| {
                let begin = id * chunk;
                let end = if id == nworkers - 1 { CORPUS } else { begin + chunk };
                s.spawn(move || {
                    let mut tab = vec![Pair::default(); BUCKETS];
                    scan(&text[begin..end], &mut tab);
                    tab
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for tab in &tables {
        for p in tab.iter().filter(|p| p.count != 0) {
            bump(&mut merged, p.key, p.count);
        }
    }
    let _ = summarize(&merged);

    // Checksum pass: whole-corpus rescan (single-thread equivalent).
    merged.fill(Pair::default());
    scan(&text, &mut merged);
    let (total, best, best_key) = summarize(&merged);
    let cs = total ^ ((best as i64) * 1315423911) ^ best_key as i64;
    println!("CHECKSUM={cs}");
}
