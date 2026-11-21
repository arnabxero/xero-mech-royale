//! xero-mech: mechfight/1 bot. Reads JSON lines on stdin, writes actions on stdout.
use serde_json::Value;
use std::io::{self, BufRead, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use xero_mech::bot::Bot;
use xero_mech::proto::{tick_value, Action, Config, Obs};

const NAME: &str = "xero-mech";

fn send(out: &mut impl Write, v: &impl serde::Serialize) {
    if let Ok(s) = serde_json::to_string(v) {
        let _ = out.write_all(s.as_bytes());
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    }
}

fn main() {
    let (tx, rx) = mpsc::channel::<Option<(Instant, String)>>();
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut lock = stdin.lock();
        let mut line = String::new();
        loop {
            line.clear();
            match lock.read_line(&mut line) {
                Ok(0) | Err(_) => {
                    let _ = tx.send(None);
                    break;
                }
                Ok(_) => {
                    if !line.trim().is_empty() {
                        let _ = tx.send(Some((Instant::now(), line.clone())));
                    }
                }
            }
        }
    });
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut bot: Option<Bot> = None;
    let mut last: Action = Action::default();
    let mut times: Vec<f64> = Vec::new();
    let budget_ms: f64 = std::env::var("XERO_BUDGET_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(14.0);
    loop {
        let first = match rx.recv() {
            Ok(Some(m)) => m,
            _ => break,
        };
        let mut batch = vec![first];
        let mut eof = false;
        loop {
            match rx.try_recv() {
                Ok(Some(m)) => batch.push(m),
                Ok(None) => {
                    eof = true;
                    break;
                }
                Err(_) => break,
            }
        }
        let mut latest: Option<(Instant, Obs)> = None;
        let mut carry_events = Vec::new();
        let mut carry_sounds = Vec::new();
        let mut carry_beams = Vec::new();
        let mut ended = false;
        for (at, raw) in batch {
            let v: Value = match serde_json::from_str(&raw) {
                Ok(v) => v,
                Err(_) => continue,
            };
            match v.get("type").and_then(|t| t.as_str()) {
                Some("hello") => {
                    let t0 = Instant::now();
                    let cfg = Config::new(v.get("config").cloned().unwrap_or(Value::Null));
                    let map_dir = v.get("map").and_then(|m| m.get("dir")).and_then(|d| d.as_str()).map(|s| s.to_string());
                    let slot = v.get("slot").and_then(|s| s.as_i64()).unwrap_or(0);
                    let seed = v.get("seed").and_then(|s| s.as_u64()).unwrap_or(1);
                    let built = catch_unwind(AssertUnwindSafe(|| {
                        let world = xero_mech::load_world(map_dir.as_deref(), xero_mech::params_from(&cfg));
                        Bot::new(cfg.clone(), world, slot, seed)
                    }));
                    match built {
                        Ok(mut b) => {
                            if let Ok(spec) = std::env::var("XERO_TUNE") {
                                b.tune.apply(&spec);
                                eprintln!("[xero] tune overrides: {spec}");
                            }
                            bot = Some(b)
                        }
                        Err(_) => {
                            eprintln!("[xero] bot construction panicked; running without world");
                            bot = Some(Bot::new(cfg, None, slot, seed));
                        }
                    }
                    eprintln!("[xero] ready after {:.0} ms", t0.elapsed().as_secs_f64() * 1000.0);
                    send(&mut out, &serde_json::json!({"type": "ready", "name": NAME}));
                }
                Some("obs") => match serde_json::from_value::<Obs>(v.clone()) {
                    Ok(o) => {
                        if let Some((_, prev)) = latest.take() {
                            carry_events.extend(prev.events);
                            carry_sounds.extend(prev.sounds);
                            carry_beams.extend(prev.beams);
                        }
                        latest = Some((at, o));
                    }
                    Err(e) => {
                        eprintln!("[xero] obs parse error: {e}");
                        if let Some(t) = v.get("tick").and_then(|t| t.as_f64()) {
                            let mut o = Obs::default();
                            o.tick = t;
                            o.kind = "bad".into();
                            latest = Some((at, o));
                        }
                    }
                },
                Some("end") => {
                    ended = true;
                }
                _ => {}
            }
        }
        if let Some((at, mut o)) = latest {
            let tick = o.tick;
            let mut action = None;
            if o.kind == "obs" {
                if let Some(b) = bot.as_mut() {
                    if !carry_events.is_empty() {
                        carry_events.extend(std::mem::take(&mut o.events));
                        o.events = carry_events;
                    }
                    if !carry_sounds.is_empty() {
                        carry_sounds.extend(std::mem::take(&mut o.sounds));
                        o.sounds = carry_sounds;
                    }
                    if !carry_beams.is_empty() {
                        carry_beams.extend(std::mem::take(&mut o.beams));
                        o.beams = carry_beams;
                    }
                    let deadline = at + Duration::from_micros((budget_ms * 1000.0) as u64);
                    match catch_unwind(AssertUnwindSafe(|| b.act(&o, deadline))) {
                        Ok(a) => action = Some(a),
                        Err(_) => eprintln!("[xero] act panicked at tick {tick}"),
                    }
                }
            }
            let mut a = match action {
                Some(a) => {
                    last = a.clone();
                    a
                }
                None => {
                    let mut a = last.clone();
                    a.fire = [false, false];
                    a.jump = false;
                    a.reload = false;
                    a.pick = None;
                    a.take = None;
                    a.drop = None;
                    a.debug = None;
                    a
                }
            };
            a.tick = tick_value(tick);
            send(&mut out, &a);
            times.push(at.elapsed().as_secs_f64() * 1000.0);
        }
        if ended || eof {
            break;
        }
    }
    if !times.is_empty() {
        let mut s = times.clone();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p = |q: f64| s[((s.len() - 1) as f64 * q) as usize];
        eprintln!("[xero] decisions {} p50 {:.2} p99 {:.2} max {:.2} ms", s.len(), p(0.5), p(0.99), p(1.0));
    }
}
