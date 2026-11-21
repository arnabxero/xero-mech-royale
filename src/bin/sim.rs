//! Local tools: `sim stats` (world build + flow-field timing) and `sim nav [runs] [seed]`
//! (navigation smoke test: one mech, approximate physics, random spawns and zone sequences).
use std::path::PathBuf;
use std::time::{Duration, Instant};
use xero_mech::bot::{Bot, Rng};
use xero_mech::nav::FlowField;
use xero_mech::proto::*;
use xero_mech::sim::*;
use xero_mech::world::World;

fn arena() -> PathBuf {
    std::env::var("ARENA").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("arena"))
}

fn load(cfg: &Config) -> World {
    let t0 = Instant::now();
    let w = World::load(&arena(), xero_mech::params_from(cfg)).expect("map");
    eprintln!("world built in {:.0} ms", t0.elapsed().as_secs_f64() * 1000.0);
    w
}

fn stats() {
    let cfg = default_config();
    let w = load(&cfg);
    let n = w.nx * w.ny;
    let stand = (0..n).filter(|&c| w.standable(c)).count();
    let roof = (0..n).filter(|&c| w.standable(c) && w.flags[c] & xero_mech::world::PRISM_FLOOR != 0).count();
    println!("cells {n} standable {stand} roof-standable {roof} intervals {} jump edges {}", w.iv.len(), w.jf.len());
    let mut ff = FlowField::new(n);
    for (cx, cy, r) in [(41.3f32, -72.8f32, 300.0f32), (-276.0, 128.0, 20.0), (300.0, -250.0, 50.0)] {
        let t0 = Instant::now();
        ff.start_disc(&w, (0, 0, 0), cx, cy, r, 40);
        let mut slices = 0;
        while !ff.run(&w, Instant::now() + Duration::from_millis(15)) {
            slices += 1;
        }
        let reached = ff.dist.iter().filter(|d| d.is_finite()).count();
        println!(
            "flow field to ({cx},{cy}) r{r}: {:.1} ms, {} slices of 15 ms, pops {}, reached {reached}",
            t0.elapsed().as_secs_f64() * 1000.0,
            slices + 1,
            ff.pops
        );
    }
}

fn gun() -> WeaponObs {
    let mut w = WeaponObs { kind: "gun".into(), ammo: Some(12.0), reload: Some(0.0), ..Default::default() };
    for (k, v) in [("damage", 45.0), ("cooldown", 0.24), ("speed", 252.0), ("range", 230.0), ("spread_deg", 0.6), ("mag", 12.0), ("reload", 2.4)] {
        w.stats.insert(k.into(), v);
    }
    w
}

struct RunResult {
    died_at: Option<f64>,
    stuck_events: u32,
    max_stuck_s: f64,
    out_s: f64,
    zone_dmg: f64,
    final_d: f64,
    stuck_spots: Vec<[f64; 3]>,
    max_ms: f64,
}

fn run_nav(w: &'static World, cfg: &Config, seed: u64, verbose: bool) -> RunResult {
    let ph = Phys::from(cfg);
    let mut rng = Rng::new(seed);
    let bnd = if std::env::var("WILD_ZONE").is_ok() { None } else { Some([w.bounds[0] as f64, w.bounds[1] as f64, w.bounds[2] as f64, w.bounds[3] as f64]) };
    let zs = ZoneSched::new(cfg, &mut rng, bnd);
    // spawn: random standable ground cell within 0.8 * start radius
    let (sc, sr) = zs.circles[0];
    let mut spawn = [0.0; 3];
    for _ in 0..10000 {
        let a = rng.f() * std::f64::consts::TAU;
        let r = sr * 0.8 * rng.f().sqrt();
        let x = sc[0] + a.cos() * r;
        let y = sc[1] + a.sin() * r;
        if x < w.bounds[0] as f64 + 30.0 || x > w.bounds[1] as f64 - 30.0 || y < w.bounds[2] as f64 + 30.0 || y > w.bounds[3] as f64 - 30.0 {
            continue;
        }
        if let Some(c) = w.cell_of(x as f32, y as f32) {
            if w.standable(c) && w.flags[c] & xero_mech::world::PRISM_FLOOR == 0 {
                spawn = [x, y, w.floor[c] as f64];
                break;
            }
        }
    }
    let mut bot = Bot::new(cfg.clone(), Some(w), 0, seed);
    let mut b = Body { pos: spawn, vel: [0.0; 3], grounded: true, energy: 100.0, since_spend: 0.0 };
    let mut hp = 1400.0;
    let mut aim = [0.0f64, 0.0];
    let mut last_dmg_t = -99.0;
    let mut res = RunResult { died_at: None, stuck_events: 0, max_stuck_s: 0.0, out_s: 0.0, zone_dmg: 0.0, final_d: 0.0, stuck_spots: vec![], max_ms: 0.0 };
    let dt = 0.05;
    let trace = std::env::var("TRACE").is_ok();
    let mut tick = 0u64;
    let mut act = Action::default();
    let mut prog_pos = spawn;
    let mut prog_t = 0.0;
    let mut cur_stuck = 0.0f64;
    while (tick as f64) * dt < 470.0 {
        let t = tick as f64 * dt;
        let zone = zs.obs(t);
        let (zc, zr) = zone.at(t);
        let in_zone = ((b.pos[0] - zc[0]).powi(2) + (b.pos[1] - zc[1]).powi(2)).sqrt() <= zr;
        if tick % 2 == 0 {
            let mut o = Obs { kind: "obs".into(), tick: tick as f64, t, alive: 16.0, zone: zone.clone(), ..Default::default() };
            o.me = SelfObs {
                id: 0.0,
                pos: b.pos,
                vel: b.vel,
                aim,
                hp,
                max_hp: 1400.0,
                energy: b.energy,
                max_energy: 100.0,
                grounded: b.grounded,
                in_zone,
                weapons: vec![Some(gun()), None],
                ..Default::default()
            };
            let t0 = Instant::now();
            act = bot.act(&o, t0 + Duration::from_millis(18));
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            res.max_ms = res.max_ms.max(ms);
            if trace && (tick % 10 == 0 || (std::env::var("TRACE_FROM").ok().and_then(|s| s.parse::<f64>().ok()).map(|f| t >= f && t < f + 3.0).unwrap_or(false))) {
                eprintln!(
                    "t={:.2} pos=({:.3},{:.3},{:.3}) v=({:.2},{:.2},{:.2}) g={} e={:.0} mv=({:.2},{:.2}) b={} j={} {} zone=({:.0},{:.0},{:.0})",
                    t, b.pos[0], b.pos[1], b.pos[2], b.vel[0], b.vel[1], b.vel[2], b.grounded as u8, b.energy,
                    act.r#move[0], act.r#move[1], act.boost as u8, act.jump as u8, bot.label, zone.next_center[0], zone.next_center[1], zone.next_radius
                );
            }
        }
        // torso slew
        let dyaw = xero_mech::bot::wrap(act.aim[0] - aim[0]);
        let mx = 4.0 * dt;
        aim[0] = xero_mech::bot::wrap(aim[0] + dyaw.clamp(-mx, mx));
        aim[1] += (act.aim[1] - aim[1]).clamp(-3.0 * dt, 3.0 * dt);
        step(w, &ph, &mut b, act.r#move, act.boost, act.jump && tick % 2 == 0, dt);
        if !in_zone {
            hp -= zone.dps * dt;
            res.out_s += dt;
            res.zone_dmg += zone.dps * dt;
            last_dmg_t = t;
        } else if t - last_dmg_t > 5.0 {
            hp = (hp + 45.0 * dt).min(1400.0);
        }
        if hp <= 0.0 {
            res.died_at = Some(t);
            break;
        }
        // progress tracking: wants to move but hasn't for a while
        let want = act.r#move[0].hypot(act.r#move[1]) > 0.3;
        if ((b.pos[0] - prog_pos[0]).powi(2) + (b.pos[1] - prog_pos[1]).powi(2)).sqrt() > 3.0 || !want {
            if cur_stuck > 3.0 {
                res.stuck_events += 1;
                res.stuck_spots.push(prog_pos);
                if verbose {
                    eprintln!("  seed {seed}: stuck {:.1}s at {:.1},{:.1},{:.1} t={:.1}", cur_stuck, prog_pos[0], prog_pos[1], prog_pos[2], t);
                }
            }
            prog_pos = b.pos;
            prog_t = t;
            cur_stuck = 0.0;
        } else {
            cur_stuck = t - prog_t;
            res.max_stuck_s = res.max_stuck_s.max(cur_stuck);
        }
        tick += 1;
    }
    if cur_stuck > 3.0 {
        res.stuck_events += 1;
        res.stuck_spots.push(prog_pos);
    }
    let fc = zs.circles.last().unwrap().0;
    res.final_d = ((b.pos[0] - fc[0]).powi(2) + (b.pos[1] - fc[1]).powi(2)).sqrt();
    res
}

fn nav(runs: u64, seed0: u64, verbose: bool) {
    let cfg = default_config();
    let w: &'static World = Box::leak(Box::new(load(&cfg)));
    let mut died = 0;
    let mut total_stuck = 0;
    let mut worst_ms: f64 = 0.0;
    let mut out_total = 0.0;
    for k in 0..runs {
        let seed = seed0 + k;
        let r = run_nav(w, &cfg, seed, verbose);
        worst_ms = worst_ms.max(r.max_ms);
        out_total += r.out_s;
        total_stuck += r.stuck_events;
        if r.died_at.is_some() {
            died += 1;
        }
        println!(
            "seed {seed}: died {:?} stuck_events {} max_stuck {:.1}s out_of_zone {:.1}s zone_dmg {:.0} final_d {:.1} max_ms {:.1}",
            r.died_at.map(|t| (t * 10.0).round() / 10.0),
            r.stuck_events,
            r.max_stuck_s,
            r.out_s,
            r.zone_dmg,
            r.final_d,
            r.max_ms
        );
        for s in r.stuck_spots.iter().take(3) {
            println!("    stuck spot {:.1},{:.1},{:.1}", s[0], s[1], s[2]);
        }
    }
    println!("SUMMARY runs {runs} died {died} stuck_events {total_stuck} out_of_zone_total {out_total:.1}s worst_decision_ms {worst_ms:.1}");
}

fn probe(x: f64, y: f64, z: f64, mx: f64, my: f64) {
    let cfg = default_config();
    let w = load(&cfg);
    let ph = Phys::from(&cfg);
    for dj in -4i32..=4 {
        let mut line = String::new();
        for di in -4i32..=4 {
            let c = w.cell_of(x as f32 + di as f32, y as f32 + dj as f32).unwrap();
            line += &format!("{:6.1}{}{} ", w.floor[c], if w.standable(c) { 's' } else { '.' }, if w.flags[c] & xero_mech::world::HAS_IV != 0 { 'P' } else { ' ' });
        }
        println!("{line}");
    }
    let c = w.cell_of(x as f32, y as f32).unwrap();
    println!("center ivs {:?} terr {} ground {}", w.ivs(c), w.terr[c], w.ground(x as f32, y as f32));
    let mut b = Body { pos: [x, y, z], vel: [0.0; 3], grounded: false, energy: 100.0, since_spend: 0.0 };
    for k in 0..30 {
        println!("{k} pos ({:.2},{:.2},{:.2}) vel ({:.2},{:.2},{:.2}) g {} collides {}", b.pos[0], b.pos[1], b.pos[2], b.vel[0], b.vel[1], b.vel[2], b.grounded, collides(&w, &ph, b.pos[0], b.pos[1], b.pos[2]));
        step(&w, &ph, &mut b, [mx, my], false, false, 0.05);
    }
}

/// FFA tournament: `ours` copies of our bot vs scripted opponents, `runs` matches.
fn ffa(runs: u64, seed0: u64, ours: usize, mix: &str) {
    use xero_mech::ffa::{Ffa, Kind};
    let cfg = default_config();
    let w: &'static World = Box::leak(Box::new(load(&cfg)));
    // all simulated bots share finished flow fields (identical zone discs give identical fields)
    xero_mech::nav::shared::enable(std::env::var("SIM_NO_SHARE").is_err());
    let mut total = 0.0;
    let mut n_ours: f64 = 0.0;
    let mut zone_deaths = 0;
    let mut wins = 0;
    let mut place_hist = vec![0u32; 17];
    let mut seen_total = 0.0;
    let mut alive_total = 0.0;
    let mut acc_s = [0u32; 3];
    let mut acc_h = [0u32; 3];
    let mut opp_s = [0u32; 3];
    let mut opp_h = [0u32; 3];
    let mut lz = [0u32; 2];
    let t_start = Instant::now();
    for k in 0..runs {
        let seed = seed0 + k;
        let mut kinds = Vec::new();
        let nmech = if mix.starts_with("duel") || mix == "dummy" { 2 } else { 16 };
        for i in 0..nmech {
            if i < ours {
                kinds.push(Kind::Ours);
            } else {
                let opp = match mix {
                    "example" => Kind::Example,
                    "rusher" => Kind::Rusher,
                    "camper" => Kind::Camper,
                    "aggro" => Kind::Aggro,
                    "squad" | "duel" => Kind::Squad,
                    "squad2" => Kind::Squad2,
                    "duelg" => Kind::Aggro,
                    "dummy" => Kind::Dummy,
                    "hard" => if i % 2 == 0 { Kind::Aggro } else { Kind::Rusher },
                    _ => match i % 3 {
                        0 => Kind::Example,
                        1 => Kind::Rusher,
                        _ => Kind::Camper,
                    },
                };
                kinds.push(opp);
            }
        }
        // rotate our slot(s) among spawn positions
        let rot = (seed as usize) % nmech;
        kinds.rotate_right(rot);
        let mut f = Ffa::new(w, cfg.clone(), seed, &kinds);
        let places = f.run();
        let mut line = format!("seed {seed}:");
        for (i, m) in f.mechs.iter().enumerate() {
            if m.kind == Kind::Ours {
                total += 16.0 - places[i] as f64;
                n_ours += 1.0;
                place_hist[places[i].min(16)] += 1;
                if places[i] == 1 {
                    wins += 1;
                }
                if m.killed_by_zone && m.alive == false && m.death_t < 414.0 {
                    zone_deaths += 1;
                }
                let killer = m.killer.map(|k| format!("{:?}#{}", f.mechs[k].kind, k)).unwrap_or_else(|| "-".into());
                let mut modes: Vec<(String, f64)> = m.modes.iter().map(|(k, v)| (k.clone(), *v)).collect();
                modes.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
                let ms: Vec<String> = modes.iter().take(4).map(|(k, v)| format!("{k}:{:.0}", v)).collect();
                seen_total += m.seen_s;
                alive_total += m.alive_s;
                for b in 0..3 {
                    acc_s[b] += m.shots[b];
                    acc_h[b] += m.hits[b];
                }
                lz[0] += m.laser_shots;
                lz[1] += m.laser_hits;
                for (j, e) in f.mechs.iter().enumerate() {
                    if j != i && e.kind != Kind::Ours {
                        for b in 0..3 {
                            opp_s[b] += e.shots[b];
                            opp_h[b] += e.hits[b];
                        }
                    }
                }
                line += &format!(
                    " ours#{i} seen {:.0}/{:.0}s modes {} |",
                    m.seen_s,
                    m.alive_s,
                    ms.join(",")
                );
                line += &format!(
                    " place {} by {} [{}] death {:.0}s{} kills {} dealt {:.0} taken {:.0} zone {:.0} |",
                    places[i],
                    killer,
                    m.mode_at_death,
                    if m.alive { f.t } else { m.death_t },
                    if m.killed_by_zone { "(zone)" } else { "" },
                    m.kills,
                    m.dmg_dealt,
                    m.dmg_taken,
                    m.zone_dmg
                );
            }
        }
        println!("{line}");
    }
    println!("SEEN fraction {:.3}", seen_total / alive_total.max(1e-9));
    let pct = |h: u32, s: u32| if s == 0 { 0.0 } else { 100.0 * h as f64 / s as f64 };
    println!(
        "ACCURACY ours gun <60 {:.0}% ({}) <120 {:.0}% ({}) >=120 {:.0}% ({}) laser {:.0}% ({}) | opponents gun <60 {:.0}% <120 {:.0}% >=120 {:.0}%",
        pct(acc_h[0], acc_s[0]), acc_s[0], pct(acc_h[1], acc_s[1]), acc_s[1], pct(acc_h[2], acc_s[2]), acc_s[2], pct(lz[1], lz[0]), lz[0],
        pct(opp_h[0], opp_s[0]), pct(opp_h[1], opp_s[1]), pct(opp_h[2], opp_s[2])
    );
    println!(
        "FFA SUMMARY runs {runs} ours/match {ours} mix {mix}: mean score {:.2} wins {wins} early-zone-deaths {zone_deaths} places {:?} ({:.1}s)",
        total / n_ours.max(1.0),
        &place_hist[1..],
        t_start.elapsed().as_secs_f64()
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("stats") => stats(),
        Some("ffa") => {
            let runs = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);
            let seed = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
            let ours = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(1);
            let mix = args.get(5).cloned().unwrap_or_else(|| "mixed".into());
            ffa(runs, seed, ours, &mix)
        }
        Some("probe") => {
            let f: Vec<f64> = args[2..].iter().filter_map(|s| s.parse().ok()).collect();
            probe(f[0], f[1], f[2], *f.get(3).unwrap_or(&0.0), *f.get(4).unwrap_or(&0.0))
        }
        Some("nav") => {
            let runs = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
            let seed = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
            let verbose = args.iter().any(|a| a == "-v");
            nav(runs, seed, verbose)
        }
        _ => eprintln!("usage: sim stats | sim nav [runs] [seed] [-v]"),
    }
}
