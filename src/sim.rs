//! A small local simulator of mechfight physics (protocol §7) used to smoke-test navigation:
//! mech movement with acceleration, wall collision and sliding, step-up, slopes, gravity,
//! jump/jet, energy, and the shrinking zone. It grows towards a full FFA simulator.
use crate::proto::*;
use crate::world::World;

#[derive(Clone, Debug)]
pub struct Phys {
    pub walk_speed: f64,
    pub boost_speed: f64,
    pub ground_accel: f64,
    pub air_accel: f64,
    pub gravity: f64,
    pub jump_speed: f64,
    pub jet_accel: f64,
    pub max_rise: f64,
    pub step_up: f64,
    pub energy_max: f64,
    pub energy_regen: f64,
    pub regen_delay: f64,
    pub boost_drain: f64,
    pub jet_drain: f64,
    pub radius: f64,
    pub height: f64,
    pub slope_tan: f64,
}

impl Phys {
    pub fn from(cfg: &Config) -> Self {
        Phys {
            walk_speed: cfg.f("mech", "walk_speed", 11.7),
            boost_speed: cfg.f("mech", "boost_speed", 19.5),
            ground_accel: cfg.f("mech", "ground_accel", 58.5),
            air_accel: cfg.f("mech", "air_accel", 12.0),
            gravity: cfg.f("mech", "gravity", 22.0),
            jump_speed: cfg.f("mech", "jump_speed", 11.0),
            jet_accel: cfg.f("mech", "jet_accel", 34.0),
            max_rise: cfg.f("mech", "max_rise_speed", 10.0),
            step_up: cfg.f("mech", "step_up", 1.2),
            energy_max: cfg.f("mech", "energy", 100.0),
            energy_regen: cfg.f("mech", "energy_regen", 18.0),
            regen_delay: cfg.f("mech", "energy_regen_delay", 0.6),
            boost_drain: cfg.f("mech", "boost_drain", 17.0),
            jet_drain: cfg.f("mech", "jet_drain", 15.0),
            radius: cfg.f("mech", "radius", 2.5),
            height: cfg.f("mech", "height", 9.0),
            slope_tan: cfg.f("mech", "max_slope_deg", 50.0).to_radians().tan(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Body {
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub grounded: bool,
    pub energy: f64,
    pub since_spend: f64,
}

/// Does a mech cylinder at (x,y) with feet z intersect a wall (anything taller than step-up)?
pub fn collides(w: &World, ph: &Phys, x: f64, y: f64, z: f64) -> bool {
    let r = ph.radius;
    let ri = r.ceil() as i32 + 1;
    let ci = (x - w.min_x as f64).round() as i32;
    let cj = (y - w.min_y as f64).round() as i32;
    for dj in -ri..=ri {
        for di in -ri..=ri {
            let ii = ci + di;
            let jj = cj + dj;
            if ii < 0 || jj < 0 || ii >= w.nx as i32 || jj >= w.ny as i32 {
                return true;
            }
            let cx = w.min_x as f64 + ii as f64;
            let cy = w.min_y as f64 + jj as f64;
            // distance from cylinder center to the cell square (cells are 1 m squares centered on vertices)
            let qx = (x - cx).abs() - 0.5;
            let qy = (y - cy).abs() - 0.5;
            let dd = qx.max(0.0).hypot(qy.max(0.0));
            if dd > r - 0.15 {
                continue;
            }
            let c = jj as usize * w.nx + ii as usize;
            if w.flags[c] & crate::world::OOB != 0 {
                return true;
            }
            if w.flags[c] & crate::world::HAS_IV == 0 {
                continue;
            }
            for iv in w.ivs(c) {
                if (iv[0] as f64) < z + ph.height - 0.2 && (iv[1] as f64) > z + ph.step_up {
                    return true;
                }
            }
        }
    }
    false
}

/// Direction pushing a mech out of overlapping walls (unit vector, or zero).
fn push_out(w: &World, ph: &Phys, x: f64, y: f64, z: f64) -> [f64; 2] {
    let r = ph.radius;
    let ri = r.ceil() as i32 + 1;
    let ci = (x - w.min_x as f64).round() as i32;
    let cj = (y - w.min_y as f64).round() as i32;
    let mut acc = [0.0, 0.0];
    for dj in -ri..=ri {
        for di in -ri..=ri {
            let ii = ci + di;
            let jj = cj + dj;
            if ii < 0 || jj < 0 || ii >= w.nx as i32 || jj >= w.ny as i32 {
                continue;
            }
            let cx = w.min_x as f64 + ii as f64;
            let cy = w.min_y as f64 + jj as f64;
            let dx = x - cx;
            let dy = y - cy;
            let d = dx.hypot(dy);
            if d > r + 0.5 {
                continue;
            }
            let c = jj as usize * w.nx + ii as usize;
            let blocked = w.ivs(c).iter().any(|iv| (iv[0] as f64) < z + ph.height - 0.2 && (iv[1] as f64) > z + ph.step_up);
            if blocked {
                acc[0] += dx / d.max(0.1);
                acc[1] += dy / d.max(0.1);
            }
        }
    }
    let n = acc[0].hypot(acc[1]);
    if n < 1e-6 {
        [0.0, 0.0]
    } else {
        [acc[0] / n, acc[1] / n]
    }
}

/// Lowest ceiling (prism base) above feet z within the cylinder, if any.
fn ceiling(w: &World, ph: &Phys, x: f64, y: f64, z: f64) -> f64 {
    let mut best = f64::MAX;
    let r = ph.radius;
    let ri = r.ceil() as i32;
    let ci = (x - w.min_x as f64).round() as i32;
    let cj = (y - w.min_y as f64).round() as i32;
    for dj in -ri..=ri {
        for di in -ri..=ri {
            if ((di * di + dj * dj) as f64).sqrt() > r {
                continue;
            }
            let ii = ci + di;
            let jj = cj + dj;
            if ii < 0 || jj < 0 || ii >= w.nx as i32 || jj >= w.ny as i32 {
                continue;
            }
            let c = jj as usize * w.nx + ii as usize;
            for iv in w.ivs(c) {
                let b = iv[0] as f64;
                if b >= z + ph.height - 0.3 && b < best {
                    best = b;
                }
            }
        }
    }
    best
}

/// Advance one physics tick (dt) for a mech with the given action.
pub fn step(w: &World, ph: &Phys, b: &mut Body, mv: [f64; 2], boost: bool, jump: bool, dt: f64) {
    let ml = mv[0].hypot(mv[1]);
    let m = if ml > 1.0 { [mv[0] / ml, mv[1] / ml] } else { mv };
    let moving = ml > 0.05;
    let mut spent = false;
    let boosting = boost && b.grounded && moving && b.energy > 0.0;
    let speed = if boosting { ph.boost_speed } else { ph.walk_speed };
    if boosting {
        b.energy -= ph.boost_drain * dt;
        spent = true;
    }
    let des = [m[0] * speed, m[1] * speed];
    let acc = if b.grounded { ph.ground_accel } else { ph.air_accel };
    let dvx = des[0] - b.vel[0];
    let dvy = des[1] - b.vel[1];
    let dl = dvx.hypot(dvy);
    let lim = acc * dt;
    if dl > lim {
        b.vel[0] += dvx / dl * lim;
        b.vel[1] += dvy / dl * lim;
    } else {
        b.vel[0] = des[0];
        b.vel[1] = des[1];
    }
    if jump && b.grounded {
        b.vel[2] = ph.jump_speed;
        b.grounded = false;
    }
    if !b.grounded {
        if boost && b.energy > 0.0 {
            if b.vel[2] < ph.max_rise {
                b.vel[2] = (b.vel[2] + ph.jet_accel * dt).min(ph.max_rise.max(b.vel[2]));
            }
            b.energy -= ph.jet_drain * dt;
            spent = true;
        }
        b.vel[2] -= ph.gravity * dt;
    }
    // horizontal move with sliding
    let (x, y, z) = (b.pos[0], b.pos[1], b.pos[2]);
    if collides(w, ph, x, y, z) {
        let p = push_out(w, ph, x, y, z);
        b.pos[0] += p[0] * 0.6;
        b.pos[1] += p[1] * 0.6;
    } else {
        let nx = x + b.vel[0] * dt;
        let ny = y + b.vel[1] * dt;
        // allow stepping up: test at the support height of the destination if within step
        let ok = |px: f64, py: f64| -> bool {
            if !collides(w, ph, px, py, z) {
                // slope rule on terrain: can't walk up steeper than max slope
                if b.grounded {
                    let g0 = w.ground(x as f32, y as f32) as f64;
                    let g1 = w.ground(px as f32, py as f32) as f64;
                    let d = (px - x).hypot(py - y).max(1e-6);
                    if (z - g0).abs() < 0.3 && g1 - g0 > ph.slope_tan * d + 0.02 {
                        return false;
                    }
                }
                return true;
            }
            false
        };
        if ok(nx, ny) {
            b.pos[0] = nx;
            b.pos[1] = ny;
        } else if ok(nx, y) {
            b.pos[0] = nx;
            b.vel[1] = 0.0;
        } else if ok(x, ny) {
            b.pos[1] = ny;
            b.vel[0] = 0.0;
        } else {
            b.vel[0] = 0.0;
            b.vel[1] = 0.0;
        }
    }
    // vertical
    let z0 = b.pos[2];
    let mut nz = z0 + b.vel[2] * dt;
    if b.vel[2] > 0.0 {
        let ceil = ceiling(w, ph, b.pos[0], b.pos[1], z0);
        if nz + ph.height > ceil {
            nz = (ceil - ph.height).max(z0);
            b.vel[2] = 0.0;
        }
    }
    let sup = w.support(b.pos[0] as f32, b.pos[1] as f32, (z0.max(nz)) as f32, ph.step_up as f32) as f64;
    if nz <= sup + 1e-6 {
        b.pos[2] = sup;
        b.vel[2] = 0.0;
        b.grounded = true;
    } else if b.grounded && nz - sup < 0.6 && b.vel[2] <= 0.0 {
        b.pos[2] = sup; // follow terrain down small slopes
        b.vel[2] = 0.0;
    } else {
        b.pos[2] = nz;
        b.grounded = false;
    }
    if spent {
        b.since_spend = 0.0;
    } else {
        b.since_spend += dt;
        if b.since_spend >= ph.regen_delay {
            b.energy = (b.energy + ph.energy_regen * dt).min(ph.energy_max);
        }
    }
    b.energy = b.energy.max(0.0);
}

/// Zone schedule generated like the server's: each next circle lies inside the current one.
#[derive(Clone, Debug)]
pub struct ZoneSched {
    pub circles: Vec<([f64; 2], f64)>, // circle i (0 = start)
    pub times: Vec<(f64, f64)>,        // for phase i: (shrink_start, shrink_end)
    pub dps: Vec<f64>,
}

impl ZoneSched {
    /// `bounds` (minX, maxX, minY, maxY), when given, keeps every circle center inside the map.
    pub fn new(cfg: &Config, rng: &mut crate::bot::Rng, bounds: Option<[f64; 4]>) -> Self {
        let sc = cfg
            .raw
            .get("zone")
            .and_then(|z| z.get("start_center"))
            .and_then(|v| v.as_array())
            .map(|a| [a[0].as_f64().unwrap_or(41.3), a[1].as_f64().unwrap_or(-72.8)])
            .unwrap_or([41.3, -72.8]);
        let sr = cfg.f("zone", "start_radius", 560.0);
        let mut circles = vec![(sc, sr)];
        let mut times = Vec::new();
        let mut dps = Vec::new();
        let mut t = 0.0;
        for (hold, shrink, r, d) in cfg.zone_phases() {
            let (c, rc) = *circles.last().unwrap();
            let maxoff = (rc - r).max(0.0);
            let mut nc = c;
            for _ in 0..200 {
                let a = rng.f() * std::f64::consts::TAU;
                let off = maxoff * rng.f().sqrt();
                nc = [c[0] + a.cos() * off, c[1] + a.sin() * off];
                match bounds {
                    Some(b) if nc[0] < b[0] + 30.0 || nc[0] > b[1] - 30.0 || nc[1] < b[2] + 30.0 || nc[1] > b[3] - 30.0 => continue,
                    _ => break,
                }
            }
            circles.push((nc, r));
            t += hold;
            times.push((t, t + shrink));
            t += shrink;
            dps.push(d);
        }
        ZoneSched { circles, times, dps }
    }

    pub fn obs(&self, t: f64) -> ZoneObs {
        let n = self.times.len();
        let mut i = 0;
        while i < n && t >= self.times[i].1 {
            i += 1;
        }
        if i >= n {
            let (c, r) = *self.circles.last().unwrap();
            return ZoneObs {
                center: c,
                radius: r,
                next_center: c,
                next_radius: r,
                shrink_start: self.times[n - 1].0,
                shrink_end: self.times[n - 1].1,
                dps: *self.dps.last().unwrap_or(&260.0),
            };
        }
        let (c0, r0) = self.circles[i];
        let (c1, r1) = self.circles[i + 1];
        ZoneObs {
            center: c0,
            radius: r0,
            next_center: c1,
            next_radius: r1,
            shrink_start: self.times[i].0,
            shrink_end: self.times[i].1,
            dps: self.dps[i],
        }
    }
}

pub fn default_config() -> Config {
    let s = include_str!("default_config.json");
    Config::new(serde_json::from_str(s).unwrap())
}
