//! Static world model built from the arena files: terrain, prisms rasterized onto a 1 m grid
//! (aligned with the terrain vertices), the walkable "floor" layer, wall clearance for a mech
//! cylinder, jump/drop edges (jet climbs onto roofs, drops off them), and line-of-sight tests.
use serde_json::Value;
use std::fs;
use std::path::Path;

pub const STAND: u8 = 1; // a mech can stand here (floor + clearance)
pub const PRISM_FLOOR: u8 = 2; // floor is a prism top (roof / deck), not terrain
pub const HAS_IV: u8 = 4; // some prism covers this cell
pub const OOB: u8 = 8; // outside the map bounds (wall)
pub const HAS_FLOOR: u8 = 16; // a standable surface exists (ignoring wall clearance)

#[derive(Clone, Debug)]
pub struct Prism {
    pub kind: u8, // 0 building, 1 stadium, 2 bridge
    pub base: f32,
    pub top: f32,
    pub ring: Vec<[f32; 2]>,
    pub holes: Vec<Vec<[f32; 2]>>,
    pub bbox: [f32; 4],
}

#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub height: f32,
    pub crown_r: f32,
    pub crown_base: f32,
    pub trunk_r: f32,
    pub cx: f32,
    pub cy: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub radius: f32,
    pub height: f32,
    pub step_up: f32,
    pub max_slope_deg: f32,
    pub max_climb: f32,
}

impl Default for Params {
    fn default() -> Self {
        Params { radius: 2.5, height: 9.0, step_up: 1.2, max_slope_deg: 50.0, max_climb: 45.0 }
    }
}

pub struct World {
    pub nx: usize,
    pub ny: usize,
    pub min_x: f32,
    pub min_y: f32,
    pub bounds: [f32; 4], // minX, maxX, minY, maxY
    pub terr: Vec<f32>,
    pub iv_start: Vec<u32>,
    pub iv: Vec<[f32; 2]>,
    pub floor: Vec<f32>,
    pub flags: Vec<u8>,
    /// Chamfer distance (cells) to the nearest non-standable cell, capped at 7.
    pub wdist: Vec<u8>,
    /// Concealment score per cell from nearby tree crowns (0..255).
    pub cover: Vec<u8>,
    /// Jump edges, forward CSR: from cell -> (to cell, cost)
    pub jf_start: Vec<u32>,
    pub jf: Vec<(u32, f32)>,
    /// Jump edges, reverse CSR: to cell -> (from cell, cost)
    pub jr_start: Vec<u32>,
    pub jr: Vec<(u32, f32)>,
    pub prisms: Vec<Prism>,
    pub trees: Vec<Tree>,
    pub p: Params,
    pub slope_tan: f32,
    /// tree spatial grid (cells of TREE_CELL m): CSR of tree indices whose crown overlaps the cell
    pub tg_nx: usize,
    pub tg_ny: usize,
    pub tg_start: Vec<u32>,
    pub tg: Vec<u32>,
    /// canopy: foliage length (m) along a sight line beyond which it is blocked
    pub canopy_block_len: f32,
}

pub const TREE_CELL: f32 = 16.0;

fn num(v: &Value) -> f32 {
    v.as_f64().unwrap_or(0.0) as f32
}

fn pts(v: &Value) -> Vec<[f32; 2]> {
    let a = v.as_array().cloned().unwrap_or_default();
    let mut out = Vec::with_capacity(a.len() / 2);
    let mut i = 0;
    while i + 1 < a.len() {
        out.push([num(&a[i]), num(&a[i + 1])]);
        i += 2;
    }
    out
}

pub const NB8: [(i32, i32, f32); 8] = [
    (1, 0, 1.0),
    (-1, 0, 1.0),
    (0, 1, 1.0),
    (0, -1, 1.0),
    (1, 1, std::f32::consts::SQRT_2),
    (1, -1, std::f32::consts::SQRT_2),
    (-1, 1, std::f32::consts::SQRT_2),
    (-1, -1, std::f32::consts::SQRT_2),
];

impl World {
    pub fn load(dir: &Path, p: Params) -> Result<World, String> {
        let wj = fs::read_to_string(dir.join("v2/world.json")).map_err(|e| format!("world.json: {e}"))?;
        let w: Value = serde_json::from_str(&wj).map_err(|e| format!("world.json parse: {e}"))?;
        let t = &w["terrain"];
        let nx = t["nx"].as_u64().unwrap_or(1070) as usize;
        let ny = t["ny"].as_u64().unwrap_or(724) as usize;
        let min_x = t["minX"].as_f64().unwrap_or(-513.0) as f32;
        let min_y = t["minY"].as_f64().unwrap_or(-334.0) as f32;
        let step = t["step"].as_f64().unwrap_or(1.0) as f32;
        let scale = t["scale"].as_f64().unwrap_or(0.01) as f32;
        if (step - 1.0).abs() > 1e-6 {
            return Err(format!("terrain step {step} != 1 not supported"));
        }
        let fname = t["file"].as_str().unwrap_or("terrain.i16");
        let raw = fs::read(dir.join("v2").join(fname)).map_err(|e| format!("terrain: {e}"))?;
        if raw.len() < nx * ny * 2 {
            return Err("terrain file too short".into());
        }
        let terr: Vec<f32> =
            (0..nx * ny).map(|k| i16::from_le_bytes([raw[2 * k], raw[2 * k + 1]]) as f32 * scale).collect();
        let b = &w["bounds"];
        let bounds = [
            b["minX"].as_f64().unwrap_or(min_x as f64) as f32,
            b["maxX"].as_f64().unwrap_or((min_x + nx as f32 - 1.0) as f64) as f32,
            b["minY"].as_f64().unwrap_or(min_y as f64) as f32,
            b["maxY"].as_f64().unwrap_or((min_y + ny as f32 - 1.0) as f64) as f32,
        ];
        let mut prisms = Vec::new();
        for pr in w["prisms"].as_array().cloned().unwrap_or_default() {
            let kind = match pr["kind"].as_str().unwrap_or("building") {
                "stadium" => 1,
                "bridge" => 2,
                _ => 0,
            };
            let ring = pts(&pr["ring"]);
            if ring.len() < 3 {
                continue;
            }
            let holes: Vec<Vec<[f32; 2]>> = pr["holes"]
                .as_array()
                .map(|hs| hs.iter().map(pts).filter(|h| h.len() >= 3).collect())
                .unwrap_or_default();
            let mut bbox = [f32::MAX, f32::MIN, f32::MAX, f32::MIN];
            for q in &ring {
                bbox[0] = bbox[0].min(q[0]);
                bbox[1] = bbox[1].max(q[0]);
                bbox[2] = bbox[2].min(q[1]);
                bbox[3] = bbox[3].max(q[1]);
            }
            prisms.push(Prism { kind, base: num(&pr["base"]), top: num(&pr["top"]), ring, holes, bbox });
        }
        let mut trees = Vec::new();
        if let Some(tr) = w.get("trees") {
            let fields: Vec<String> = tr["fields"]
                .as_array()
                .map(|a| a.iter().map(|s| s.as_str().unwrap_or("").to_string()).collect())
                .unwrap_or_default();
            let data = tr["data"].as_array().cloned().unwrap_or_default();
            let nf = fields.len().max(1);
            let idx = |name: &str| fields.iter().position(|f| f == name);
            let (ix, iy, iz, ih, icr, icb, itr, icx, icy) = (
                idx("x"),
                idx("y"),
                idx("z"),
                idx("height"),
                idx("crownR"),
                idx("crownBase"),
                idx("trunkR"),
                idx("crownX"),
                idx("crownY"),
            );
            let g = |row: usize, i: Option<usize>, d: f32| i.map(|i| num(&data[row * nf + i])).unwrap_or(d);
            for r in 0..data.len() / nf {
                let x = g(r, ix, 0.0);
                let y = g(r, iy, 0.0);
                trees.push(Tree {
                    x,
                    y,
                    z: g(r, iz, 0.0),
                    height: g(r, ih, 10.0),
                    crown_r: g(r, icr, 3.0),
                    crown_base: g(r, icb, 3.0),
                    trunk_r: g(r, itr, 0.5),
                    cx: g(r, icx, x),
                    cy: g(r, icy, y),
                });
            }
        }
        let n = nx * ny;
        let mut wd = World {
            nx,
            ny,
            min_x,
            min_y,
            bounds,
            terr,
            iv_start: vec![0; n + 1],
            iv: Vec::new(),
            floor: vec![0.0; n],
            flags: vec![0; n],
            wdist: vec![0; n],
            cover: vec![0; n],
            jf_start: vec![0; n + 1],
            jf: Vec::new(),
            jr_start: vec![0; n + 1],
            jr: Vec::new(),
            prisms,
            trees,
            p,
            slope_tan: (p.max_slope_deg.to_radians()).tan(),
            tg_nx: 0,
            tg_ny: 0,
            tg_start: Vec::new(),
            tg: Vec::new(),
            canopy_block_len: {
                let r = &w["rules"];
                let dens = r["canopyDensity"].as_f64().unwrap_or(0.35).max(1e-3);
                let bt = r["canopyBlockT"].as_f64().unwrap_or(0.25).clamp(1e-3, 0.999);
                ((1.0 / bt).ln() / dens) as f32
            },
        };
        wd.rasterize();
        wd.compute_floor();
        wd.compute_clearance();
        wd.compute_wdist();
        wd.compute_jumps();
        wd.compute_cover();
        wd.build_tree_grid();
        Ok(wd)
    }

    #[inline]
    pub fn idx(&self, i: usize, j: usize) -> usize {
        j * self.nx + i
    }
    #[inline]
    pub fn cell_xy(&self, c: usize) -> (f32, f32) {
        ((c % self.nx) as f32 + self.min_x, (c / self.nx) as f32 + self.min_y)
    }
    #[inline]
    pub fn cell_of(&self, x: f32, y: f32) -> Option<usize> {
        let i = (x - self.min_x).round();
        let j = (y - self.min_y).round();
        if i < 0.0 || j < 0.0 || i >= self.nx as f32 || j >= self.ny as f32 {
            return None;
        }
        Some(j as usize * self.nx + i as usize)
    }
    #[inline]
    pub fn ivs(&self, c: usize) -> &[[f32; 2]] {
        &self.iv[self.iv_start[c] as usize..self.iv_start[c + 1] as usize]
    }
    #[inline]
    pub fn standable(&self, c: usize) -> bool {
        self.flags[c] & STAND != 0
    }

    /// Bilinear terrain height.
    pub fn ground(&self, x: f32, y: f32) -> f32 {
        let fx = (x - self.min_x).clamp(0.0, (self.nx - 1) as f32 - 1e-3);
        let fy = (y - self.min_y).clamp(0.0, (self.ny - 1) as f32 - 1e-3);
        let i = fx.floor() as usize;
        let j = fy.floor() as usize;
        let i1 = (i + 1).min(self.nx - 1);
        let j1 = (j + 1).min(self.ny - 1);
        let ax = fx - i as f32;
        let ay = fy - j as f32;
        let h00 = self.terr[j * self.nx + i];
        let h10 = self.terr[j * self.nx + i1];
        let h01 = self.terr[j1 * self.nx + i];
        let h11 = self.terr[j1 * self.nx + i1];
        (h00 * (1.0 - ax) + h10 * ax) * (1.0 - ay) + (h01 * (1.0 - ax) + h11 * ax) * ay
    }

    fn rasterize(&mut self) {
        let mut entries: Vec<(u32, f32, f32)> = Vec::new();
        for pr in &self.prisms {
            let j0 = ((pr.bbox[2] - self.min_y).ceil().max(0.0)) as i64;
            let j1 = ((pr.bbox[3] - self.min_y).floor()).min(self.ny as f32 - 1.0) as i64;
            let mut xs: Vec<f32> = Vec::new();
            for j in j0..=j1 {
                let y = self.min_y + j as f32;
                xs.clear();
                let mut add_edges = |poly: &Vec<[f32; 2]>| {
                    let m = poly.len();
                    for k in 0..m {
                        let a = poly[k];
                        let b = poly[(k + 1) % m];
                        if (a[1] <= y) != (b[1] <= y) {
                            xs.push(a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]));
                        }
                    }
                };
                add_edges(&pr.ring);
                for h in &pr.holes {
                    add_edges(h);
                }
                xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let mut k = 0;
                while k + 1 < xs.len() {
                    let i0 = ((xs[k] - self.min_x).ceil().max(0.0)) as i64;
                    let i1 = ((xs[k + 1] - self.min_x).floor()).min(self.nx as f32 - 1.0) as i64;
                    for i in i0..=i1 {
                        entries.push(((j as usize * self.nx + i as usize) as u32, pr.base, pr.top));
                    }
                    k += 2;
                }
            }
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.partial_cmp(&b.1).unwrap()));
        let n = self.nx * self.ny;
        let mut counts = vec![0u32; n + 1];
        for e in &entries {
            counts[e.0 as usize] += 1;
        }
        let mut s = 0u32;
        for c in 0..n {
            self.iv_start[c] = s;
            s += counts[c];
        }
        self.iv_start[n] = s;
        self.iv = entries.iter().map(|e| [e.1, e.2]).collect();
        for c in 0..n {
            if counts[c] > 0 {
                self.flags[c] |= HAS_IV;
            }
        }
    }

    fn compute_floor(&mut self) {
        let h = self.p.height;
        let n = self.nx * self.ny;
        let mut cands: Vec<f32> = Vec::new();
        for c in 0..n {
            let (x, y) = self.cell_xy(c);
            if x < self.bounds[0] || x > self.bounds[1] || y < self.bounds[2] || y > self.bounds[3] {
                self.flags[c] |= OOB;
                self.floor[c] = self.terr[c];
                continue;
            }
            let t = self.terr[c];
            if self.flags[c] & HAS_IV == 0 {
                self.floor[c] = t;
                self.flags[c] |= HAS_FLOOR;
                continue;
            }
            cands.clear();
            cands.push(t);
            let ivs = self.ivs(c);
            for iv in ivs {
                if iv[1] > t - 0.05 {
                    cands.push(iv[1].max(t));
                }
            }
            cands.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mut found = None;
            for &s in &cands {
                let ok = ivs.iter().all(|iv| !(iv[0] < s + h && iv[1] > s + 0.05));
                if ok {
                    found = Some(s);
                    break;
                }
            }
            match found {
                Some(s) => {
                    self.floor[c] = s;
                    self.flags[c] |= HAS_FLOOR;
                    if s > t + 0.01 {
                        self.flags[c] |= PRISM_FLOOR;
                    }
                }
                None => {
                    self.floor[c] = ivs.iter().fold(t, |m, iv| m.max(iv[1]));
                }
            }
        }
    }

    /// Does cell n obstruct a mech whose feet are at height h (a wall, a low overhang or the map edge)?
    #[inline]
    pub fn obstructs(&self, n: usize, h: f32) -> bool {
        let f = self.flags[n];
        if f & OOB != 0 {
            return true;
        }
        if f & HAS_IV == 0 {
            return false;
        }
        let top = h + self.p.height - 0.3;
        let lo = h + self.p.step_up;
        self.ivs(n).iter().any(|iv| iv[0] < top && iv[1] > lo)
    }

    fn disc(r: f32) -> Vec<(i32, i32)> {
        let ri = r.ceil() as i32;
        let mut v = Vec::new();
        for dj in -ri..=ri {
            for di in -ri..=ri {
                if (di * di + dj * dj) as f32 <= r * r {
                    v.push((di, dj));
                }
            }
        }
        v
    }

    fn compute_clearance(&mut self) {
        let r = self.p.radius + 0.7;
        let disc = Self::disc(r);
        let n = self.nx * self.ny;
        let mut stand = vec![false; n];
        for j in 0..self.ny {
            for i in 0..self.nx {
                let c = j * self.nx + i;
                if self.flags[c] & HAS_FLOOR == 0 || self.flags[c] & OOB != 0 {
                    continue;
                }
                let h = self.floor[c];
                let mut ok = true;
                for &(di, dj) in &disc {
                    let ii = i as i32 + di;
                    let jj = j as i32 + dj;
                    if ii < 0 || jj < 0 || ii >= self.nx as i32 || jj >= self.ny as i32 {
                        ok = false;
                        break;
                    }
                    let nc = jj as usize * self.nx + ii as usize;
                    if self.obstructs(nc, h) {
                        ok = false;
                        break;
                    }
                }
                stand[c] = ok;
            }
        }
        for c in 0..n {
            if stand[c] {
                self.flags[c] |= STAND;
            }
        }
    }

    fn compute_wdist(&mut self) {
        // two-pass chamfer distance (1, 1.4) to the nearest non-standable cell, in cells, capped
        let n = self.nx * self.ny;
        let mut d = vec![0f32; n];
        for c in 0..n {
            d[c] = if self.standable(c) { 99.0 } else { 0.0 };
        }
        let nx = self.nx;
        for j in 0..self.ny {
            for i in 0..nx {
                let c = j * nx + i;
                if d[c] == 0.0 {
                    continue;
                }
                let mut m = d[c];
                if i > 0 {
                    m = m.min(d[c - 1] + 1.0);
                }
                if j > 0 {
                    m = m.min(d[c - nx] + 1.0);
                    if i > 0 {
                        m = m.min(d[c - nx - 1] + 1.4);
                    }
                    if i + 1 < nx {
                        m = m.min(d[c - nx + 1] + 1.4);
                    }
                }
                d[c] = m;
            }
        }
        for j in (0..self.ny).rev() {
            for i in (0..nx).rev() {
                let c = j * nx + i;
                if d[c] == 0.0 {
                    continue;
                }
                let mut m = d[c];
                if i + 1 < nx {
                    m = m.min(d[c + 1] + 1.0);
                }
                if j + 1 < self.ny {
                    m = m.min(d[c + nx] + 1.0);
                    if i + 1 < nx {
                        m = m.min(d[c + nx + 1] + 1.4);
                    }
                    if i > 0 {
                        m = m.min(d[c + nx - 1] + 1.4);
                    }
                }
                d[c] = m;
            }
        }
        for c in 0..n {
            self.wdist[c] = d[c].min(7.0) as u8;
        }
    }

    /// Jump edges: climbs (jump + jet) up onto higher standable surfaces and drops off ledges,
    /// across the band of non-standable cells along a wall.
    fn compute_jumps(&mut self) {
        let n = self.nx * self.ny;
        let step = self.p.step_up;
        let hgt = self.p.height;
        let mut edges: Vec<(u32, u32, f32)> = Vec::new();
        let dirs: Vec<(f32, f32)> = (0..16)
            .map(|k| {
                let a = k as f32 * std::f32::consts::PI / 8.0;
                (a.cos(), a.sin())
            })
            .collect();
        for c in 0..n {
            if !self.standable(c) || self.wdist[c] > 1 {
                continue;
            }
            let (x0, y0) = self.cell_xy(c);
            let f0 = self.floor[c];
            for &(dx, dy) in &dirs {
                let mut max_between = f32::MIN;
                let mut lowest_base_between = f32::MAX;
                let mut k = 1.0f32;
                while k <= 11.0 {
                    let x = x0 + dx * k;
                    let y = y0 + dy * k;
                    let nc = match self.cell_of(x, y) {
                        Some(v) => v,
                        None => break,
                    };
                    k += 1.0;
                    if nc == c {
                        continue;
                    }
                    if self.flags[nc] & OOB != 0 {
                        break;
                    }
                    if self.standable(nc) {
                        let f1 = self.floor[nc];
                        let d = f1 - f0;
                        if d.abs() <= step {
                            break; // ordinary walking connection
                        }
                        let both_terrain =
                            self.flags[c] & PRISM_FLOOR == 0 && self.flags[nc] & PRISM_FLOOR == 0;
                        if both_terrain && k <= 3.0 {
                            break; // steep terrain, handled by the slope rule
                        }
                        if d > 0.0 {
                            if d > self.p.max_climb {
                                break;
                            }
                            if max_between > f1 + step {
                                break;
                            }
                            // an underside between us and the roof would stop the jet
                            if lowest_base_between > f0 + step && lowest_base_between < f1 + hgt {
                                break;
                            }
                            let dist = k - 1.0;
                            edges.push((c as u32, nc as u32, dist + 4.0 + d * 1.2));
                        } else {
                            if max_between > f0 + step {
                                break;
                            }
                            let dist = k - 1.0;
                            edges.push((c as u32, nc as u32, dist + 1.5 + (-d) * 0.05));
                        }
                        break;
                    } else {
                        let fl = self.floor[nc];
                        max_between = max_between.max(fl);
                        for iv in self.ivs(nc) {
                            max_between = max_between.max(iv[1]);
                            if iv[0] > fl + 0.05 {
                                lowest_base_between = lowest_base_between.min(iv[0]);
                            }
                        }
                    }
                }
            }
        }
        // dedupe (from,to) keeping min cost
        edges.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.partial_cmp(&b.2).unwrap()));
        edges.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
        let mut cnt = vec![0u32; n + 1];
        for e in &edges {
            cnt[e.0 as usize] += 1;
        }
        let mut s = 0;
        for c in 0..n {
            self.jf_start[c] = s;
            s += cnt[c];
        }
        self.jf_start[n] = s;
        self.jf = edges.iter().map(|e| (e.1, e.2)).collect();
        let mut rev: Vec<(u32, u32, f32)> = edges.iter().map(|e| (e.1, e.0, e.2)).collect();
        rev.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut cnt = vec![0u32; n + 1];
        for e in &rev {
            cnt[e.0 as usize] += 1;
        }
        let mut s = 0;
        for c in 0..n {
            self.jr_start[c] = s;
            s += cnt[c];
        }
        self.jr_start[n] = s;
        self.jr = rev.iter().map(|e| (e.1, e.2)).collect();
    }

    fn compute_cover(&mut self) {
        // crowns within ~3 m above-ground band that hide a mech from far viewers
        let mut acc = vec![0f32; self.nx * self.ny];
        for t in &self.trees {
            let r = t.crown_r.max(1.0);
            if t.height - t.z < 6.0 && t.height < 6.0 {
                // tiny tree; height may be absolute, so this check is lenient
            }
            let ri = r.ceil() as i32 + 1;
            let ci = (t.cx - self.min_x).round() as i32;
            let cj = (t.cy - self.min_y).round() as i32;
            for dj in -ri..=ri {
                for di in -ri..=ri {
                    let ii = ci + di;
                    let jj = cj + dj;
                    if ii < 0 || jj < 0 || ii >= self.nx as i32 || jj >= self.ny as i32 {
                        continue;
                    }
                    let d2 = (di * di + dj * dj) as f32;
                    if d2 <= r * r {
                        acc[jj as usize * self.nx + ii as usize] += 1.0;
                    }
                }
            }
        }
        for c in 0..acc.len() {
            self.cover[c] = (acc[c] * 40.0).min(255.0) as u8;
        }
    }

    fn build_tree_grid(&mut self) {
        let gx = ((self.nx as f32) / TREE_CELL).ceil() as usize + 1;
        let gy = ((self.ny as f32) / TREE_CELL).ceil() as usize + 1;
        let mut pairs: Vec<(u32, u32)> = Vec::new();
        for (k, t) in self.trees.iter().enumerate() {
            if t.height <= t.crown_base {
                continue;
            }
            let r = t.crown_r.max(0.5);
            let i0 = (((t.cx - r - self.min_x) / TREE_CELL).floor().max(0.0)) as usize;
            let i1 = (((t.cx + r - self.min_x) / TREE_CELL).floor().max(0.0) as usize).min(gx - 1);
            let j0 = (((t.cy - r - self.min_y) / TREE_CELL).floor().max(0.0)) as usize;
            let j1 = (((t.cy + r - self.min_y) / TREE_CELL).floor().max(0.0) as usize).min(gy - 1);
            for j in j0..=j1 {
                for i in i0..=i1 {
                    pairs.push(((j * gx + i) as u32, k as u32));
                }
            }
        }
        pairs.sort();
        let mut start = vec![0u32; gx * gy + 1];
        for p in &pairs {
            start[p.0 as usize + 1] += 1;
        }
        for c in 0..gx * gy {
            start[c + 1] += start[c];
        }
        self.tg_nx = gx;
        self.tg_ny = gy;
        self.tg_start = start;
        self.tg = pairs.iter().map(|p| p.1).collect();
    }

    /// Length (m) of the segment a-b inside tree crowns (ellipsoids from crownBase to height).
    pub fn canopy_len(&self, a: [f32; 3], b: [f32; 3]) -> f32 {
        if self.tg_nx == 0 {
            return 0.0;
        }
        let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if len < 1e-3 {
            return 0.0;
        }
        let n = ((len / 4.0).ceil() as i32).max(1);
        let mut cells: [u32; 128] = [u32::MAX; 128];
        let mut nc = 0usize;
        for s in 0..=n {
            let f = s as f32 / n as f32;
            let x = a[0] + d[0] * f;
            let y = a[1] + d[1] * f;
            let i = ((x - self.min_x) / TREE_CELL).floor();
            let j = ((y - self.min_y) / TREE_CELL).floor();
            if i < 0.0 || j < 0.0 || i as usize >= self.tg_nx || j as usize >= self.tg_ny {
                continue;
            }
            let c = (j as usize * self.tg_nx + i as usize) as u32;
            if nc == 0 || cells[nc - 1] != c {
                if !cells[..nc].contains(&c) && nc < 128 {
                    cells[nc] = c;
                    nc += 1;
                }
            }
        }
        let mut seen: Vec<u32> = Vec::new();
        let mut total = 0.0f32;
        for &c in &cells[..nc] {
            let c = c as usize;
            for &k in &self.tg[self.tg_start[c] as usize..self.tg_start[c + 1] as usize] {
                if seen.contains(&k) {
                    continue;
                }
                seen.push(k);
                let t = &self.trees[k as usize];
                let rz = (t.height - t.crown_base) * 0.5;
                if rz <= 0.05 {
                    continue;
                }
                let r = t.crown_r.max(0.3);
                let cz = t.z + t.crown_base + rz;
                // unit-sphere space
                let ox = (a[0] - t.cx) / r;
                let oy = (a[1] - t.cy) / r;
                let oz = (a[2] - cz) / rz;
                let dx = d[0] / r;
                let dy = d[1] / r;
                let dz = d[2] / rz;
                let qa = dx * dx + dy * dy + dz * dz;
                let qb = 2.0 * (ox * dx + oy * dy + oz * dz);
                let qc = ox * ox + oy * oy + oz * oz - 1.0;
                let disc = qb * qb - 4.0 * qa * qc;
                if disc <= 0.0 || qa < 1e-9 {
                    continue;
                }
                let sq = disc.sqrt();
                let t0 = ((-qb - sq) / (2.0 * qa)).max(0.0);
                let t1 = ((-qb + sq) / (2.0 * qa)).min(1.0);
                if t1 > t0 {
                    total += (t1 - t0) * len;
                }
            }
        }
        total
    }

    /// Would a viewer's eye see a mech whose feet are at `feet` (center or top visible, buildings
    /// and terrain block, dense canopy blocks beyond the near range)?
    pub fn mech_visible(&self, eye: [f32; 3], feet: [f32; 3], height: f32, near_range: f32) -> bool {
        let dx = feet[0] - eye[0];
        let dy = feet[1] - eye[1];
        let near = dx * dx + dy * dy <= near_range * near_range;
        for zoff in [height * 0.5, height * 0.98] {
            let p = [feet[0], feet[1], feet[2] + zoff];
            if !self.los(eye, p) {
                continue;
            }
            if near || self.canopy_len(eye, p) < self.canopy_block_len {
                return true;
            }
        }
        false
    }

    #[inline]
    pub fn jumps_from(&self, c: usize) -> &[(u32, f32)] {
        &self.jf[self.jf_start[c] as usize..self.jf_start[c + 1] as usize]
    }
    #[inline]
    pub fn jumps_to(&self, c: usize) -> &[(u32, f32)] {
        &self.jr[self.jr_start[c] as usize..self.jr_start[c + 1] as usize]
    }

    /// Cost of walking from standable cell a to its 8-neighbour b (None if not walkable).
    #[inline]
    pub fn walk_cost(&self, a: usize, b: usize, len: f32) -> Option<f32> {
        if self.flags[b] & STAND == 0 || self.flags[a] & STAND == 0 {
            return None;
        }
        let d = self.floor[b] - self.floor[a];
        if d > self.p.step_up {
            return None;
        }
        if d > self.slope_tan * len && self.flags[a] & PRISM_FLOOR == 0 && self.flags[b] & PRISM_FLOOR == 0 {
            return None;
        }
        let wd = self.wdist[b];
        let pen = if wd < 4 { (4 - wd) as f32 * 0.35 } else { 0.0 };
        Some(len * (1.0 + pen))
    }

    /// Is the point inside solid geometry (prism or below terrain)?
    #[inline]
    pub fn solid_at(&self, x: f32, y: f32, z: f32) -> bool {
        let c = match self.cell_of(x, y) {
            Some(c) => c,
            None => return true,
        };
        if z < self.terr[c] - 0.2 {
            return true;
        }
        if self.flags[c] & HAS_IV == 0 {
            return false;
        }
        self.ivs(c).iter().any(|iv| z > iv[0] && z < iv[1])
    }

    /// Line of sight for shots and sight (buildings + terrain; trees ignored).
    pub fn los(&self, a: [f32; 3], b: [f32; 3]) -> bool {
        let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let n = (len / 0.7).ceil().max(1.0) as i32;
        for s in 1..n {
            let f = s as f32 / n as f32;
            if self.solid_at(a[0] + d[0] * f, a[1] + d[1] * f, a[2] + d[2] * f) {
                return false;
            }
        }
        true
    }

    /// Fraction of a sight line from a to b that passes through tree crowns (0 = clear).
    pub fn canopy_along(&self, a: [f32; 3], b: [f32; 3]) -> f32 {
        // cheap estimate via the cover grid at the sample points' cells at crown heights
        let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
        let n = (len / 2.0).ceil().max(1.0) as i32;
        let mut acc = 0.0;
        for s in 1..n {
            let f = s as f32 / n as f32;
            if let Some(c) = self.cell_of(a[0] + d[0] * f, a[1] + d[1] * f) {
                acc += self.cover[c] as f32 / 255.0 * 2.0;
            }
        }
        acc
    }

    /// Highest supporting surface at (x,y) not above z + tol (terrain or prism top).
    pub fn support(&self, x: f32, y: f32, z: f32, tol: f32) -> f32 {
        let g = self.ground(x, y);
        let c = match self.cell_of(x, y) {
            Some(c) => c,
            None => return g,
        };
        let mut s = g;
        if self.flags[c] & HAS_IV != 0 {
            for iv in self.ivs(c) {
                if iv[1] <= z + tol && iv[1] > s {
                    s = iv[1];
                }
            }
        }
        s
    }

    /// Nearest standable cell to (x,y) within r cells, preferring floors near z.
    pub fn nearest_standable(&self, x: f32, y: f32, z: f32, r: i32) -> Option<usize> {
        let ci = (x - self.min_x).round() as i32;
        let cj = (y - self.min_y).round() as i32;
        let mut best = None;
        let mut bd = f32::MAX;
        for dj in -r..=r {
            for di in -r..=r {
                let ii = ci + di;
                let jj = cj + dj;
                if ii < 0 || jj < 0 || ii >= self.nx as i32 || jj >= self.ny as i32 {
                    continue;
                }
                let c = jj as usize * self.nx + ii as usize;
                if !self.standable(c) {
                    continue;
                }
                let dz = (self.floor[c] - z).abs();
                let d = ((di * di + dj * dj) as f32).sqrt() + if dz > 3.0 { dz * 2.0 } else { 0.0 };
                if d < bd {
                    bd = d;
                    best = Some(c);
                }
            }
        }
        best
    }

    /// Straight-line walkability between two points on the floor layer (no climbs).
    pub fn walkable_line(&self, a: (f32, f32), b: (f32, f32)) -> bool {
        let dx = b.0 - a.0;
        let dy = b.1 - a.1;
        let len = (dx * dx + dy * dy).sqrt();
        let n = (len / 0.7).ceil().max(1.0) as i32;
        let mut prev = match self.cell_of(a.0, a.1) {
            Some(c) => c,
            None => return false,
        };
        for s in 1..=n {
            let f = s as f32 / n as f32;
            let c = match self.cell_of(a.0 + dx * f, a.1 + dy * f) {
                Some(c) => c,
                None => return false,
            };
            if c == prev {
                continue;
            }
            if !self.standable(c) {
                return false;
            }
            let pc = self.cell_xy(prev);
            let cc = self.cell_xy(c);
            let l = ((pc.0 - cc.0).powi(2) + (pc.1 - cc.1).powi(2)).sqrt();
            if self.floor[c] - self.floor[prev] > self.p.step_up.min(self.slope_tan * l + 0.05) {
                return false;
            }
            prev = c;
        }
        true
    }
}
