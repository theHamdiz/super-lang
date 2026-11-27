//! 03_astar — 64×64 grid, 40 queries, binary min-heap (lazy duplicates, as in C)
const S: i32 = 64;
const CELLS: usize = (S * S) as usize;
const QUERIES: usize = 40;
const INF: i32 = 0x3fffffff;
const DIRS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

struct Lcg(u64);
impl Lcg {
    #[inline(always)]
    fn next_u(&mut self) -> u32 {
        self.0 = (self.0.wrapping_mul(1103515245).wrapping_add(12345)) % 2147483648;
        self.0 as u32
    }
    #[inline(always)]
    fn cell(&mut self) -> usize {
        self.next_u() as usize % CELLS
    }
}

struct Search {
    walkable: [bool; CELLS],
    gscore: [i32; CELLS],
    came: [i32; CELLS],
    open: [bool; CELLS],
    heap: [(i32, i32); CELLS], // (prio, node)
    hn: usize,
}

#[inline(always)]
fn heur(a: i32, b: i32) -> i32 {
    ((a % S) - (b % S)).abs() + ((a / S) - (b / S)).abs()
}

impl Search {
    fn push(&mut self, node: i32, prio: i32) {
        let mut i = self.hn;
        self.hn += 1;
        self.heap[i] = (prio, node);
        while i > 0 {
            let p = (i - 1) / 2;
            if self.heap[i].0 >= self.heap[p].0 {
                break;
            }
            self.heap.swap(i, p);
            i = p;
        }
    }

    fn pop(&mut self) -> i32 {
        let top = self.heap[0].1;
        self.hn -= 1;
        self.heap[0] = self.heap[self.hn];
        let mut i = 0;
        loop {
            let l = 2 * i + 1;
            let r = l + 1;
            let mut sm = i;
            if l < self.hn && self.heap[l].0 < self.heap[sm].0 {
                sm = l;
            }
            if r < self.hn && self.heap[r].0 < self.heap[sm].0 {
                sm = r;
            }
            if sm == i {
                break;
            }
            self.heap.swap(i, sm);
            i = sm;
        }
        top
    }

    fn astar(&mut self, start: i32, goal: i32) -> i32 {
        self.gscore.fill(INF);
        self.open.fill(false);
        self.came.fill(-1);
        self.hn = 0;
        self.gscore[start as usize] = 0;
        self.push(start, heur(start, goal));
        self.open[start as usize] = true;
        while self.hn > 0 {
            let cur = self.pop();
            self.open[cur as usize] = false;
            if cur == goal {
                let (mut len, mut c) = (0, goal);
                while c != start && c >= 0 {
                    len += 1;
                    c = self.came[c as usize];
                }
                return len;
            }
            let (cx, cy) = (cur % S, cur / S);
            for (dx, dy) in DIRS {
                let (nx, ny) = (cx + dx, cy + dy);
                if nx < 0 || ny < 0 || nx >= S || ny >= S {
                    continue;
                }
                let ni = ny * S + nx;
                if !self.walkable[ni as usize] {
                    continue;
                }
                let tent = self.gscore[cur as usize] + 1;
                if tent < self.gscore[ni as usize] {
                    self.came[ni as usize] = cur;
                    self.gscore[ni as usize] = tent;
                    let f = tent + heur(ni, goal);
                    self.push(ni, f);
                    self.open[ni as usize] = true;
                }
            }
        }
        -1
    }
}

fn main() {
    let mut rng = Lcg(10846167);
    let mut s = Box::new(Search {
        walkable: [true; CELLS],
        gscore: [0; CELLS],
        came: [0; CELLS],
        open: [false; CELLS],
        heap: [(0, 0); CELLS],
        hn: 0,
    });
    for _ in 0..CELLS / 5 {
        let cell = rng.cell();
        if cell != 0 && cell != CELLS - 1 {
            s.walkable[cell] = false;
        }
    }
    let mut cs: i64 = 0;
    for _ in 0..QUERIES {
        let mut a = rng.cell();
        let mut b = rng.cell();
        while !s.walkable[a] {
            a = rng.cell();
        }
        while !s.walkable[b] {
            b = rng.cell();
        }
        cs += s.astar(a as i32, b as i32) as i64;
    }
    println!("CHECKSUM={cs}");
}
