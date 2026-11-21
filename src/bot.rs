//! The decision layer: memory of enemies, zone rotation, loot, cover, combat and the
//! movement executor (path following, climbs, stuck recovery).
use crate::nav::{AStar, FlowField};
use crate::proto::*;
use crate::world::World;
use std::collections::{HashMap, VecDeque};
use std::f64::consts::PI;
use std::time::Instant;

#[derive(Clone, Debug, Default)]
pub struct Track {
    pub id: i64,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub aim: [f64; 2],
    pub hp: f64,
    pub max_hp: f64,
    pub weapons: Vec<String>,
    pub charging: bool,
    pub seen_t: f64,
    pub first_t: f64,
    pub cubes: f64,
    /// last time this mech damaged us
    pub hurt_us_t: f64,
    pub is_npc: bool,
}

/// Behaviour parameters (the survivor defaults are what ships; the simulator also runs an
/// aggressive variant as a sparring partner).
#[derive(Clone, Debug)]
pub struct Tune {
    pub aggressive: bool,
    /// always fight enemies closer than this (they see us through proximity sensors anyway)
    pub fight_close: f64,
    /// fight back against an attacker within this range when healthy
    pub fight_back_range: f64,
    /// hunt weak enemies within this range
    pub hunt_weak_range: f64,
    /// retreat to cover below this HP fraction
    pub retreat_hp: f64,
    /// break line of sight from attackers farther than fight_back_range
    pub cover_from_far: bool,
    /// shoot crates when calm
    pub crates: bool,
    /// only retreat when cover is at most this far (m); otherwise keep fighting
    pub retreat_cover_max: f64,
    /// strafe direction change interval range (s)
    pub strafe_min: f64,
    pub strafe_max: f64,
    /// zig-zag while retreating/covering
    pub zigzag: bool,
    /// chase the target at any distance (simulator hunters)
    pub hunt: bool,
    /// boost while strafing in fights when energy allows
    pub boost_strafe: bool,
    /// weapon loot search radius while slot 1 is empty
    pub loot_radius: f64,
    /// seconds to stay in cover after the last hit from a far attacker
    pub cover_hold_s: f64,
    /// when only one enemy is around, duel it head-on (no cover runs, later retreat)
    pub solo_duel: bool,
    /// sidestep incoming bullets and mortar impacts
    pub dodge: bool,
}

impl Default for Tune {
    fn default() -> Self {
        Tune {
            aggressive: false,
            fight_close: 55.0,
            fight_back_range: 90.0,
            hunt_weak_range: 160.0,
            retreat_hp: 0.45,
            cover_from_far: true,
            crates: true,
            retreat_cover_max: 1e9,
            strafe_min: 0.8,
            strafe_max: 2.0,
            zigzag: false,
            hunt: false,
            boost_strafe: false,
            loot_radius: 70.0,
            cover_hold_s: 8.0,
            solo_duel: false,
            dodge: true,
        }
    }
}

impl Tune {
    pub fn aggressive() -> Self {
        Tune {
            aggressive: true,
            fight_close: 200.0,
            fight_back_range: 230.0,
            hunt_weak_range: 230.0,
            retreat_hp: 0.0,
            cover_from_far: false,
            crates: true,
            ..Tune::default()
        }
    }

    /// Apply overrides like "retreat_hp=0.3,zigzag=1" (used by the simulator for A/B tests).
    pub fn apply(&mut self, spec: &str) {
        for kv in spec.split(',') {
            let mut it = kv.splitn(2, '=');
            let k = it.next().unwrap_or("").trim();
            let v: f64 = match it.next().and_then(|v| v.trim().parse().ok()) {
                Some(v) => v,
                None => continue,
            };
            match k {
                "fight_close" => self.fight_close = v,
                "fight_back_range" => self.fight_back_range = v,
                "hunt_weak_range" => self.hunt_weak_range = v,
                "retreat_hp" => self.retreat_hp = v,
                "cover_from_far" => self.cover_from_far = v != 0.0,
                "crates" => self.crates = v != 0.0,
                "retreat_cover_max" => self.retreat_cover_max = v,
                "strafe_min" => self.strafe_min = v,
                "strafe_max" => self.strafe_max = v,
                "zigzag" => self.zigzag = v != 0.0,
                "aggressive" => self.aggressive = v != 0.0,
                "boost_strafe" => self.boost_strafe = v != 0.0,
                "loot_radius" => self.loot_radius = v,
                "cover_hold_s" => self.cover_hold_s = v,
                "solo_duel" => self.solo_duel = v != 0.0,
                "dodge" => self.dodge = v != 0.0,
                _ => eprintln!("unknown tune key {k}"),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Cover,
    Rotate,
    Hold,
    Loot,
    Fight,
    Retreat,
    Drop,
    Unstuck,
}

/// Small deterministic RNG (xorshift).
pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub fn f(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

pub fn wrap(a: f64) -> f64 {
    let mut a = a % (2.0 * PI);
    if a > PI {
        a -= 2.0 * PI;
    }
    if a < -PI {
        a += 2.0 * PI;
    }
    a
}

fn d2(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn f3(a: [f64; 3]) -> [f32; 3] {
    [a[0] as f32, a[1] as f32, a[2] as f32]
}

pub struct Bot {
    pub cfg: Config,
    pub world: Option<&'static World>,
    pub rng: Rng,
    pub slot: i64,
    // config numbers
    walk_speed: f64,
    mech_h: f64,
    eye_h: f64,
    muzzle_h: f64,
    pickup_r: f64,
    cube_r: f64,
    near_range: f64,
    // nav state
    ff: FlowField,
    ff_next: FlowField,
    ff_ready: bool,
    astar: AStar,
    path: Vec<usize>,
    local_path: Vec<usize>,
    local_goal: Option<(f64, f64)>,
    local_path_t: f64,
    climbing: Option<(usize, f64)>, // (target cell, start t)
    pos_hist: VecDeque<(f64, [f64; 3], f64, f64)>, // t, pos, commanded speed (0..1), actual horiz speed
    stuck_until: f64,
    stuck_dir: [f64; 2],
    stuck_count: u32,
    last_stuck_t: f64,
    // memory
    pub tracks: HashMap<i64, Track>,
    dmg_log: VecDeque<(f64, Option<f64>, Option<i64>, f64, String)>, // t, bearing, by, amount, weapon
    last_hurt_t: f64,
    last_dealt_t: f64,
    sounds: VecDeque<(f64, f64, String, String)>, // t, bearing, dist band, kind
    hold_spot: Option<(usize, (i64, i64, i64))>,
    strafe_sign: f64,
    strafe_t: f64,
    scan_yaw: f64,
    pub mode: Mode,
    laser_hold: bool,
    pub t: f64,
    blocked_pickups: HashMap<i64, f64>,
    loot_target: Option<(i64, f64)>,
    cover_until: f64,
    cover_spot: Option<(f64, f64)>,
    cover_threats: Vec<[f64; 3]>,
    crate_target: Option<i64>,
    pub debug: bool,
    pub tune: Tune,
    pub label: String,
    pub stats_n: u64,
    last_move: [f64; 2],
    deadline: Instant,
}

impl Bot {
    pub fn new(cfg: Config, world: Option<&'static World>, slot: i64, seed: u64) -> Self {
        let n = world.as_ref().map(|w| w.nx * w.ny).unwrap_or(1);
        Bot {
            walk_speed: cfg.f("mech", "walk_speed", 11.7),
            mech_h: cfg.f("mech", "height", 9.0),
            eye_h: cfg.f("mech", "eye_height", 8.0),
            muzzle_h: cfg.f("mech", "muzzle_height", 7.0),
            pickup_r: cfg.f("loot", "pickup_radius", 4.0),
            cube_r: cfg.f("cubes", "pickup_radius", 3.5),
            near_range: cfg.f("vision", "near_range", 45.0),
            cfg,
            world,
            rng: Rng::new(seed ^ 0xA5A5_1234),
            slot,
            ff: FlowField::new(n),
            ff_next: FlowField::new(n),
            ff_ready: false,
            astar: AStar::new(n),
            path: Vec::new(),
            local_path: Vec::new(),
            local_goal: None,
            local_path_t: -99.0,
            climbing: None,
            pos_hist: VecDeque::new(),
            stuck_until: -1.0,
            stuck_dir: [0.0, 0.0],
            stuck_count: 0,
            last_stuck_t: -99.0,
            tracks: HashMap::new(),
            dmg_log: VecDeque::new(),
            last_hurt_t: -99.0,
            last_dealt_t: -99.0,
            sounds: VecDeque::new(),
            hold_spot: None,
            strafe_sign: 1.0,
            strafe_t: 0.0,
            scan_yaw: 0.0,
            mode: Mode::Rotate,
            laser_hold: false,
            t: 0.0,
            blocked_pickups: HashMap::new(),
            loot_target: None,
            cover_until: -1.0,
            cover_spot: None,
            cover_threats: Vec::new(),
            crate_target: None,
            debug: false,
            tune: Tune::default(),
            label: String::new(),
            stats_n: 0,
            last_move: [0.0, 0.0],
            deadline: Instant::now(),
        }
    }

    // ------------------------------------------------------------------ memory
    fn update_memory(&mut self, o: &Obs) {
        self.t = o.t;
        let me = &o.me;
        for m in &o.mechs {
            let id = m.id as i64;
            if id == me.id as i64 {
                continue;
            }
            let e = self.tracks.entry(id).or_insert_with(|| Track { id, first_t: o.t, ..Default::default() });
            e.pos = m.pos;
            e.vel = m.vel;
            e.aim = m.aim;
            e.hp = m.hp.unwrap_or(e.hp.max(1.0));
            e.max_hp = m.max_hp.unwrap_or(1400.0);
            e.weapons = m.weapons.clone();
            e.charging = m.charging;
            e.seen_t = o.t;
            e.cubes = m.cubes.unwrap_or(0.0);
            e.is_npc = m.kind == "npc";
        }
        // forget stale tracks
        let t = o.t;
        self.tracks.retain(|_, v| t - v.seen_t < 20.0);
        for ev in &o.events {
            match ev.kind.as_str() {
                "damaged" => {
                    let w = ev.weapon.clone().unwrap_or_default();
                    if w != "zone" {
                        self.last_hurt_t = t;
                    }
                    let by = ev.by.map(|b| b as i64);
                    if let Some(b) = by {
                        if let Some(tr) = self.tracks.get_mut(&b) {
                            tr.hurt_us_t = t;
                        }
                    }
                    self.dmg_log.push_back((t, ev.bearing, by, ev.amount.unwrap_or(0.0), w));
                }
                "hit" => {
                    self.last_dealt_t = t;
                }
                "take_failed" => {
                    if let Some(id) = ev.id.as_ref().and_then(|v| v.as_f64()) {
                        self.blocked_pickups.insert(id as i64, t + 20.0);
                    }
                }
                _ => {}
            }
        }
        while let Some(f) = self.dmg_log.front() {
            if t - f.0 > 12.0 {
                self.dmg_log.pop_front();
            } else {
                break;
            }
        }
        for s in &o.sounds {
            self.sounds.push_back((t - s.age, s.bearing, s.dist.clone(), s.kind.clone()));
        }
        while let Some(f) = self.sounds.front() {
            if t - f.0 > 8.0 {
                self.sounds.pop_front();
            } else {
                break;
            }
        }
        self.blocked_pickups.retain(|_, until| *until > t);
    }

    // ------------------------------------------------------------------ zone field
    fn zone_target(&self, o: &Obs) -> (f64, f64, f64, f64) {
        let z = &o.zone;
        let nr = z.next_radius.max(0.0);
        // target disc: every standable cell inside is a goal (cost 0)
        let r = if nr <= 1.0 {
            4.0
        } else if nr <= 30.0 {
            (nr * 0.5).max(5.0)
        } else if nr <= 80.0 {
            nr * 0.55
        } else {
            nr * 0.6
        };
        (z.next_center[0], z.next_center[1], r, r)
    }

    fn nav_update(&mut self, o: &Obs, deadline: Instant) {
        let w = match self.world {
            Some(w) => w,
            None => return,
        };
        let (cx, cy, r, inner) = self.zone_target(o);
        let tag = (cx.round() as i64, cy.round() as i64, (r * 10.0).round() as i64);
        if self.ff_next.tag != tag && !(self.ff.tag == tag && self.ff_ready) {
            self.ff_next.start_disc(w, tag, cx as f32, cy as f32, r as f32, 40);
        }
        if self.ff_next.tag == tag && !self.ff_next.done {
            self.ff_next.run(w, deadline);
        }
        if self.ff_next.tag == tag && self.ff_next.done && self.ff.tag != tag {
            std::mem::swap(&mut self.ff, &mut self.ff_next);
            self.ff_ready = true;
            self.hold_spot = None;
        }
    }

    /// Path distance (m) from our position to the zone target region, if known.
    fn zone_dist(&self, o: &Obs) -> Option<f64> {
        let w = self.world?;
        if !self.ff_ready {
            return None;
        }
        let p = o.me.pos;
        let c = self.ff.entry(w, p[0] as f32, p[1] as f32, p[2] as f32, 6)?;
        let d = self.ff.dist[c];
        if d.is_finite() {
            Some(d as f64)
        } else {
            None
        }
    }

    /// Seconds until the zone edge (minus margin) passes point p, looking up to 90 s ahead.
    fn time_until_outside(&self, o: &Obs, p: [f64; 3], margin: f64) -> f64 {
        let mut dt = 0.0;
        while dt <= 90.0 {
            let (c, r) = o.zone.at(o.t + dt);
            let d = ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt();
            if d > r - margin {
                return dt;
            }
            dt += 1.0;
        }
        999.0
    }

    fn point_safe(&self, o: &Obs, x: f64, y: f64, horizon: f64, margin: f64) -> bool {
        let (c, r) = o.zone.at(o.t + horizon);
        ((x - c[0]).powi(2) + (y - c[1]).powi(2)).sqrt() < r - margin
    }

    // ------------------------------------------------------------------ main
    pub fn act(&mut self, o: &Obs, deadline: Instant) -> Action {
        self.stats_n += 1;
        self.deadline = deadline;
        self.update_memory(o);
        if !self.ff_ready {
            // get the first field as early as possible
            self.nav_update(o, deadline);
        }
        let me = &o.me;
        let pos = me.pos;
        let mut act = Action { tick: tick_value(o.tick), aim: me.aim, ..Default::default() };

        // upgrades
        if let Some(off) = &o.offer {
            act.pick = Some(self.pick_upgrade(off, me));
        }

        // ---------------- situation
        let enemies: Vec<Track> = self
            .tracks
            .values()
            .filter(|t| !t.is_npc && o.t - t.seen_t < 0.15)
            .cloned()
            .collect();
        let hp_frac = me.hp / me.max_hp.max(1.0);
        let recently_hurt = o.t - self.last_hurt_t < 2.5;
        let zone_d = self.zone_dist(o);
        let t_out = self.time_until_outside(o, pos, 4.0);
        let need_t = zone_d.map(|d| d / self.walk_speed).unwrap_or(0.0);
        let outside_now = !me.in_zone;
        let urgent = outside_now || (zone_d.unwrap_or(0.0) > 1.0 && need_t + 10.0 > t_out);

        // threat selection
        let mut target: Option<Track> = None;
        let mut best_score = f64::MAX;
        for e in &enemies {
            let d = d2(e.pos, pos);
            let mut s = d;
            if o.t - e.hurt_us_t < 3.0 {
                s -= 80.0;
            }
            if e.hp < 400.0 {
                s -= 40.0;
            }
            if s < best_score {
                best_score = s;
                target = Some(e.clone());
            }
        }
        let attacker_bearing = self.dmg_log.iter().rev().find(|d| o.t - d.0 < 1.5 && d.1.is_some()).and_then(|d| d.1);

        // ---------------- decide mode
        let mut mode = Mode::Rotate;
        let mut goal: Option<(f64, f64)> = None;
        let mut speed: f64 = 1.0;
        let mut want_take: Option<u64> = None;
        let mut want_drop: Option<u32> = None;
        let mut fight = false;
        let mut cover_from: Vec<[f64; 3]> = Vec::new();
        if let Some(tg) = &target {
            let d = d2(tg.pos, pos);
            let attacking_us = o.t - tg.hurt_us_t < 4.0;
            let weak = tg.hp < tg.max_hp * 0.35;
            // how many distinct enemies are around (seen recently within 250 m, or hit us lately)?
            let mut threat_ids: Vec<i64> = self
                .tracks
                .values()
                .filter(|t| !t.is_npc && o.t - t.seen_t < 4.0 && d2(t.pos, pos) < 250.0)
                .map(|t| t.id)
                .collect();
            for dl in self.dmg_log.iter() {
                if o.t - dl.0 < 6.0 {
                    if let Some(b) = dl.2 {
                        if !threat_ids.contains(&b) {
                            threat_ids.push(b);
                        }
                    }
                }
            }
            let solo = self.tune.solo_duel && threat_ids.len() <= 1 && !self.tune.aggressive;
            let mut tn = self.tune.clone();
            if solo {
                tn.cover_from_far = false;
                tn.fight_back_range = tn.fight_back_range.max(250.0);
                tn.retreat_hp = tn.retreat_hp.min(0.25);
                tn.hunt_weak_range = tn.hunt_weak_range.max(230.0);
            }
            let healthy = hp_frac > tn.retreat_hp;
            let tn = &tn;
            let our_laser = me.weapons.iter().flatten().any(|w| w.kind == "laser");
            if d < tn.fight_close
                || (attacking_us && healthy && d < tn.fight_back_range)
                || (attacking_us && our_laser && healthy && d < 250.0 && me.hp >= tg.hp * 0.9)
                || (weak && healthy && d < tn.hunt_weak_range)
                || (tn.aggressive && d < 200.0)
                || tn.hunt
            {
                fight = true;
            } else if attacking_us && tn.cover_from_far {
                cover_from.push(tg.pos);
            }
            if fight && !healthy && d > 40.0 && !urgent {
                mode = Mode::Retreat;
            } else if fight {
                mode = Mode::Fight;
            }
        }
        if mode == Mode::Rotate && recently_hurt && hp_frac < 0.5 && !urgent && !self.tune.aggressive {
            mode = Mode::Retreat;
        }
        // shot from somewhere we can't (or shouldn't) fight: break line of sight
        if mode == Mode::Rotate && !urgent && self.tune.cover_from_far {
            if cover_from.is_empty() && recently_hurt {
                if let Some(b) = attacker_bearing {
                    cover_from.push([pos[0] + b.cos() * 120.0, pos[1] + b.sin() * 120.0, pos[2]]);
                }
            }
            if !cover_from.is_empty() {
                mode = Mode::Cover;
                if o.t > self.cover_until || recently_hurt {
                    // (re)choose a spot when starting or when still being hit
                    let moved = self.cover_threats.first().zip(cover_from.first()).map(|(a, b)| d2(*a, *b) > 25.0).unwrap_or(true);
                    if self.cover_spot.is_none() || moved || recently_hurt {
                        self.cover_spot = self.find_cover(o, &cover_from, None);
                    }
                    self.cover_threats = cover_from.clone();
                    self.cover_until = o.t + self.tune.cover_hold_s;
                }
            } else if o.t < self.cover_until && self.tune.cover_from_far {
                mode = Mode::Cover;
                cover_from = self.cover_threats.clone();
            }
        }
        if mode != Mode::Cover && self.cover_spot.is_some() && o.t >= self.cover_until {
            // cover over: re-choose the hold spot (the old one may be exposed to that threat)
            self.cover_spot = None;
            self.hold_spot = None;
        }
        if urgent && mode != Mode::Fight {
            mode = Mode::Rotate;
        }
        if urgent && mode == Mode::Fight && t_out < need_t + 4.0 {
            mode = Mode::Rotate; // run, shooting on the way
        }

        // loot / drops when calm
        let calm = enemies.iter().all(|e| d2(e.pos, pos) > 90.0) && o.t - self.last_hurt_t > 3.0;
        if mode == Mode::Rotate && !urgent && calm {
            if let Some((gx, gy, take, drop, _)) = self.pick_loot(o) {
                mode = Mode::Loot;
                goal = Some((gx, gy));
                want_take = take;
                want_drop = drop;
            } else if let Some((gx, gy)) = self.pick_drop(o, &enemies) {
                mode = Mode::Drop;
                goal = Some((gx, gy));
            }
        }

        // ---------------- movement
        let mut mv = [0.0f64; 2];
        let mut boost = false;
        let mut jump = false;
        match mode {
            Mode::Fight => {
                let tg = target.clone().unwrap();
                let d = d2(tg.pos, pos);
                // strafe perpendicular, keep preferred distance
                if o.t > self.strafe_t {
                    self.strafe_sign = if self.rng.f() < 0.5 { -1.0 } else { 1.0 };
                    self.strafe_t = o.t + self.tune.strafe_min + self.rng.f() * (self.tune.strafe_max - self.tune.strafe_min).max(0.0);
                }
                let to = [(tg.pos[0] - pos[0]) / d.max(0.1), (tg.pos[1] - pos[1]) / d.max(0.1)];
                let perp = [-to[1] * self.strafe_sign, to[0] * self.strafe_sign];
                let pref = self.preferred_range(me);
                let radial = if d < pref - 10.0 {
                    -0.6
                } else if d > pref + 20.0 {
                    if self.tune.aggressive { 1.5 } else { 0.5 }
                } else {
                    0.0
                };
                let mut dir = [perp[0] + to[0] * radial, perp[1] + to[1] * radial];
                // keep inside the zone
                if !self.point_safe(o, pos[0] + dir[0] * 15.0, pos[1] + dir[1] * 15.0, 5.0, 5.0) {
                    if let Some(g) = self.zone_dir(o) {
                        dir = [dir[0] * 0.3 + g[0], dir[1] * 0.3 + g[1]];
                    }
                }
                let dir = self.walkable_dir(pos, dir);
                mv = dir;
                speed = 1.0;
                let has_lightning = me.weapons.iter().flatten().any(|w| w.kind == "lightning");
                if self.tune.boost_strafe && d > 20.0 && me.grounded && me.energy > if has_lightning { 70.0 } else { 35.0 } {
                    boost = true;
                }
                if (self.tune.aggressive || self.tune.hunt) && d > pref + 40.0 {
                    let (m, b, j) = self.go_to(o, tg.pos[0], tg.pos[1]);
                    mv = m;
                    boost = b;
                    jump = j;
                }
                let (_, rnow) = o.zone.at(o.t);
                if rnow < 40.0 || o.zone.next_radius <= 1.0 {
                    let (cf, _) = o.zone.at(o.t + 1.5);
                    let dd = ((cf[0] - pos[0]).powi(2) + (cf[1] - pos[1]).powi(2)).sqrt();
                    if dd > rnow.min(25.0) * 0.35 {
                        let (m, b, j) = self.go_to_prec(o, cf[0], cf[1], 0.3);
                        mv = m;
                        boost = b;
                        jump = j;
                    }
                }
            }
            Mode::Cover => {
                let spot = self.cover_spot.or_else(|| self.find_cover(o, &cover_from, None));
                if let Some((gx, gy)) = spot {
                    let (m, b, j) = self.go_to(o, gx, gy);
                    mv = m;
                    boost = b;
                    jump = j;
                    if self.tune.zigzag {
                        mv = self.zigzag(o, mv);
                    }
                }
            }
            Mode::Retreat => {
                let threat_pos: Vec<[f64; 3]> = enemies.iter().map(|e| e.pos).collect();
                let tb = attacker_bearing;
                if let Some((gx, gy)) = self.find_cover(o, &threat_pos, tb) {
                    let dd = ((gx - pos[0]).powi(2) + (gy - pos[1]).powi(2)).sqrt();
                    if dd <= self.tune.retreat_cover_max {
                        goal = Some((gx, gy));
                    }
                }
                if let Some(g) = goal {
                    let (m, b, j) = self.go_to(o, g.0, g.1);
                    mv = m;
                    boost = b || me.energy > 40.0;
                    jump = j;
                    if self.tune.zigzag {
                        mv = self.zigzag(o, mv);
                    }
                } else if let Some(tg) = target.clone() {
                    // no cover close enough: keep fighting (strafe) instead of running in the open
                    mode = Mode::Fight;
                    let d = d2(tg.pos, pos).max(0.1);
                    if o.t > self.strafe_t {
                        self.strafe_sign = if self.rng.f() < 0.5 { -1.0 } else { 1.0 };
                        self.strafe_t = o.t + self.tune.strafe_min + self.rng.f() * (self.tune.strafe_max - self.tune.strafe_min).max(0.0);
                    }
                    let to = [(tg.pos[0] - pos[0]) / d, (tg.pos[1] - pos[1]) / d];
                    mv = self.walkable_dir(pos, [-to[1] * self.strafe_sign, to[0] * self.strafe_sign]);
                } else if let Some(g) = self.zone_dir(o) {
                    mv = g;
                }
            }
            Mode::Loot | Mode::Drop => {
                let g = goal.unwrap();
                let (m, b, j) = self.go_to(o, g.0, g.1);
                mv = m;
                boost = b;
                jump = j;
                let dd = ((g.0 - pos[0]).powi(2) + (g.1 - pos[1]).powi(2)).sqrt();
                if mode == Mode::Drop && dd < 3.0 {
                    mv = [0.0, 0.0];
                }
                if mode == Mode::Loot && dd < 1.0 {
                    mv = [0.0, 0.0];
                }
            }
            _ => {
                let final_phase = o.zone.next_radius <= 1.0;
                if final_phase {
                    // the last circle closes to a point: only the moving center stays safe
                    let (cf, _) = o.zone.at(o.t + 1.5);
                    let dd = ((cf[0] - pos[0]).powi(2) + (cf[1] - pos[1]).powi(2)).sqrt();
                    if dd > 0.4 {
                        let (m, b, j) = self.go_to_prec(o, cf[0], cf[1], 0.3);
                        mv = m;
                        boost = b || (dd > 8.0 && (outside_now || t_out < dd / self.walk_speed + 3.0) && me.energy > 15.0);
                        jump = j;
                        speed = (dd / 3.0).clamp(0.3, 1.0);
                    }
                    mode = if dd < 5.0 { Mode::Hold } else { Mode::Rotate };
                } else {
                    // rotate to / hold in the zone
                    let (m, b, j, at_target) = self.follow_zone(o);
                    mv = m;
                    boost = b || (urgent && (outside_now || t_out < need_t + 3.0) && me.energy > 15.0);
                    jump = j;
                    if at_target {
                        mode = Mode::Hold;
                        // walk to a hold spot (reachable on foot) if we have one
                        if let Some((hx, hy)) = self.choose_hold_spot(o) {
                            let dd = ((hx - pos[0]).powi(2) + (hy - pos[1]).powi(2)).sqrt();
                            if dd > 1.5 {
                                let (m2, b2, j2) = self.go_to(o, hx, hy);
                                mv = m2;
                                boost = b2;
                                jump = j2;
                                speed = if dd > 25.0 { 1.0 } else { 0.42 };
                            } else {
                                mv = [0.0, 0.0];
                            }
                        } else {
                            mv = [0.0, 0.0];
                        }
                    }
                }
            }
        }
        if want_take.is_some() {
            act.take = want_take;
            act.drop = want_drop;
        }
        // silent movement when enemies are close and we're not in a hurry or fighting
        let enemy_near = enemies.iter().any(|e| d2(e.pos, pos) < 80.0)
            || self.sounds.iter().any(|s| o.t - s.0 < 4.0 && s.2 == "near" && (s.3 == "step" || s.3 == "boost"));
        if enemy_near && !urgent && (mode == Mode::Hold || mode == Mode::Loot) {
            speed = speed.min(0.4);
        }

        // ---------------- dodge incoming mortar shells and bullets
        if let Some(d) = if self.tune.dodge { self.dodge_dir(o) } else { None } {
            let urgent_move = mode == Mode::Rotate && urgent;
            if !urgent_move {
                mv = self.walkable_dir(pos, d);
                speed = 1.0;
                if me.grounded && me.energy > 25.0 {
                    boost = true;
                }
            }
        }

        // ---------------- stuck detection / recovery
        let cmd_speed = (mv[0] * mv[0] + mv[1] * mv[1]).sqrt() * speed;
        let act_speed = (me.vel[0] * me.vel[0] + me.vel[1] * me.vel[1]).sqrt();
        let cmd_speed = if mode == Mode::Fight { 0.0 } else { cmd_speed };
        self.pos_hist.push_back((o.t, pos, cmd_speed, act_speed));
        while let Some(f) = self.pos_hist.front() {
            if o.t - f.0 > 1.6 {
                self.pos_hist.pop_front();
            } else {
                break;
            }
        }
        if o.t < self.stuck_until {
            mode = Mode::Unstuck;
            mv = self.stuck_dir;
            speed = 1.0;
            jump = me.grounded;
            boost = !me.grounded && me.energy > 5.0;
        } else if self.is_stuck(o) {
            self.stuck_count += 1;
            if o.t - self.last_stuck_t > 8.0 {
                self.stuck_count = 1;
            }
            self.last_stuck_t = o.t;
            if mode == Mode::Hold {
                if let (Some(w), Some((_, tag))) = (self.world, self.hold_spot) {
                    if let Some(c) = w.nearest_standable(pos[0] as f32, pos[1] as f32, pos[2] as f32, 2) {
                        self.hold_spot = Some((c, tag));
                    }
                }
            }
            let base = if (mv[0] * mv[0] + mv[1] * mv[1]) > 0.01 { mv } else { self.last_move };
            let ang = base[1].atan2(base[0]);
            let off = match self.stuck_count % 4 {
                1 => 0.0,
                2 => PI / 2.0,
                3 => -PI / 2.0,
                _ => PI,
            };
            let a = ang + off + (self.rng.f() - 0.5) * 0.6;
            self.stuck_dir = [a.cos(), a.sin()];
            self.stuck_until = o.t + 0.9;
            self.pos_hist.clear();
            mode = Mode::Unstuck;
            mv = self.stuck_dir;
            speed = 1.0;
            jump = me.grounded;
            eprintln!("[xero] t={:.1} stuck #{} at {:.1},{:.1},{:.1}", o.t, self.stuck_count, pos[0], pos[1], pos[2]);
        }

        let n = (mv[0] * mv[0] + mv[1] * mv[1]).sqrt();
        if n > 1e-6 {
            act.r#move = [mv[0] / n * speed, mv[1] / n * speed];
            self.last_move = act.r#move;
        } else {
            act.r#move = [0.0, 0.0];
        }
        act.boost = boost;
        act.jump = jump;

        // ---------------- aim & fire
        self.aim_and_fire(o, &mut act, target.as_ref(), mode, attacker_bearing);
        self.mode = mode;
        self.label = format!("{:?} hp{:.0} z{:.0}", mode, me.hp, zone_d.unwrap_or(-1.0));
        // background work (flow field for the next circle) with whatever time is left
        self.nav_update(o, deadline);
        act
    }

    /// Add a sideways weave to a travel direction so leading shooters miss.
    fn zigzag(&mut self, o: &Obs, mv: [f64; 2]) -> [f64; 2] {
        if o.t > self.strafe_t {
            self.strafe_sign = -self.strafe_sign;
            self.strafe_t = o.t + 0.5 + self.rng.f() * 0.6;
        }
        let n = (mv[0] * mv[0] + mv[1] * mv[1]).sqrt();
        if n < 1e-6 {
            return mv;
        }
        let f = [mv[0] / n, mv[1] / n];
        let side = [-f[1] * self.strafe_sign, f[0] * self.strafe_sign];
        self.walkable_dir(o.me.pos, [f[0] + side[0] * 0.8, f[1] + side[1] * 0.8])
    }

    fn preferred_range(&self, me: &SelfObs) -> f64 {
        let has = |k: &str| me.weapons.iter().flatten().any(|w| w.kind == k);
        if has("lightning") {
            30.0
        } else if has("laser") {
            90.0
        } else if has("mortar") {
            80.0
        } else {
            60.0
        }
    }

    fn is_stuck(&self, o: &Obs) -> bool {
        if self.pos_hist.len() < 10 {
            return false;
        }
        let f = self.pos_hist.front().unwrap();
        let span = o.t - f.0;
        if span < 1.2 {
            return false;
        }
        let avg_cmd: f64 = self.pos_hist.iter().map(|h| h.2).sum::<f64>() / self.pos_hist.len() as f64;
        let min_cmd = self.pos_hist.iter().map(|h| h.2).fold(f64::MAX, f64::min);
        if avg_cmd < 0.25 || min_cmd < 0.15 {
            return false;
        }
        let expected = avg_cmd * self.walk_speed * span;
        let moved = d2(f.1, o.me.pos);
        let dz = (f.1[2] - o.me.pos[2]).abs();
        let avg_speed: f64 = self.pos_hist.iter().map(|h| h.3).sum::<f64>() / self.pos_hist.len() as f64;
        // either not moving at all, or going nowhere (oscillating) at a fraction of the commanded speed
        let slow = avg_speed < 0.2 * avg_cmd * self.walk_speed;
        moved < (0.18 * expected).max(0.8) && dz < 1.5 && (slow || moved < 0.5)
    }

    // ------------------------------------------------------------------ movement helpers
    /// Direction from the zone flow field (None if not ready).
    fn zone_dir(&mut self, o: &Obs) -> Option<[f64; 2]> {
        let (m, _, _, _) = self.follow_zone(o);
        if m[0] == 0.0 && m[1] == 0.0 {
            None
        } else {
            Some(m)
        }
    }

    /// Follow the zone flow field. Returns (move, boost, jump, at_target).
    fn follow_zone(&mut self, o: &Obs) -> ([f64; 2], bool, bool, bool) {
        let pos = o.me.pos;
        let (cx, cy, r0, _) = self.zone_target(o);
        let r = if self.ff_ready && self.ff.tag.0 == cx.round() as i64 && self.ff.tag.1 == cy.round() as i64 {
            (self.ff.radius as f64).max(r0)
        } else {
            r0
        };
        let direct = {
            let dx = cx - pos[0];
            let dy = cy - pos[1];
            let d = (dx * dx + dy * dy).sqrt();
            if d < r * 0.9 {
                return ([0.0, 0.0], false, false, true);
            }
            [dx / d.max(1e-6), dy / d.max(1e-6)]
        };
        let w = match self.world {
            Some(w) => w,
            None => return (direct, false, false, false),
        };
        if !self.ff_ready {
            return (direct, false, false, false);
        }
        let entry = match self.ff.entry(w, pos[0] as f32, pos[1] as f32, pos[2] as f32, 8) {
            Some(c) => c,
            None => return (direct, false, false, false),
        };
        if self.ff.dist[entry] <= 0.0 {
            return ([0.0, 0.0], false, false, true);
        }
        let mut chain = std::mem::take(&mut self.path);
        self.ff.chain(entry, 40, &mut chain);
        let r = self.follow_path(o, &chain);
        self.path = chain;
        (r.0, r.1, r.2, false)
    }

    /// Steer along a cell path starting near our position. Returns (move, boost, jump).
    fn follow_path(&mut self, o: &Obs, path: &[usize]) -> ([f64; 2], bool, bool) {
        let w = self.world.unwrap();
        let me = &o.me;
        let pos = me.pos;
        if path.is_empty() {
            return ([0.0, 0.0], false, false);
        }
        let step = w.p.step_up;
        // find the first jump edge in the path
        let mut jump_at = path.len();
        for i in 0..path.len() - 1 {
            let a = path[i];
            let b = path[i + 1];
            let (ax, ay) = w.cell_xy(a);
            let (bx, by) = w.cell_xy(b);
            let l = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
            if l > 1.5 || (w.floor[b] - w.floor[a]).abs() > step {
                jump_at = i;
                break;
            }
        }
        // in the middle of a climb: keep going until feet clear the target surface
        if let Some((tc, t0)) = self.climbing {
            let (tx, ty) = w.cell_xy(tc);
            let tz = w.floor[tc] as f64;
            let dx = tx as f64 - pos[0];
            let dy = ty as f64 - pos[1];
            let d = (dx * dx + dy * dy).sqrt();
            let done = (me.grounded && (pos[2] - tz).abs() < 1.0) || o.t - t0 > 6.0 || (me.grounded && o.t - t0 > 1.0);
            if !done {
                let need_up = pos[2] < tz + 0.8;
                let mv = if need_up && d < 1.0 { [0.0, 0.0] } else { [dx / d.max(1e-6), dy / d.max(1e-6)] };
                return (mv, !me.grounded && need_up && me.energy > 0.5, me.grounded && need_up);
            }
            self.climbing = None;
        }
        if jump_at == 0 {
            let a = path[0];
            let b = path[1];
            let (bx, by) = w.cell_xy(b);
            let dz = w.floor[b] - w.floor[a];
            let dx = bx as f64 - pos[0];
            let dy = by as f64 - pos[1];
            let d = (dx * dx + dy * dy).sqrt().max(1e-6);
            let dir = [dx / d, dy / d];
            if dz > step {
                let need_e = dz as f64 * 1.6 + 8.0;
                if me.energy < need_e.min(o.me.max_energy * 0.95) && me.in_zone {
                    // wait for energy (stand still close to the takeoff point)
                    return ([0.0, 0.0], false, false);
                }
                self.climbing = Some((b, o.t));
                return (dir, false, me.grounded);
            }
            // drop: just walk off
            return (dir, false, false);
        }
        // string pulling: farthest walkable point before the jump edge
        let lim = jump_at.min(path.len() - 1).min(30);
        let mut target = path[lim.min(1)];
        let mut k = lim;
        while k >= 1 {
            let (x, y) = w.cell_xy(path[k]);
            if w.walkable_line((pos[0] as f32, pos[1] as f32), (x, y)) {
                target = path[k];
                break;
            }
            k -= 1;
        }
        let (tx, ty) = w.cell_xy(target);
        let dx = tx as f64 - pos[0];
        let dy = ty as f64 - pos[1];
        let d = (dx * dx + dy * dy).sqrt().max(1e-6);
        ([dx / d, dy / d], false, false)
    }

    /// Navigate to a nearby point using straight lines or a bounded A*. Returns (move, boost, jump).
    fn go_to(&mut self, o: &Obs, gx: f64, gy: f64) -> ([f64; 2], bool, bool) {
        self.go_to_prec(o, gx, gy, 1.0)
    }

    fn go_to_prec(&mut self, o: &Obs, gx: f64, gy: f64, prec: f64) -> ([f64; 2], bool, bool) {
        let pos = o.me.pos;
        let dx = gx - pos[0];
        let dy = gy - pos[1];
        let d = (dx * dx + dy * dy).sqrt();
        let direct = [dx / d.max(1e-6), dy / d.max(1e-6)];
        let w = match self.world {
            Some(w) => w,
            None => return (direct, false, false),
        };
        if d < prec {
            return ([0.0, 0.0], false, false);
        }
        let tz = w.cell_of(gx as f32, gy as f32).map(|c| w.floor[c] as f64).unwrap_or(pos[2]);
        let same_level = (tz - pos[2]).abs() < w.p.step_up as f64;
        if d < 3.0 && self.climbing.is_none() && same_level {
            return (direct, false, false);
        }
        if self.climbing.is_none() && same_level && w.walkable_line((pos[0] as f32, pos[1] as f32), (gx as f32, gy as f32)) {
            return (direct, false, false);
        }
        let replan = match self.local_goal {
            Some((lx, ly)) => (lx - gx).abs() + (ly - gy).abs() > 2.0 || o.t - self.local_path_t > 1.5,
            None => true,
        };
        if replan {
            self.local_goal = Some((gx, gy));
            self.local_path_t = o.t;
            self.local_path.clear();
            let s = match w.cell_of(pos[0] as f32, pos[1] as f32) {
                Some(c) if w.standable(c) && (w.floor[c] as f64 - pos[2]).abs() < 3.0 => Some(c),
                _ => w.nearest_standable(pos[0] as f32, pos[1] as f32, pos[2] as f32, 6),
            };
            if let Some(s) = s {
                if let Some(p) = self.astar.search(w, s, gx as f32, gy as f32, 1.0, 12_000) {
                    self.local_path = p;
                }
            }
        }
        if self.local_path.is_empty() {
            return (direct, false, false);
        }
        // trim the path to start from the cell nearest to us
        let mut best_i = 0;
        let mut bd = f64::MAX;
        for (i, &c) in self.local_path.iter().enumerate().take(60) {
            let (x, y) = w.cell_xy(c);
            let dd = (x as f64 - pos[0]).powi(2) + (y as f64 - pos[1]).powi(2);
            if dd < bd {
                bd = dd;
                best_i = i;
            }
        }
        let p: Vec<usize> = self.local_path[best_i..].to_vec();
        if p.len() < 2 {
            return (direct, false, false);
        }
        self.follow_path(o, &p)
    }

    /// Direction to step to avoid a predicted mortar impact or an incoming bullet, if any.
    fn dodge_dir(&mut self, o: &Obs) -> Option<[f64; 2]> {
        let me = &o.me;
        let pos = me.pos;
        let w = self.world;
        let g = self.cfg.w("mortar", "gravity", 22.0);
        let splash = self.cfg.w("mortar", "splash_radius", 7.0) + 3.5;
        let mut push = [0.0f64, 0.0];
        let mut any = false;
        for p in &o.projectiles {
            if p.kind == "mortar" {
                // integrate the shell until it hits the ground/roof (or 6 s)
                let mut q = p.pos;
                let mut v = p.vel;
                let dt = 0.05;
                let mut impact = None;
                for _ in 0..120 {
                    q = [q[0] + v[0] * dt, q[1] + v[1] * dt, q[2] + v[2] * dt];
                    v[2] -= g * dt;
                    let ground = match w {
                        Some(w) => w.support(q[0] as f32, q[1] as f32, q[2] as f32, 0.0) as f64,
                        None => pos[2],
                    };
                    let hits_mech = ((q[0] - pos[0]).powi(2) + (q[1] - pos[1]).powi(2)).sqrt() < 2.8 && q[2] > pos[2] && q[2] < pos[2] + 9.0;
                    if q[2] <= ground || hits_mech {
                        impact = Some(q);
                        break;
                    }
                }
                if let Some(ip) = impact {
                    let dx = pos[0] - ip[0];
                    let dy = pos[1] - ip[1];
                    let d = (dx * dx + dy * dy).sqrt();
                    if d < splash {
                        let k = (splash - d) / splash + 0.2;
                        if d < 0.5 {
                            // dead center: step sideways to the shell's flight direction
                            let n = (p.vel[0] * p.vel[0] + p.vel[1] * p.vel[1]).sqrt().max(1e-6);
                            push[0] += -p.vel[1] / n * k;
                            push[1] += p.vel[0] / n * k;
                        } else {
                            push[0] += dx / d * k;
                            push[1] += dy / d * k;
                        }
                        any = true;
                    }
                }
            } else {
                // straight bullet: will it pass within our radius soon?
                let rel = [pos[0] - p.pos[0], pos[1] - p.pos[1], pos[2] + 4.5 - p.pos[2]];
                let vv = p.vel[0] * p.vel[0] + p.vel[1] * p.vel[1] + p.vel[2] * p.vel[2];
                if vv < 1.0 {
                    continue;
                }
                let tc = (rel[0] * p.vel[0] + rel[1] * p.vel[1] + rel[2] * p.vel[2]) / vv;
                if !(0.12..=0.8).contains(&tc) {
                    continue; // too late to react, or receding / far
                }
                let cx = p.pos[0] + p.vel[0] * tc - pos[0];
                let cy = p.pos[1] + p.vel[1] * tc - pos[1];
                let miss = (cx * cx + cy * cy).sqrt();
                if miss < 3.0 {
                    let n = (p.vel[0] * p.vel[0] + p.vel[1] * p.vel[1]).sqrt().max(1e-6);
                    let mut side = [-p.vel[1] / n, p.vel[0] / n];
                    // step to the side that increases the miss distance
                    if side[0] * cx + side[1] * cy > 0.0 {
                        side = [-side[0], -side[1]];
                    }
                    push[0] += side[0] * 0.5;
                    push[1] += side[1] * 0.5;
                    any = true;
                }
            }
        }
        if !any {
            return None;
        }
        let n = (push[0] * push[0] + push[1] * push[1]).sqrt();
        if n < 1e-6 {
            return None;
        }
        Some([push[0] / n, push[1] / n])
    }

    /// Bend a desired direction so a short step along it stays walkable.
    fn walkable_dir(&self, pos: [f64; 3], dir: [f64; 2]) -> [f64; 2] {
        let w = match self.world {
            Some(w) => w,
            None => return dir,
        };
        let n = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
        if n < 1e-6 {
            return dir;
        }
        let base = dir[1].atan2(dir[0]);
        for k in [0.0, 0.4, -0.4, 0.8, -0.8, 1.2, -1.2, 1.6, -1.6, 2.2, -2.2, PI] {
            let a = base + k;
            let ex = pos[0] + a.cos() * 6.0;
            let ey = pos[1] + a.sin() * 6.0;
            if w.walkable_line((pos[0] as f32, pos[1] as f32), (ex as f32, ey as f32)) {
                return [a.cos(), a.sin()];
            }
        }
        dir
    }

    // ------------------------------------------------------------------ choices
    fn pick_upgrade(&self, off: &Offer, me: &SelfObs) -> u32 {
        let has = |k: &str| me.weapons.iter().flatten().any(|w| w.kind == k);
        let mut best = 0usize;
        let mut bs = i32::MIN;
        for (i, ch) in off.choices.iter().enumerate() {
            let s = match ch.id.as_str() {
                "plating" => 100,
                "armor" => 90,
                "nanites" => 70,
                "lightning_efficiency" if has("lightning") => 60,
                "laser_charge" if has("laser") => 58,
                "laser_damage" if has("laser") => 56,
                "lightning_range" if has("lightning") => 55,
                "gun_rate" => 50,
                "gun_damage" => 48,
                "sensors" => 45,
                "mortar_reload" if has("mortar") => 44,
                "gyros" => 40,
                "servos" => 38,
                "gun_mag" => 36,
                "capacitor" => 34,
                "jets" => 30,
                "reactor" => 28,
                "gun_velocity" => 26,
                "lightning_chain" => 20,
                "mortar_splash" => 18,
                "mortar_velocity" => 16,
                "laser_pierce" => 10,
                _ => 0,
            };
            if s > bs {
                bs = s;
                best = i;
            }
        }
        best as u32
    }

    fn weapon_rank(kind: &str) -> i32 {
        match kind {
            "laser" => 3,
            "lightning" => 2,
            "mortar" => 1,
            _ => 0,
        }
    }

    /// Loot target: (x, y, take id).
    fn pick_loot(&mut self, o: &Obs) -> Option<(f64, f64, Option<u64>, Option<u32>, i64)> {
        let me = &o.me;
        let pos = me.pos;
        let held: Vec<String> = me.weapons.iter().flatten().map(|w| w.kind.clone()).collect();
        let slot1_empty = me.weapons.get(1).map(|w| w.is_none()).unwrap_or(true);
        let mut best: Option<(f64, f64, f64, Option<u64>, Option<u32>, i64)> = None;
        for p in &o.pickups {
            if self.blocked_pickups.contains_key(&(p.id as i64)) {
                continue;
            }
            if held.iter().any(|h| *h == p.kind) {
                continue;
            }
            let rank = Self::weapon_rank(&p.kind);
            if !slot1_empty {
                // only swap slot 1 for a better weapon
                let cur = me.weapons.get(1).and_then(|w| w.as_ref()).map(|w| Self::weapon_rank(&w.kind)).unwrap_or(0);
                if rank <= cur {
                    continue;
                }
            }
            let d = d2(p.pos, pos);
            let dz = (p.pos[2] - pos[2]).abs();
            let lr = if slot1_empty { self.tune.loot_radius } else { 70.0 };
            if d > lr || dz > 8.0 {
                continue;
            }
            if !self.point_safe(o, p.pos[0], p.pos[1], 15.0, 8.0) {
                continue;
            }
            let score = d - rank as f64 * 15.0;
            let take = if d < self.pickup_r - 0.3 && dz < 3.5 { Some(p.id as u64) } else { None };
            let drop = if slot1_empty { None } else { Some(1u32) };
            if best.as_ref().map(|b| score < b.2).unwrap_or(true) {
                best = Some((p.pos[0], p.pos[1], score, take, drop, p.id as i64));
            }
        }
        for c in &o.cubes {
            if self.blocked_pickups.contains_key(&(c.id as i64)) {
                continue;
            }
            let d = d2(c.pos, pos);
            let dz = (c.pos[2] - pos[2]).abs();
            if d > 60.0 || dz > 6.0 {
                continue;
            }
            if !self.point_safe(o, c.pos[0], c.pos[1], 15.0, 8.0) {
                continue;
            }
            let score = d - 10.0;
            if best.as_ref().map(|b| score < b.2).unwrap_or(true) {
                best = Some((c.pos[0], c.pos[1], score, None, None, c.id as i64));
            }
        }
        let b = best?;
        // give up on a target that takes too long (unreachable, contested, or not takeable)
        match self.loot_target {
            Some((id, t0)) if id == b.5 => {
                if o.t - t0 > 14.0 {
                    self.blocked_pickups.insert(id, o.t + 60.0);
                    self.loot_target = None;
                    return None;
                }
            }
            _ => self.loot_target = Some((b.5, o.t)),
        }
        Some((b.0, b.1, b.3, if b.3.is_some() { b.4 } else { None }, b.5))
    }

    fn pick_drop(&self, o: &Obs, enemies: &[Track]) -> Option<(f64, f64)> {
        let pos = o.me.pos;
        for d in &o.drops {
            if d.lands_in > 6.0 {
                continue;
            }
            if let Some(h) = d.holder {
                if h as i64 != o.me.id as i64 {
                    continue;
                }
            }
            let dist = d2(d.pos, pos);
            if dist > 90.0 {
                continue;
            }
            if enemies.iter().any(|e| d2(e.pos, d.pos) < 110.0) {
                continue;
            }
            if !self.point_safe(o, d.pos[0], d.pos[1], 10.0, 6.0) {
                continue;
            }
            return Some((d.pos[0], d.pos[1]));
        }
        None
    }

    /// A hold spot inside the zone target region, reachable on foot: next to a wall (cover on
    /// one side) and under canopy.
    fn choose_hold_spot(&mut self, o: &Obs) -> Option<(f64, f64)> {
        let w = self.world?;
        if let Some((c, tag)) = self.hold_spot {
            if tag == self.ff.tag {
                let (x, y) = w.cell_xy(c);
                return Some((x as f64, y as f64));
            }
        }
        if !self.ff_ready || Instant::now() + std::time::Duration::from_millis(9) > self.deadline {
            return None; // not now: stand still this decision, search next time
        }
        let pos = o.me.pos;
        let (cx, cy, r, _) = self.zone_target(o);
        let r = r.max(self.ff.radius as f64);
        let start = match w.cell_of(pos[0] as f32, pos[1] as f32) {
            Some(c) if w.standable(c) && (w.floor[c] as f64 - pos[2]).abs() < 2.0 => c,
            _ => w.nearest_standable(pos[0] as f32, pos[1] as f32, pos[2] as f32, 4)?,
        };
        // BFS over walkable 8-neighbour edges within 45 m
        let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let mut q = VecDeque::new();
        seen.insert(start);
        q.push_back(start);
        let (sx, sy) = w.cell_xy(start);
        let mut cands: Vec<(f64, usize)> = Vec::new();
        while let Some(u) = q.pop_front() {
            let (x, y) = w.cell_xy(u);
            let dme = (((x - sx).powi(2) + (y - sy).powi(2)) as f64).sqrt();
            let dc = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
            if dc <= r * 0.9 {
                let wall = match w.wdist[u] {
                    1 => 4.0,
                    2 | 3 => 8.0,
                    4 => 4.0,
                    _ => 0.0,
                };
                let cover = w.cover[u] as f64 / 255.0 * 12.0;
                let sc = wall + cover - dme * 0.15 - (dc / r.max(1.0)) * 4.0;
                cands.push((sc, u));
            }
            if seen.len() > 8000 {
                continue;
            }
            let ui = (u % w.nx) as i32;
            let uj = (u / w.nx) as i32;
            for &(di, dj, len) in crate::world::NB8.iter() {
                let vi = ui + di;
                let vj = uj + dj;
                if vi < 0 || vj < 0 || vi >= w.nx as i32 || vj >= w.ny as i32 {
                    continue;
                }
                let v = vj as usize * w.nx + vi as usize;
                if seen.contains(&v) {
                    continue;
                }
                let (vx, vy) = w.cell_xy(v);
                if ((vx - sx).powi(2) + (vy - sy).powi(2)) > 45.0 * 45.0 {
                    continue;
                }
                if w.walk_cost(u, v, len).is_some() {
                    seen.insert(v);
                    q.push_back(v);
                }
            }
        }
        if cands.is_empty() {
            self.hold_spot = Some((start, self.ff.tag));
            let (x, y) = w.cell_xy(start);
            return Some((x as f64, y as f64));
        }
        // evaluate concealment for the most promising candidates plus a spread sample
        cands.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        let mut eval: Vec<(f64, usize)> = cands.iter().take(60).cloned().collect();
        let stride = (cands.len() / 40).max(1);
        eval.extend(cands.iter().step_by(stride).take(40).cloned());
        let mut best = eval[0].1;
        let mut bs = f64::MIN;
        for &(sc, c) in &eval {
            let hidden = self.concealment(w, c);
            let total = sc + hidden * 3.0;
            if total > bs {
                bs = total;
                best = c;
            }
        }
        self.hold_spot = Some((best, self.ff.tag));
        let (x, y) = w.cell_xy(best);
        Some((x as f64, y as f64))
    }

    /// How many of 16 far viewpoints (8 directions x 70/120 m, eye 8 m above ground) would NOT
    /// see a mech standing on cell c.
    fn concealment(&self, w: &World, c: usize) -> f64 {
        let (x, y) = w.cell_xy(c);
        let feet = [x, y, w.floor[c]];
        let mut hidden = 0.0;
        for k in 0..8 {
            let a = k as f32 * std::f32::consts::PI / 4.0 + 0.2;
            for &dist in &[70.0f32, 120.0] {
                let ex = x + a.cos() * dist;
                let ey = y + a.sin() * dist;
                let ez = w.ground(ex, ey) + 8.0;
                if !w.mech_visible([ex, ey, ez], feet, self.mech_h as f32, 0.0) {
                    hidden += 1.0;
                }
            }
        }
        hidden
    }

    /// Cover from threats: a nearby standable spot with no line of sight from any threat.
    fn find_cover(&self, o: &Obs, threats: &[[f64; 3]], bearing: Option<f64>) -> Option<(f64, f64)> {
        let w = self.world?;
        let pos = o.me.pos;
        let mut tps: Vec<[f32; 3]> = threats.iter().map(|t| [t[0] as f32, t[1] as f32, (t[2] + self.eye_h) as f32]).collect();
        if tps.is_empty() {
            if let Some(b) = bearing {
                tps.push([(pos[0] + b.cos() * 60.0) as f32, (pos[1] + b.sin() * 60.0) as f32, (pos[2] + self.eye_h) as f32]);
            } else {
                return None;
            }
        }
        let mut best = None;
        let mut bs = f64::MAX;
        for k in 0..24 {
            let a = k as f64 * PI / 12.0;
            for &r in &[12.0, 22.0, 34.0] {
                let x = pos[0] + a.cos() * r;
                let y = pos[1] + a.sin() * r;
                let c = match w.cell_of(x as f32, y as f32) {
                    Some(c) => c,
                    None => continue,
                };
                if !w.standable(c) || (w.floor[c] as f64 - pos[2]).abs() > 2.5 {
                    continue;
                }
                if !self.point_safe(o, x, y, 10.0, 5.0) {
                    continue;
                }
                let z = w.floor[c] + 5.0;
                let hidden = tps.iter().filter(|t| !w.los(**t, [x as f32, y as f32, z])).count();
                let mut s = r + (tps.len() - hidden) as f64 * 100.0;
                // away from threats is better
                for t in &tps {
                    let dt = ((t[0] as f64 - x).powi(2) + (t[1] as f64 - y).powi(2)).sqrt();
                    s -= dt.min(80.0) * 0.3;
                }
                if !w.walkable_line((pos[0] as f32, pos[1] as f32), (x as f32, y as f32)) {
                    s += 25.0;
                }
                if s < bs {
                    bs = s;
                    best = Some((x, y));
                }
            }
        }
        best
    }

    // ------------------------------------------------------------------ combat
    fn aim_and_fire(&mut self, o: &Obs, act: &mut Action, target: Option<&Track>, mode: Mode, bearing: Option<f64>) {
        let me = &o.me;
        let pos = me.pos;
        let muzzle = [pos[0], pos[1], pos[2] + me.stat("muzzle_height", self.muzzle_h)];
        let mut fire = [false, false];
        let mut aim = act.aim;
        let w = self.world;
        let has_los = |a: [f64; 3], b: [f64; 3]| -> bool { w.map(|w| w.los(f3(a), f3(b))).unwrap_or(true) };

        let mut engaged = false;
        if let Some(tg) = target {
            let d = d2(tg.pos, pos);
            let center = [tg.pos[0], tg.pos[1], tg.pos[2] + 4.5];
            let los = has_los(muzzle, center) || has_los(muzzle, [tg.pos[0], tg.pos[1], tg.pos[2] + 7.5]);
            let shoot_ok = mode == Mode::Fight || mode == Mode::Retreat || mode == Mode::Cover || (o.t - tg.hurt_us_t < 4.0) || d < 50.0 || self.tune.aggressive;
            if shoot_ok {
                engaged = true;
                // choose weapon per slot
                let mut desired: Option<[f64; 2]> = None;
                for (si, wo) in me.weapons.iter().enumerate().take(2) {
                    let wo = match wo {
                        Some(w) => w,
                        None => continue,
                    };
                    match wo.kind.as_str() {
                        "gun" => {
                            let speed = wo.stat("speed", 252.0);
                            let range = wo.stat("range", 230.0);
                            let a = lead_aim(muzzle, center, tg.vel, speed);
                            if desired.is_none() {
                                desired = Some(a);
                            }
                            let err = ang_err(me.aim, a);
                            let tol = (2.0f64 / d.max(1.0)).atan() + 0.01;
                            if los && d < range * 0.98 && err < tol && wo.cooldown <= 0.0 && wo.ammo.unwrap_or(1.0) > 0.0 {
                                fire[si] = true;
                            }
                        }
                        "lightning" => {
                            let range = wo.stat("range", 45.0);
                            let cone = wo.stat("cone_deg", 30.0).to_radians();
                            let yaw = (center[1] - muzzle[1]).atan2(center[0] - muzzle[0]);
                            if d < range + 8.0 && desired.is_none() {
                                desired = Some([yaw, (center[2] - muzzle[2]).atan2(d.max(0.1))]);
                            }
                            let dy = wrap(yaw - me.aim[0]).abs();
                            if los && d < range - 1.0 && dy < cone * 0.5 * 0.85 && wo.cooldown <= 0.0 && me.energy >= wo.stat("energy", 3.0) {
                                fire[si] = true;
                            }
                        }
                        "laser" => {
                            let range = wo.stat("range", 280.0);
                            let full = wo.stat("charge", 0.75);
                            let charge = wo.charge.unwrap_or(0.0);
                            let a = [(center[1] - muzzle[1]).atan2(center[0] - muzzle[0]), {
                                let h = d.max(0.1);
                                (center[2] - muzzle[2]).atan2(h)
                            }];
                            // lead a little for the remaining charge time? hitscan: aim at predicted pos at release
                            let rem = (full - charge).max(0.0);
                            let pc = [center[0] + tg.vel[0] * rem * 0.5, center[1] + tg.vel[1] * rem * 0.5, center[2]];
                            let a2 = [(pc[1] - muzzle[1]).atan2(pc[0] - muzzle[0]), (pc[2] - muzzle[2]).atan2(d2(pc, muzzle).max(0.1))];
                            let _ = a;
                            if desired.is_none() || d > 50.0 {
                                desired = Some(a2);
                            }
                            let err = ang_err(me.aim, a2);
                            let tol = (2.0f64 / d.max(1.0)).atan() + 0.008;
                            if d < range * 0.97 {
                                if charge <= 0.0 {
                                    // start charging only if cooled down and roughly aimed
                                    if wo.cooldown <= 0.0 && err < 0.35 && los {
                                        fire[si] = true;
                                        self.laser_hold = true;
                                    }
                                } else if charge < full * 0.98 {
                                    fire[si] = true; // keep charging
                                } else {
                                    // full: release when on target (or keep holding a bit)
                                    fire[si] = !(err < tol && los);
                                }
                            } else if charge > 0.0 {
                                fire[si] = charge < full * 0.98;
                            }
                        }
                        "mortar" => {
                            let speed = wo.stat("speed", 72.0);
                            let g = wo.stat("gravity", 22.0);
                            let tube = [pos[0], pos[1], pos[2] + self.mech_h + 0.5];
                            if d > 22.0 && d < 230.0 {
                                let high = !los;
                                if let Some(a) = mortar_aim(tube, [tg.pos[0], tg.pos[1], tg.pos[2] + 1.0], tg.vel, speed, g, high) {
                                    if desired.is_none() || !los {
                                        desired = Some(a);
                                    }
                                    let err = ang_err(me.aim, a);
                                    if err < 0.03 && wo.cooldown <= 0.0 {
                                        fire[si] = true;
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(a) = desired {
                    aim = a;
                }
            }
        }
        // laser: never hold a charge we don't need
        if !engaged {
            for (si, wo) in me.weapons.iter().enumerate().take(2) {
                if let Some(wo) = wo {
                    if wo.kind == "laser" && wo.charge.unwrap_or(0.0) > 0.0 {
                        fire[si] = false;
                    }
                }
            }
        }
        if !engaged {
            // face a threat: the attacker's bearing, the nearest visible enemy, a nearby sound, or scan
            if let Some(tg) = target {
                aim = [(tg.pos[1] - pos[1]).atan2(tg.pos[0] - pos[0]), 0.0];
            } else if let Some(b) = bearing {
                aim = [b, 0.0];
            } else if let Some(s) = self.sounds.iter().rev().find(|s| o.t - s.0 < 2.0 && s.3 != "drop" && s.3 != "death") {
                aim = [s.1, 0.0];
            } else if mode == Mode::Hold || (act.r#move[0] == 0.0 && act.r#move[1] == 0.0) {
                self.scan_yaw = wrap(self.scan_yaw + 0.25);
                aim = [self.scan_yaw, 0.0];
            } else {
                let m = act.r#move;
                aim = [m[1].atan2(m[0]), 0.0];
                self.scan_yaw = aim[0];
            }
            // shoot crates when calm (cubes + weapons)
            if let Some((cid, cpos)) = self.crate_shot(o) {
                let gun_slot = me.weapons.iter().position(|w| w.as_ref().map(|w| w.kind == "gun").unwrap_or(false));
                if let Some(si) = gun_slot {
                    let wo = me.weapons[si].as_ref().unwrap();
                    let c = [cpos[0], cpos[1], cpos[2] + 1.2];
                    let a = lead_aim(muzzle, c, [0.0, 0.0, 0.0], wo.stat("speed", 252.0));
                    aim = a;
                    let d = d2(c, muzzle);
                    if ang_err(me.aim, a) < (1.0f64 / d.max(1.0)).atan() + 0.006 && wo.cooldown <= 0.0 && wo.ammo.unwrap_or(1.0) > 0.0 && has_los(muzzle, c) {
                        fire[si] = true;
                    }
                    self.crate_target = Some(cid);
                }
            } else {
                self.crate_target = None;
                // top up the magazine when nothing is around
                for wo in me.weapons.iter().flatten() {
                    if wo.kind == "gun" && wo.ammo.unwrap_or(12.0) < wo.stat("mag", 12.0) * 0.5 && wo.reload.unwrap_or(0.0) <= 0.0 && target.is_none() {
                        act.reload = true;
                    }
                }
            }
        }
        aim[1] = aim[1].clamp(me.stat("min_pitch", -0.7), me.stat("max_pitch", 1.3));
        act.aim = [wrap(aim[0]), aim[1]];
        act.fire = fire;
    }

    fn crate_shot(&self, o: &Obs) -> Option<(i64, [f64; 3])> {
        if o.t > 240.0 || !self.tune.crates {
            return None;
        }
        if self.tracks.values().any(|t| !t.is_npc && o.t - t.seen_t < 3.0) || o.t - self.last_hurt_t < 6.0 {
            return None;
        }
        let pos = o.me.pos;
        let mut best = None;
        let mut bd = f64::MAX;
        for c in &o.crates {
            let d = d2(c.pos, pos);
            if d < 70.0 && d < bd && (c.pos[2] - pos[2]).abs() < 30.0 {
                bd = d;
                best = Some((c.id as i64, c.pos));
            }
        }
        best
    }
}

/// Angular error between actual aim and desired aim.
pub fn ang_err(a: [f64; 2], b: [f64; 2]) -> f64 {
    let dy = wrap(a[0] - b[0]).abs();
    let dp = (a[1] - b[1]).abs();
    (dy * dy + dp * dp).sqrt()
}

/// Aim [yaw,pitch] for a straight projectile with lead.
pub fn lead_aim(from: [f64; 3], to: [f64; 3], vel: [f64; 3], speed: f64) -> [f64; 2] {
    let p = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let v = [vel[0], vel[1], vel[2].clamp(-6.0, 6.0) * 0.3];
    let a = v[0] * v[0] + v[1] * v[1] + v[2] * v[2] - speed * speed;
    let b = 2.0 * (p[0] * v[0] + p[1] * v[1] + p[2] * v[2]);
    let c = p[0] * p[0] + p[1] * p[1] + p[2] * p[2];
    let mut t = (c.sqrt()) / speed;
    let disc = b * b - 4.0 * a * c;
    if disc >= 0.0 && a.abs() > 1e-9 {
        let s = disc.sqrt();
        let t1 = (-b - s) / (2.0 * a);
        let t2 = (-b + s) / (2.0 * a);
        let tt = [t1, t2].iter().cloned().filter(|x| *x > 0.0).fold(f64::MAX, f64::min);
        if tt < 5.0 {
            t = tt;
        }
    }
    let q = [p[0] + v[0] * t, p[1] + v[1] * t, p[2] + v[2] * t];
    let h = (q[0] * q[0] + q[1] * q[1]).sqrt();
    [q[1].atan2(q[0]), q[2].atan2(h.max(0.1))]
}

/// Ballistic aim for the mortar (high or low arc), leading the target by flight time.
pub fn mortar_aim(from: [f64; 3], to: [f64; 3], vel: [f64; 3], speed: f64, g: f64, high: bool) -> Option<[f64; 2]> {
    let mut tgt = to;
    let mut result = None;
    for _ in 0..3 {
        let dx = tgt[0] - from[0];
        let dy = tgt[1] - from[1];
        let d = (dx * dx + dy * dy).sqrt().max(0.1);
        let dz = tgt[2] - from[2];
        let v2 = speed * speed;
        let disc = v2 * v2 - g * (g * d * d + 2.0 * dz * v2);
        if disc < 0.0 {
            return None;
        }
        let s = disc.sqrt();
        let th = if high { ((v2 + s) / (g * d)).atan() } else { ((v2 - s) / (g * d)).atan() };
        let flight = d / (speed * th.cos()).max(0.1);
        result = Some([dy.atan2(dx), th]);
        tgt = [to[0] + vel[0] * flight, to[1] + vel[1] * flight, to[2]];
    }
    result
}

