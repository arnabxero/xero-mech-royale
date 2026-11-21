//! Free-for-all simulator: 16 mechs with approximate physics (sim.rs), vision (FOV wedge, near
//! sensors, LOS, 5 s memory), sounds, guns with travel time and spread, lasers and lightning,
//! rear armor, regen, zone damage and placement. Opponents are scripted bots modelled on the kit
//! example (walk to the zone center, shoot the nearest visible mech) plus variants.
use crate::bot::{wrap, Bot, Rng};
use crate::proto::*;
use crate::sim::*;
use crate::world::World;
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Ours,
    Aggro,
    Squad,
    Example,
    Rusher,
    Camper,
}

#[derive(Clone, Debug)]
pub struct SimWeapon {
    pub kind: String,
    pub cooldown: f64,
    pub ammo: f64,
    pub reload: f64,
    pub charge: f64,
}

impl SimWeapon {
    fn gun() -> Self {
        SimWeapon { kind: "gun".into(), cooldown: 0.0, ammo: 12.0, reload: 0.0, charge: 0.0 }
    }
    fn of(kind: &str) -> Self {
        SimWeapon { kind: kind.into(), cooldown: 1.0, ammo: 0.0, reload: 0.0, charge: 0.0 }
    }
}

pub struct SimMech {
    pub id: usize,
    pub kind: Kind,
    pub body: Body,
    pub aim: [f64; 2],
    pub hp: f64,
    pub max_hp: f64,
    pub alive: bool,
    pub place: usize,
    pub death_t: f64,
    pub killed_by_zone: bool,
    pub weapons: Vec<Option<SimWeapon>>,
    pub last_combat_t: f64,
    pub seen: HashMap<usize, f64>,
    pub events: Vec<Event>,
    pub sounds: Vec<SoundObs>,
    pub act: Action,
    pub bot: Option<Bot>,
    pub step_t: f64,
    pub kills: u32,
    pub dmg_dealt: f64,
    pub dmg_taken: f64,
    pub zone_dmg: f64,
    pub laser_charging: bool,
    pub killer: Option<usize>,
    pub mode_at_death: String,
    /// seconds during which at least one living enemy could see this mech
    pub seen_s: f64,
    /// seconds alive
    pub alive_s: f64,
    /// mode histogram (seconds) for bots
    pub modes: HashMap<String, f64>,
}

struct Bullet {
    owner: usize,
    pos: [f64; 3],
    vel: [f64; 3],
    dist_left: f64,
    dmg: f64,
}

struct Pickup {
    id: u64,
    kind: String,
    pos: [f64; 3],
}

pub struct Ffa<'a> {
    pub w: &'a World,
    pub cfg: Config,
    pub ph: Phys,
    pub zs: ZoneSched,
    pub mechs: Vec<SimMech>,
    bullets: Vec<Bullet>,
    pickups: Vec<Pickup>,
    pub t: f64,
    pub tick: u64,
    pub rng: Rng,
    pub alive: usize,
    pub budget_ms: u64,
    pub log: bool,
    /// squad members currently hunting (they always know where our mechs are)
    pub hunters: Vec<usize>,
    pub all_hunt_t: f64,
}

fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn weapon_obs(w: &SimWeapon, cfg: &Config) -> WeaponObs {
    let mut o = WeaponObs { kind: w.kind.clone(), cooldown: w.cooldown.max(0.0), ..Default::default() };
    if let Some(sec) = cfg.raw.get("weapons").and_then(|x| x.get(&w.kind)).and_then(|x| x.as_object()) {
        for (k, v) in sec {
            if let Some(f) = v.as_f64() {
                o.stats.insert(k.clone(), f);
            }
        }
    }
    match w.kind.as_str() {
        "gun" => {
            o.ammo = Some(w.ammo);
            o.reload = Some(w.reload.max(0.0));
        }
        "laser" => o.charge = Some(w.charge),
        _ => {}
    }
    o
}

impl<'a> Ffa<'a> {
    pub fn new(w: &'static World, cfg: Config, seed: u64, kinds: &[Kind]) -> Ffa<'a> {
        let mut rng = Rng::new(seed);
        let bnd = Some([w.bounds[0] as f64, w.bounds[1] as f64, w.bounds[2] as f64, w.bounds[3] as f64]);
        let zs = ZoneSched::new(&cfg, &mut rng, bnd);
        let ph = Phys::from(&cfg);
        let (sc, sr) = zs.circles[0];
        let mut spawns: Vec<[f64; 3]> = Vec::new();
        let mut tries = 0;
        let duel = kinds.len() == 2;
        while spawns.len() < kinds.len() && tries < 200_000 {
            tries += 1;
            let a = rng.f() * std::f64::consts::TAU;
            let r = sr * 0.8 * rng.f().sqrt();
            let x = sc[0] + a.cos() * r;
            let y = sc[1] + a.sin() * r;
            if x < w.bounds[0] as f64 + 30.0 || x > w.bounds[1] as f64 - 30.0 || y < w.bounds[2] as f64 + 30.0 || y > w.bounds[3] as f64 - 30.0 {
                continue;
            }
            let c = match w.cell_of(x as f32, y as f32) {
                Some(c) => c,
                None => continue,
            };
            if !w.standable(c) || w.flags[c] & crate::world::PRISM_FLOOR != 0 {
                continue;
            }
            let sep = if tries < 100_000 { 130.0 } else { 60.0 };
            if spawns.iter().any(|s| dist2(*s, [x, y, 0.0]) < sep) {
                continue;
            }
            // duels start at a typical engagement distance (130-200 m)
            if duel && spawns.len() == 1 && dist2(spawns[0], [x, y, 0.0]) > 200.0 {
                continue;
            }
            spawns.push([x, y, w.floor[c] as f64]);
        }
        let mut mechs = Vec::new();
        for (i, k) in kinds.iter().enumerate() {
            let bot = match k {
                Kind::Ours => {
                    let mut b = Bot::new(cfg.clone(), Some(w), i as i64, seed * 31 + i as u64);
                    if let Ok(spec) = std::env::var("XERO_TUNE") {
                        b.tune.apply(&spec);
                    }
                    Some(b)
                }
                Kind::Aggro => {
                    let mut b = Bot::new(cfg.clone(), Some(w), i as i64, seed * 31 + i as u64);
                    b.tune = crate::bot::Tune::aggressive();
                    Some(b)
                }
                Kind::Squad => {
                    // non-hunting squad members hold the zone and only engage us up close
                    let mut b = Bot::new(cfg.clone(), Some(w), i as i64, seed * 31 + i as u64);
                    b.tune.fight_close = 100.0;
                    b.tune.fight_back_range = 230.0;
                    b.tune.cover_from_far = false;
                    b.tune.retreat_hp = 0.0;
                    b.tune.dodge = false;
                    Some(b)
                }
                _ => None,
            };
            let aim0 = rng.f() * 6.28 - 3.14;
            mechs.push(SimMech {
                id: i,
                kind: *k,
                body: Body { pos: spawns[i % spawns.len()], vel: [0.0; 3], grounded: true, energy: 100.0, since_spend: 0.0 },
                aim: [aim0, 0.0],
                hp: 1400.0,
                max_hp: 1400.0,
                alive: true,
                place: 0,
                death_t: 0.0,
                killed_by_zone: false,
                // the house squad shows up with a laser already (seen in the recorded practice stream)
                weapons: vec![
                    Some(SimWeapon::gun()),
                    if *k == Kind::Squad {
                        Some(SimWeapon::of("laser"))
                    } else if *k == Kind::Ours {
                        std::env::var("SIM_OURS_WEAPON").ok().map(|w| SimWeapon::of(&w))
                    } else {
                        None
                    },
                ],
                last_combat_t: -99.0,
                seen: HashMap::new(),
                events: vec![],
                sounds: vec![],
                act: Action::default(),
                bot,
                step_t: 0.0,
                kills: 0,
                dmg_dealt: 0.0,
                dmg_taken: 0.0,
                zone_dmg: 0.0,
                laser_charging: false,
                killer: None,
                mode_at_death: String::new(),
                seen_s: 0.0,
                alive_s: 0.0,
                modes: HashMap::new(),
            });
        }
        // floor loot: laser / lightning (mortar not simulated yet)
        let mut pickups = Vec::new();
        let mut pid = 8000;
        let mut tries = 0;
        while pickups.len() < 30 && tries < 100_000 {
            tries += 1;
            let x = w.bounds[0] as f64 + 30.0 + rng.f() * (w.bounds[1] - w.bounds[0] - 60.0) as f64;
            let y = w.bounds[2] as f64 + 30.0 + rng.f() * (w.bounds[3] - w.bounds[2] - 60.0) as f64;
            if let Some(c) = w.cell_of(x as f32, y as f32) {
                if w.standable(c) && w.flags[c] & crate::world::PRISM_FLOOR == 0 {
                    let kind = if rng.f() < 0.5 { "laser" } else { "lightning" };
                    pickups.push(Pickup { id: pid, kind: kind.into(), pos: [x, y, w.floor[c] as f64] });
                    pid += 1;
                }
            }
        }
        let n = mechs.len();
        let mut f = Ffa { w, cfg, ph, zs, mechs, bullets: vec![], pickups, t: 0.0, tick: 0, rng, alive: n, budget_ms: 18, log: false, hunters: vec![], all_hunt_t: 300.0 };
        f.add_hunter();
        if f.mechs.len() == 2 {
            for m in f.mechs.iter_mut() {
                if m.kind == Kind::Aggro {
                    if let Some(b) = m.bot.as_mut() {
                        b.tune.hunt = true;
                    }
                }
            }
        }
        f
    }

    /// Promote one more squad member to hunter (the one nearest to our first mech).
    pub fn add_hunter(&mut self) {
        let ours: Vec<usize> = (0..self.mechs.len()).filter(|&i| self.mechs[i].kind == Kind::Ours && self.mechs[i].alive).collect();
        let o = match ours.first() {
            Some(&o) => o,
            None => return,
        };
        let mut best = None;
        let mut bd = f64::MAX;
        for j in 0..self.mechs.len() {
            if self.mechs[j].kind != Kind::Squad || !self.mechs[j].alive || self.hunters.contains(&j) {
                continue;
            }
            let d = dist2(self.mechs[j].body.pos, self.mechs[o].body.pos);
            if d < bd {
                bd = d;
                best = Some(j);
            }
        }
        if let Some(j) = best {
            self.hunters.push(j);
            if let Some(b) = self.mechs[j].bot.as_mut() {
                b.tune = crate::bot::Tune::aggressive();
                b.tune.hunt = true;
                b.tune.dodge = false;
            }
        }
    }

    fn eye(&self, m: &SimMech) -> [f64; 3] {
        [m.body.pos[0], m.body.pos[1], m.body.pos[2] + 8.0]
    }

    /// Can viewer i see mech j right now (protocol §8, canopy approximated)?
    fn can_see(&self, i: usize, j: usize) -> bool {
        let a = &self.mechs[i];
        let b = &self.mechs[j];
        let d = dist2(a.body.pos, b.body.pos);
        if d > 200.0 {
            return false;
        }
        let near = d <= 45.0;
        if !near {
            let yaw = (b.body.pos[1] - a.body.pos[1]).atan2(b.body.pos[0] - a.body.pos[0]);
            if wrap(yaw - a.aim[0]).abs() > 60f64.to_radians() {
                return false;
            }
        }
        let e = self.eye(a);
        let f = |p: [f64; 3]| [p[0] as f32, p[1] as f32, p[2] as f32];
        self.w.mech_visible(f(e), f(b.body.pos), 9.0, 45.0)
    }

    fn hear(&mut self, from: [f64; 3], kind: &str, range: f64, except: usize) {
        for k in 0..self.mechs.len() {
            if k == except || !self.mechs[k].alive {
                continue;
            }
            let p = self.mechs[k].body.pos;
            let d = dist2(p, from);
            if d > range {
                continue;
            }
            let noise = (self.rng.f() + self.rng.f() + self.rng.f() - 1.5) * 2.0 * 7f64.to_radians();
            let bearing = wrap((from[1] - p[1]).atan2(from[0] - p[0]) + noise);
            let band = if d < 60.0 { "near" } else if d < 160.0 { "mid" } else { "far" };
            self.mechs[k].sounds.push(SoundObs { kind: kind.into(), bearing, dist: band.into(), age: 0.0 });
        }
    }

    fn obs_for(&mut self, i: usize) -> Obs {
        let t = self.t;
        let zone = self.zs.obs(t);
        let (zc, zr) = zone.at(t);
        let n = self.mechs.len();
        let mut mechs = Vec::new();
        for j in 0..n {
            if j == i || !self.mechs[j].alive {
                continue;
            }
            let squad_i = self.mechs[i].kind == Kind::Squad;
            if squad_i && self.mechs[j].kind == Kind::Squad {
                continue; // teammates
            }
            let hunting = squad_i && (self.hunters.contains(&i) || t >= self.all_hunt_t);
            if self.can_see(i, j) || (hunting && self.mechs[j].kind == Kind::Ours) {
                self.mechs[i].seen.insert(j, t);
            }
            if let Some(&st) = self.mechs[i].seen.get(&j) {
                if t - st <= 5.0 {
                    let m = &self.mechs[j];
                    mechs.push(ActorObs {
                        id: j as f64,
                        kind: "mech".into(),
                        pos: m.body.pos,
                        vel: m.body.vel,
                        aim: m.aim,
                        hp: Some(m.hp),
                        max_hp: Some(m.max_hp),
                        cubes: Some(0.0),
                        weapons: m.weapons.iter().flatten().map(|w| w.kind.clone()).collect(),
                        charging: m.laser_charging,
                    });
                }
            }
        }
        let me = &self.mechs[i];
        let mut projectiles = Vec::new();
        for (k, b) in self.bullets.iter().enumerate() {
            if b.owner == i {
                continue;
            }
            let d = dist2(b.pos, me.body.pos);
            if d < 45.0 {
                projectiles.push(ProjObs { id: k as f64, kind: "gun".into(), pos: b.pos, vel: b.vel });
            }
        }
        let pickups: Vec<PickupObs> = self
            .pickups
            .iter()
            .filter(|p| dist2(p.pos, me.body.pos) < 120.0)
            .map(|p| PickupObs { id: p.id as f64, kind: p.kind.clone(), pos: p.pos })
            .collect();
        let in_zone = ((me.body.pos[0] - zc[0]).powi(2) + (me.body.pos[1] - zc[1]).powi(2)).sqrt() <= zr;
        let me = &mut self.mechs[i];
        Obs {
            kind: "obs".into(),
            tick: self.tick as f64,
            t,
            alive: self.alive as f64,
            me: SelfObs {
                id: i as f64,
                pos: me.body.pos,
                vel: me.body.vel,
                aim: me.aim,
                hp: me.hp,
                max_hp: me.max_hp,
                energy: me.body.energy,
                max_energy: 100.0,
                grounded: me.body.grounded,
                cubes: 0.0,
                upgrades: vec![],
                weapons: me.weapons.iter().map(|w| w.as_ref().map(|w| weapon_obs(w, &self.cfg))).collect(),
                in_zone,
                stats: HashMap::new(),
            },
            mechs,
            projectiles,
            pickups,
            sounds: std::mem::take(&mut me.sounds),
            events: std::mem::take(&mut me.events),
            zone,
            ..Default::default()
        }
    }

    fn scripted(&mut self, i: usize, o: &Obs) -> Action {
        let kind = self.mechs[i].kind;
        let me = &o.me;
        let (x, y, z) = (me.pos[0], me.pos[1], me.pos[2]);
        let goal = o.zone.next_center;
        let mut act = Action { tick: tick_value(o.tick), aim: me.aim, ..Default::default() };
        let mut enemies: Vec<&ActorObs> = o.mechs.iter().filter(|m| m.kind == "mech").collect();
        enemies.sort_by(|a, b| dist2(a.pos, me.pos).partial_cmp(&dist2(b.pos, me.pos)).unwrap());
        let mut yaw = (goal[1] - y).atan2(goal[0] - x);
        let gd = ((goal[0] - x).powi(2) + (goal[1] - y).powi(2)).sqrt();
        let mut mv = [yaw.cos(), yaw.sin()];
        if kind == Kind::Camper && gd < o.zone.next_radius * 0.5 {
            mv = [0.0, 0.0];
        }
        if kind == Kind::Rusher {
            if let Some(e) = enemies.first() {
                yaw = (e.pos[1] - y).atan2(e.pos[0] - x);
                if dist2(e.pos, me.pos) > 25.0 {
                    mv = [yaw.cos(), yaw.sin()];
                }
            }
        }
        // grab a weapon when standing on it
        if me.weapons.get(1).map(|w| w.is_none()).unwrap_or(false) {
            if let Some(p) = o.pickups.iter().find(|p| dist2(p.pos, me.pos) < 4.0) {
                act.take = Some(p.id as u64);
            }
        }
        act.r#move = mv;
        act.aim = [yaw, 0.0];
        act.boost = !me.in_zone;
        act.jump = o.tick as u64 % 40 == 0;
        if let Some(e) = enemies.first() {
            let t = e.pos;
            let d = dist2(t, me.pos);
            if kind == Kind::Example {
                act.aim = [(t[1] - y).atan2(t[0] - x), (t[2] - z - 2.5).atan2(d)];
            } else {
                act.aim = crate::bot::lead_aim([x, y, z + 7.0], [t[0], t[1], t[2] + 4.5], e.vel, 252.0);
            }
            act.fire = [true, true];
            // laser: release when charged
            if let Some(Some(w)) = me.weapons.get(1) {
                if w.kind == "laser" {
                    act.fire[1] = w.charge.unwrap_or(0.0) < 0.74;
                }
            }
        }
        if let Some(off) = &o.offer {
            let _ = off;
            act.pick = Some(0);
        }
        act
    }

    fn damage(&mut self, target: usize, by: usize, amount: f64, weapon: &str, from_dir: [f64; 2]) {
        if !self.mechs[target].alive {
            return;
        }
        if self.mechs[target].kind == Kind::Squad && self.mechs[by].kind == Kind::Squad {
            return;
        }
        // from_dir: direction the shot travels (xy); attack comes from -from_dir
        let came_from = (-from_dir[1]).atan2(-from_dir[0]);
        let rear = wrap(came_from - self.mechs[target].aim[0]).abs() > std::f64::consts::FRAC_PI_2;
        let amt = if rear { amount * 1.5 } else { amount };
        let t = self.t;
        {
            let m = &mut self.mechs[target];
            m.hp -= amt;
            m.last_combat_t = t;
            m.dmg_taken += amt;
            m.events.push(Event {
                kind: "damaged".into(),
                amount: Some(amt),
                weapon: Some(weapon.into()),
                bearing: Some(came_from),
                by: Some(by as f64),
                rear: Some(rear),
                ..Default::default()
            });
        }
        {
            let a = &mut self.mechs[by];
            a.last_combat_t = t;
            a.dmg_dealt += amt;
            a.events.push(Event { kind: "hit".into(), target: Some(target as f64), amount: Some(amt), weapon: Some(weapon.into()), ..Default::default() });
        }
        if self.mechs[target].hp <= 0.0 {
            if self.mechs[by].kind == Kind::Ours && self.mechs[target].kind == Kind::Squad {
                self.add_hunter();
            }
            self.mechs[target].killer = Some(by);
            self.kill(target, false);
            self.mechs[by].kills += 1;
            self.mechs[by].events.push(Event { kind: "kill".into(), target: Some(target as f64), ..Default::default() });
            // death drop: non-gun weapons
            let p = self.mechs[target].body.pos;
            let ws: Vec<String> = self.mechs[target].weapons.iter().flatten().filter(|w| w.kind != "gun").map(|w| w.kind.clone()).collect();
            for k in ws {
                let id = 9000 + self.rng.next_u64() % 100000;
                self.pickups.push(Pickup { id, kind: k, pos: p });
            }
        }
    }

    fn kill(&mut self, i: usize, zone: bool) {
        let m = &mut self.mechs[i];
        if !m.alive {
            return;
        }
        m.alive = false;
        m.mode_at_death = m.bot.as_ref().map(|b| b.label.clone()).unwrap_or_default();
        m.place = self.alive;
        m.death_t = self.t;
        m.killed_by_zone = zone;
        let p = m.body.pos;
        self.alive -= 1;
        self.hear(p, "death", 600.0, i);
    }

    /// One 0.05 s tick.
    pub fn step(&mut self) {
        let dt = 0.05;
        let n = self.mechs.len();
        if self.t >= self.all_hunt_t && self.tick % 20 == 0 {
            for j in 0..n {
                if self.mechs[j].kind == Kind::Squad {
                    if let Some(b) = self.mechs[j].bot.as_mut() {
                        if !b.tune.hunt {
                            b.tune = crate::bot::Tune::aggressive();
                            b.tune.hunt = true;
                            b.tune.dodge = false;
                        }
                    }
                }
            }
        }
        let decide = self.tick % 2 == 0;
        if decide {
            for i in 0..n {
                if !self.mechs[i].alive {
                    continue;
                }
                let o = self.obs_for(i);
                if self.mechs[i].kind == Kind::Ours {
                    let seen = (0..n).any(|j| j != i && self.mechs[j].alive && self.can_see(j, i));
                    let m = &mut self.mechs[i];
                    m.alive_s += 0.1;
                    if seen {
                        m.seen_s += 0.1;
                    }
                    if let Some(b) = m.bot.as_ref() {
                        let md = format!("{:?}", b.mode);
                        *m.modes.entry(md).or_insert(0.0) += 0.1;
                    }
                }
                let act = if self.mechs[i].bot.is_some() {
                    let t0 = Instant::now();
                    let b = self.mechs[i].bot.as_mut().unwrap();
                    b.act(&o, t0 + Duration::from_millis(self.budget_ms))
                } else {
                    self.scripted(i, &o)
                };
                // take
                if let Some(id) = act.take {
                    if let Some(k) = self.pickups.iter().position(|p| p.id == id && dist2(p.pos, self.mechs[i].body.pos) < 4.0) {
                        let kind = self.pickups[k].kind.clone();
                        let m = &mut self.mechs[i];
                        let have = m.weapons.iter().flatten().any(|w| w.kind == kind);
                        if !have {
                            let slot = act.drop.map(|d| d as usize).or_else(|| m.weapons.iter().position(|w| w.is_none()));
                            if let Some(s) = slot {
                                let old = m.weapons[s].take();
                                m.weapons[s] = Some(SimWeapon::of(&kind));
                                m.events.push(Event { kind: "took".into(), id: Some(serde_json::Value::from(id)), wkind: Some(kind.clone()), slot: Some(s as f64), ..Default::default() });
                                let ppos = self.pickups[k].pos;
                                self.pickups.remove(k);
                                if let Some(ow) = old {
                                    let nid = 9000 + self.rng.next_u64() % 100000;
                                    self.pickups.push(Pickup { id: nid, kind: ow.kind, pos: ppos });
                                }
                            } else {
                                m.events.push(Event { kind: "take_failed".into(), id: Some(serde_json::Value::from(id)), reason: Some("full".into()), ..Default::default() });
                            }
                        } else {
                            self.mechs[i].events.push(Event { kind: "take_failed".into(), id: Some(serde_json::Value::from(id)), reason: Some("have".into()), ..Default::default() });
                        }
                    }
                }
                self.mechs[i].act = act;
            }
        }
        // movement, aim, weapons
        let zone = self.zs.obs(self.t);
        let (zc, zr) = zone.at(self.t);
        for i in 0..n {
            if !self.mechs[i].alive {
                continue;
            }
            let act = self.mechs[i].act.clone();
            {
                let m = &mut self.mechs[i];
                let dyaw = wrap(act.aim[0] - m.aim[0]);
                let mx = 4.0 * dt;
                m.aim[0] = wrap(m.aim[0] + dyaw.clamp(-mx, mx));
                let tp = act.aim[1].clamp(-0.7, 1.3);
                m.aim[1] += (tp - m.aim[1]).clamp(-3.0 * dt, 3.0 * dt);
                let charging = m.laser_charging;
                let mv = if charging { [act.r#move[0] * 0.5, act.r#move[1] * 0.5] } else { act.r#move };
                step(self.w, &self.ph, &mut m.body, mv, act.boost, act.jump && decide, dt);
                let sp = m.body.vel[0].hypot(m.body.vel[1]);
                m.step_t += dt;
                if m.body.grounded && sp > 5.0 && m.step_t > 0.6 {
                    m.step_t = 0.0;
                }
            }
            // footstep / boost sounds
            {
                let m = &self.mechs[i];
                let sp = m.body.vel[0].hypot(m.body.vel[1]);
                if m.body.grounded && sp > 5.0 && m.step_t == 0.0 {
                    let p = m.body.pos;
                    let k = if act.boost { "boost" } else { "step" };
                    let r = if act.boost { 90.0 } else { 40.0 };
                    self.hear(p, k, r, i);
                }
            }
            // weapons
            for s in 0..2 {
                let mut fire_gun = false;
                let mut fire_laser: Option<f64> = None;
                let mut fire_lightning = false;
                {
                    let m = &mut self.mechs[i];
                    let w = match m.weapons[s].as_mut() {
                        Some(w) => w,
                        None => continue,
                    };
                    w.cooldown -= dt;
                    match w.kind.as_str() {
                        "gun" => {
                            if w.reload > 0.0 {
                                w.reload -= dt;
                                if w.reload <= 0.0 {
                                    w.ammo = 12.0;
                                    w.reload = 0.0;
                                }
                            } else if act.reload && w.ammo < 12.0 {
                                w.reload = 2.4;
                            } else if act.fire[s] && w.cooldown <= 0.0 && w.ammo > 0.0 {
                                w.cooldown = 0.24;
                                w.ammo -= 1.0;
                                if w.ammo <= 0.0 {
                                    w.reload = 2.4;
                                }
                                fire_gun = true;
                            }
                        }
                        "laser" => {
                            if act.fire[s] && w.cooldown <= 0.0 {
                                w.charge += dt;
                                m.laser_charging = true;
                                if w.charge >= 0.75 + 2.0 {
                                    fire_laser = Some(w.charge);
                                }
                            } else if w.charge > 0.0 {
                                if w.charge >= 0.15 {
                                    fire_laser = Some(w.charge);
                                } else {
                                    w.charge += dt;
                                }
                            }
                            if fire_laser.is_some() {
                                w.charge = 0.0;
                                w.cooldown = 1.65;
                                m.laser_charging = false;
                            }
                        }
                        "lightning" => {
                            if act.fire[s] && w.cooldown <= 0.0 && m.body.energy >= 3.0 {
                                w.cooldown = 0.25;
                                m.body.energy -= 3.0;
                                m.body.since_spend = 0.0;
                                fire_lightning = true;
                            }
                        }
                        _ => {}
                    }
                }
                let m = &self.mechs[i];
                let dir = [m.aim[1].cos() * m.aim[0].cos(), m.aim[1].cos() * m.aim[0].sin(), m.aim[1].sin()];
                let muzzle = [m.body.pos[0], m.body.pos[1], m.body.pos[2] + 7.0];
                let start = [muzzle[0] + dir[0] * 3.0, muzzle[1] + dir[1] * 3.0, muzzle[2] + dir[2] * 3.0];
                if fire_gun {
                    let sp = 0.6f64.to_radians();
                    let ny = (self.rng.f() + self.rng.f() + self.rng.f() - 1.5) * 2.0 * sp;
                    let np = (self.rng.f() + self.rng.f() + self.rng.f() - 1.5) * 2.0 * sp;
                    let yaw = m.aim[0] + ny;
                    let pitch = m.aim[1] + np;
                    let v = [pitch.cos() * yaw.cos() * 252.0, pitch.cos() * yaw.sin() * 252.0, pitch.sin() * 252.0];
                    self.bullets.push(Bullet { owner: i, pos: start, vel: v, dist_left: 230.0, dmg: 45.0 });
                    self.hear(muzzle, "gun", 320.0, i);
                }
                if let Some(ch) = fire_laser {
                    let frac = 0.35 + 0.65 * (ch / 0.75).min(1.0);
                    let dmg = 315.0 * frac;
                    // hitscan: first mech along the ray within 280 m with LOS
                    let mut best: Option<(usize, f64)> = None;
                    for j in 0..n {
                        if j == i || !self.mechs[j].alive {
                            continue;
                        }
                        if let Some(tt) = ray_cyl(start, dir, self.mechs[j].body.pos, 2.5, 9.0, 280.0) {
                            if best.map(|b| tt < b.1).unwrap_or(true) {
                                best = Some((j, tt));
                            }
                        }
                    }
                    self.hear(muzzle, "laser", 380.0, i);
                    if let Some((j, tt)) = best {
                        let hit = [start[0] + dir[0] * tt, start[1] + dir[1] * tt, start[2] + dir[2] * tt];
                        let f = |p: [f64; 3]| [p[0] as f32, p[1] as f32, p[2] as f32];
                        if self.w.los(f(start), f(hit)) {
                            self.damage(j, i, dmg, "laser", [dir[0], dir[1]]);
                        }
                    }
                }
                if fire_lightning {
                    self.hear(muzzle, "lightning", 160.0, i);
                    let mut best: Option<(usize, f64)> = None;
                    for j in 0..n {
                        if j == i || !self.mechs[j].alive {
                            continue;
                        }
                        let tp = self.mechs[j].body.pos;
                        let d = dist2(tp, muzzle);
                        if d > 45.0 {
                            continue;
                        }
                        let yaw = (tp[1] - muzzle[1]).atan2(tp[0] - muzzle[0]);
                        if wrap(yaw - self.mechs[i].aim[0]).abs() > 15f64.to_radians() {
                            continue;
                        }
                        let f = |p: [f64; 3]| [p[0] as f32, p[1] as f32, p[2] as f32];
                        if !self.w.los(f(muzzle), f([tp[0], tp[1], tp[2] + 4.5])) {
                            continue;
                        }
                        if best.map(|b| d < b.1).unwrap_or(true) {
                            best = Some((j, d));
                        }
                    }
                    if let Some((j, _)) = best {
                        let tp = self.mechs[j].body.pos;
                        let dd = [tp[0] - muzzle[0], tp[1] - muzzle[1]];
                        self.damage(j, i, 60.0, "lightning", dd);
                        let t = self.t;
                        let m = &mut self.mechs[j];
                        for wsl in m.weapons.iter_mut().flatten() {
                            if wsl.kind == "laser" {
                                wsl.charge = 0.0;
                                wsl.cooldown = wsl.cooldown.max(2.0);
                            }
                        }
                        m.laser_charging = false;
                        m.events.push(Event { kind: "effect".into(), effect: Some("short".into()), by: Some(i as f64), ..Default::default() });
                        let _ = t;
                    }
                }
            }
        }
        // bullets
        let mut k = 0;
        while k < self.bullets.len() {
            let b = &self.bullets[k];
            let step_len = 252.0 * dt;
            let np = [b.pos[0] + b.vel[0] * dt, b.pos[1] + b.vel[1] * dt, b.pos[2] + b.vel[2] * dt];
            let dir = [b.vel[0] / 252.0, b.vel[1] / 252.0, b.vel[2] / 252.0];
            let mut hit: Option<(usize, f64)> = None;
            for j in 0..n {
                if j == b.owner || !self.mechs[j].alive {
                    continue;
                }
                if let Some(tt) = ray_cyl(b.pos, dir, self.mechs[j].body.pos, 2.8, 9.0, step_len) {
                    if hit.map(|h| tt < h.1).unwrap_or(true) {
                        hit = Some((j, tt));
                    }
                }
            }
            let f = |p: [f64; 3]| [p[0] as f32, p[1] as f32, p[2] as f32];
            let wall_ok = match hit {
                Some((_, tt)) => self.w.los(f(b.pos), f([b.pos[0] + dir[0] * tt, b.pos[1] + dir[1] * tt, b.pos[2] + dir[2] * tt])),
                None => self.w.los(f(b.pos), f(np)),
            };
            if !wall_ok {
                self.bullets.swap_remove(k);
                continue;
            }
            if let Some((j, _)) = hit {
                let owner = b.owner;
                let dmg = b.dmg;
                let v = [b.vel[0], b.vel[1]];
                self.bullets.swap_remove(k);
                self.damage(j, owner, dmg, "gun", v);
                continue;
            }
            let b = &mut self.bullets[k];
            b.pos = np;
            b.dist_left -= step_len;
            if b.dist_left <= 0.0 {
                self.bullets.swap_remove(k);
                continue;
            }
            k += 1;
        }
        // zone, regen
        for i in 0..n {
            if !self.mechs[i].alive {
                continue;
            }
            let p = self.mechs[i].body.pos;
            let inside = ((p[0] - zc[0]).powi(2) + (p[1] - zc[1]).powi(2)).sqrt() <= zr;
            let t = self.t;
            let m = &mut self.mechs[i];
            if !inside {
                m.hp -= zone.dps * dt;
                m.zone_dmg += zone.dps * dt;
                m.last_combat_t = t;
                if (self.tick % 10) == 0 {
                    m.events.push(Event { kind: "damaged".into(), amount: Some(zone.dps), weapon: Some("zone".into()), ..Default::default() });
                }
            } else if t - m.last_combat_t >= 5.0 {
                m.hp = (m.hp + 45.0 * dt).min(m.max_hp);
            }
            if m.hp <= 0.0 {
                self.kill(i, true);
            }
        }
        self.tick += 1;
        self.t = self.tick as f64 * dt;
    }

    /// Run until one mech is left or time runs out; returns placements per mech.
    pub fn run(&mut self) -> Vec<usize> {
        while self.alive > 1 && self.t < 480.0 {
            self.step();
        }
        // survivors ranked by HP
        let mut surv: Vec<usize> = (0..self.mechs.len()).filter(|&i| self.mechs[i].alive).collect();
        surv.sort_by(|a, b| self.mechs[*b].hp.partial_cmp(&self.mechs[*a].hp).unwrap());
        for (r, &i) in surv.iter().enumerate() {
            self.mechs[i].place = r + 1;
        }
        self.mechs.iter().map(|m| m.place).collect()
    }
}

/// Ray (origin o, unit dir d) vs vertical cylinder at feet p: distance along the ray within maxt.
pub fn ray_cyl(o: [f64; 3], d: [f64; 3], p: [f64; 3], r: f64, h: f64, maxt: f64) -> Option<f64> {
    let ox = o[0] - p[0];
    let oy = o[1] - p[1];
    let a = d[0] * d[0] + d[1] * d[1];
    let b = 2.0 * (ox * d[0] + oy * d[1]);
    let c = ox * ox + oy * oy - r * r;
    let mut t0 = 0.0;
    let mut t1 = maxt;
    if a < 1e-12 {
        if c > 0.0 {
            return None;
        }
    } else {
        let disc = b * b - 4.0 * a * c;
        if disc < 0.0 {
            return None;
        }
        let s = disc.sqrt();
        t0 = f64::max(t0, (-b - s) / (2.0 * a));
        t1 = f64::min(t1, (-b + s) / (2.0 * a));
        if t0 > t1 {
            return None;
        }
    }
    // vertical slab
    let zlo = p[2];
    let zhi = p[2] + h;
    if d[2].abs() < 1e-12 {
        if o[2] < zlo || o[2] > zhi {
            return None;
        }
    } else {
        let ta = (zlo - o[2]) / d[2];
        let tb = (zhi - o[2]) / d[2];
        let (lo, hi) = if ta < tb { (ta, tb) } else { (tb, ta) };
        t0 = t0.max(lo);
        t1 = t1.min(hi);
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}
