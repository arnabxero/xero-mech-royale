//! Navigation over the world's floor layer: time-sliced multi-source Dijkstra flow fields
//! (distance-to-goal for every cell, so the mech can follow it from anywhere) and a bounded
//! A* for short local trips.
use crate::world::{World, NB8};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::Instant;

pub const INF: f32 = f32::INFINITY;
pub const NONE: u32 = u32::MAX;

#[inline]
fn key(d: f32, c: u32) -> u64 {
    ((d.to_bits() as u64) << 32) | c as u64
}
#[inline]
fn unkey(k: u64) -> (f32, u32) {
    (f32::from_bits((k >> 32) as u32), (k & 0xffff_ffff) as u32)
}

/// Simulator-only cache of finished flow fields shared by all bots in one process
/// (every bot seeded from the same zone disc computes an identical field).
pub mod shared {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;
    thread_local! {
        pub static ENABLED: RefCell<bool> = const { RefCell::new(false) };
        pub static CACHE: RefCell<HashMap<(i64, i64, i64), Rc<(Vec<f32>, Vec<u32>, f32)>>> = RefCell::new(HashMap::new());
    }
    pub fn enabled() -> bool {
        ENABLED.with(|e| *e.borrow())
    }
    pub fn enable(on: bool) {
        ENABLED.with(|e| *e.borrow_mut() = on);
        CACHE.with(|c| c.borrow_mut().clear());
    }
    pub fn get(tag: (i64, i64, i64)) -> Option<Rc<(Vec<f32>, Vec<u32>, f32)>> {
        CACHE.with(|c| c.borrow().get(&tag).cloned())
    }
    pub fn put(tag: (i64, i64, i64), dist: &[f32], next: &[u32], radius: f32) {
        CACHE.with(|c| {
            let mut c = c.borrow_mut();
            if c.len() > 64 {
                c.clear();
            }
            c.insert(tag, Rc::new((dist.to_vec(), next.to_vec(), radius)));
        })
    }
}

pub struct FlowField {
    pub dist: Vec<f32>,
    pub next: Vec<u32>,
    heap: BinaryHeap<Reverse<u64>>,
    pub done: bool,
    pub pops: usize,
    /// identifies what this field leads to
    pub tag: (i64, i64, i64),
    /// effective goal radius of the last start_disc
    pub radius: f32,
}

impl FlowField {
    pub fn new(n: usize) -> Self {
        FlowField { dist: vec![INF; n], next: vec![NONE; n], heap: BinaryHeap::new(), done: true, pops: 0, tag: (0, 0, -1), radius: 0.0 }
    }

    /// Start a new search. `seed(c) -> Option<cost>` gives the start cost of goal cells.
    pub fn start(&mut self, w: &World, tag: (i64, i64, i64), cells: impl Iterator<Item = (usize, f32)>) {
        self.dist.iter_mut().for_each(|d| *d = INF);
        self.next.iter_mut().for_each(|d| *d = NONE);
        self.heap.clear();
        for (c, cost) in cells {
            if w.standable(c) && cost < self.dist[c] {
                self.dist[c] = cost;
                self.heap.push(Reverse(key(cost, c as u32)));
            }
        }
        self.done = self.heap.is_empty();
        self.pops = 0;
        self.tag = tag;
    }

    /// Seed every standable cell inside a disc at cost 0. If the disc holds fewer than
    /// `min_cells` standable cells (circle mostly on roofs or off the map), grow it until it does.
    /// Returns the effective radius used.
    pub fn start_disc(&mut self, w: &World, tag: (i64, i64, i64), cx: f32, cy: f32, r: f32, min_cells: usize) -> f32 {
        let mut r = r.max(1.0);
        let mut seeds: Vec<(usize, f32)> = Vec::new();
        loop {
            seeds.clear();
            let i0 = ((cx - r - w.min_x).floor().max(0.0)) as usize;
            let i1 = ((cx + r - w.min_x).ceil().max(0.0) as usize).min(w.nx - 1);
            let j0 = ((cy - r - w.min_y).floor().max(0.0)) as usize;
            let j1 = ((cy + r - w.min_y).ceil().max(0.0) as usize).min(w.ny - 1);
            if i0 <= i1 && j0 <= j1 {
                for j in j0..=j1 {
                    for i in i0..=i1 {
                        let c = w.idx(i, j);
                        if !w.standable(c) {
                            continue;
                        }
                        let (x, y) = w.cell_xy(c);
                        if (x - cx).powi(2) + (y - cy).powi(2) <= r * r {
                            seeds.push((c, 0.0));
                        }
                    }
                }
            }
            if seeds.len() >= min_cells || r > 2000.0 {
                break;
            }
            r = r * 1.25 + 2.0;
        }
        self.start(w, tag, seeds.into_iter());
        self.radius = r;
        r
    }

    /// Run the search until done or the deadline passes. Returns true when complete.
    pub fn run(&mut self, w: &World, deadline: Instant) -> bool {
        if self.done {
            return true;
        }
        let nx = w.nx as i32;
        let ny = w.ny as i32;
        let mut iter = 0u32;
        while let Some(Reverse(k)) = self.heap.pop() {
            let (d, u) = unkey(k);
            let u = u as usize;
            if d > self.dist[u] {
                continue;
            }
            self.pops += 1;
            let ui = (u % w.nx) as i32;
            let uj = (u / w.nx) as i32;
            // predecessors p with an edge p -> u
            for &(di, dj, len) in NB8.iter() {
                let pi = ui + di;
                let pj = uj + dj;
                if pi < 0 || pj < 0 || pi >= nx || pj >= ny {
                    continue;
                }
                let p = (pj * nx + pi) as usize;
                if let Some(cost) = w.walk_cost(p, u, len) {
                    let nd = d + cost;
                    if nd < self.dist[p] {
                        self.dist[p] = nd;
                        self.next[p] = u as u32;
                        self.heap.push(Reverse(key(nd, p as u32)));
                    }
                }
            }
            for &(p, cost) in w.jumps_to(u) {
                let p = p as usize;
                let nd = d + cost;
                if nd < self.dist[p] {
                    self.dist[p] = nd;
                    self.next[p] = u as u32;
                    self.heap.push(Reverse(key(nd, p as u32)));
                }
            }
            iter += 1;
            if iter & 1023 == 0 && Instant::now() >= deadline {
                return false;
            }
        }
        self.done = true;
        true
    }

    /// Best cell to start following from near (x,y,z): own cell if reached, else the nearby
    /// reached cell minimizing dist + offset.
    pub fn entry(&self, w: &World, x: f32, y: f32, z: f32, r: i32) -> Option<usize> {
        if let Some(c) = w.cell_of(x, y) {
            if self.dist[c].is_finite() && (w.floor[c] - z).abs() < 3.0 {
                return Some(c);
            }
        }
        let ci = (x - w.min_x).round() as i32;
        let cj = (y - w.min_y).round() as i32;
        let mut best = None;
        let mut bv = INF;
        for dj in -r..=r {
            for di in -r..=r {
                let ii = ci + di;
                let jj = cj + dj;
                if ii < 0 || jj < 0 || ii >= w.nx as i32 || jj >= w.ny as i32 {
                    continue;
                }
                let c = jj as usize * w.nx + ii as usize;
                if !self.dist[c].is_finite() {
                    continue;
                }
                let dz = w.floor[c] - z;
                // can't step up onto something much higher without a climb
                let climb_pen = if dz > w.p.step_up { 6.0 + dz * 1.5 } else { 0.0 };
                let v = self.dist[c] + ((di * di + dj * dj) as f32).sqrt() * 3.0 + climb_pen;
                if v < bv {
                    bv = v;
                    best = Some(c);
                }
            }
        }
        best
    }

    /// Follow `next` from c for up to `steps` cells.
    pub fn chain(&self, c: usize, steps: usize, out: &mut Vec<usize>) {
        out.clear();
        out.push(c);
        let mut cur = c;
        for _ in 0..steps {
            let nx = self.next[cur];
            if nx == NONE {
                break;
            }
            cur = nx as usize;
            out.push(cur);
        }
    }
}

/// Reusable scratch space for A* (dense arrays with generation stamps: no per-call allocation).
pub struct AStar {
    g: Vec<f32>,
    parent: Vec<u32>,
    stamp: Vec<u32>,
    gen: u32,
    heap: BinaryHeap<Reverse<u64>>,
}

impl AStar {
    pub fn new(n: usize) -> Self {
        AStar { g: vec![INF; n], parent: vec![NONE; n], stamp: vec![0; n], gen: 0, heap: BinaryHeap::new() }
    }

    /// Bounded A* from cell s towards (gx,gy); stops within goal_r of it or after max_nodes
    /// expansions (then returns the path to the expanded cell closest to the goal).
    pub fn search(&mut self, w: &World, s: usize, gx: f32, gy: f32, goal_r: f32, max_nodes: usize) -> Option<Vec<usize>> {
        self.gen = self.gen.wrapping_add(1);
        if self.gen == 0 {
            self.stamp.iter_mut().for_each(|x| *x = 0);
            self.gen = 1;
        }
        let gen = self.gen;
        let nx = w.nx as i32;
        let ny = w.ny as i32;
        let h = |c: usize| {
            let (x, y) = w.cell_xy(c);
            ((x - gx).powi(2) + (y - gy).powi(2)).sqrt()
        };
        self.heap.clear();
        self.g[s] = 0.0;
        self.parent[s] = NONE;
        self.stamp[s] = gen;
        self.heap.push(Reverse(key(h(s), s as u32)));
        let mut expanded = 0;
        let mut best = (h(s), s);
        while let Some(Reverse(k)) = self.heap.pop() {
            let (f, u) = unkey(k);
            let u = u as usize;
            let gu = self.g[u];
            let hu = h(u);
            if f > gu + hu + 1e-3 {
                continue; // stale entry
            }
            if hu < best.0 {
                best = (hu, u);
            }
            if hu <= goal_r {
                break;
            }
            expanded += 1;
            if expanded > max_nodes {
                break;
            }
            let ui = (u % w.nx) as i32;
            let uj = (u / w.nx) as i32;
            for &(di, dj, len) in NB8.iter() {
                let vi = ui + di;
                let vj = uj + dj;
                if vi < 0 || vj < 0 || vi >= nx || vj >= ny {
                    continue;
                }
                let v = (vj * nx + vi) as usize;
                if let Some(cost) = w.walk_cost(u, v, len) {
                    let nd = gu + cost;
                    if self.stamp[v] != gen || nd < self.g[v] {
                        self.stamp[v] = gen;
                        self.g[v] = nd;
                        self.parent[v] = u as u32;
                        self.heap.push(Reverse(key(nd + h(v), v as u32)));
                    }
                }
            }
            for &(v, cost) in w.jumps_from(u) {
                let v = v as usize;
                let nd = gu + cost;
                if self.stamp[v] != gen || nd < self.g[v] {
                    self.stamp[v] = gen;
                    self.g[v] = nd;
                    self.parent[v] = u as u32;
                    self.heap.push(Reverse(key(nd + h(v), v as u32)));
                }
            }
        }
        if best.1 == s {
            return None;
        }
        let mut path = vec![best.1];
        let mut cur = best.1;
        while self.parent[cur] != NONE && path.len() < 100_000 {
            cur = self.parent[cur] as usize;
            path.push(cur);
        }
        path.reverse();
        Some(path)
    }
}
