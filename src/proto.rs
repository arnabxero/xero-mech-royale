//! mechfight/1 message types (lenient: every field has a default so a missing or added
//! field never breaks parsing).
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct WeaponObs {
    pub kind: String,
    pub cooldown: f64,
    pub ammo: Option<f64>,
    pub reload: Option<f64>,
    pub charge: Option<f64>,
    pub stats: HashMap<String, f64>,
}

impl WeaponObs {
    pub fn stat(&self, k: &str, d: f64) -> f64 {
        self.stats.get(k).copied().unwrap_or(d)
    }
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct SelfObs {
    pub id: f64,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub aim: [f64; 2],
    pub hp: f64,
    pub max_hp: f64,
    pub energy: f64,
    pub max_energy: f64,
    pub grounded: bool,
    pub cubes: f64,
    pub upgrades: Vec<String>,
    pub weapons: Vec<Option<WeaponObs>>,
    pub in_zone: bool,
    pub stats: HashMap<String, f64>,
}

impl SelfObs {
    pub fn stat(&self, k: &str, d: f64) -> f64 {
        self.stats.get(k).copied().unwrap_or(d)
    }
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct ActorObs {
    pub id: f64,
    pub kind: String,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub aim: [f64; 2],
    pub hp: Option<f64>,
    pub max_hp: Option<f64>,
    pub cubes: Option<f64>,
    pub weapons: Vec<String>,
    pub charging: bool,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct ProjObs {
    pub id: f64,
    pub kind: String,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct ItemObs {
    pub id: f64,
    pub pos: [f64; 3],
    pub hp: Option<f64>,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct PickupObs {
    pub id: f64,
    pub kind: String,
    pub pos: [f64; 3],
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct SoundObs {
    pub kind: String,
    pub bearing: f64,
    pub dist: String,
    pub age: f64,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct BeamObs {
    pub by: f64,
    pub kind: String,
    pub path: Vec<[f64; 3]>,
    pub age: f64,
}

/// All event kinds flattened into one lenient struct.
#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Event {
    #[serde(rename = "type")]
    pub kind: String,
    pub amount: Option<f64>,
    pub weapon: Option<String>,
    pub bearing: Option<f64>,
    pub by: Option<f64>,
    pub rear: Option<bool>,
    pub target: Option<f64>,
    pub effect: Option<String>,
    pub id: Option<Value>,
    pub cubes: Option<f64>,
    #[serde(rename = "kind")]
    pub wkind: Option<String>,
    pub slot: Option<f64>,
    pub reason: Option<String>,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct DropObs {
    pub id: f64,
    pub pos: [f64; 3],
    pub radius: f64,
    pub lands_in: f64,
    pub capture_s: f64,
    pub progress: f64,
    pub holder: Option<f64>,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct ZoneObs {
    pub center: [f64; 2],
    pub radius: f64,
    pub next_center: [f64; 2],
    pub next_radius: f64,
    pub shrink_start: f64,
    pub shrink_end: f64,
    pub dps: f64,
}

impl ZoneObs {
    /// Circle (center, radius) at match time t (linear interpolation in the shrink window).
    pub fn at(&self, t: f64) -> ([f64; 2], f64) {
        if t <= self.shrink_start {
            return (self.center, self.radius);
        }
        if t >= self.shrink_end || self.shrink_end <= self.shrink_start {
            return (self.next_center, self.next_radius);
        }
        let f = (t - self.shrink_start) / (self.shrink_end - self.shrink_start);
        (
            [
                self.center[0] + (self.next_center[0] - self.center[0]) * f,
                self.center[1] + (self.next_center[1] - self.center[1]) * f,
            ],
            self.radius + (self.next_radius - self.radius) * f,
        )
    }
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct OfferChoice {
    pub id: String,
    pub name: String,
    pub desc: String,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Offer {
    pub id: f64,
    pub expires: f64,
    pub choices: Vec<OfferChoice>,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Obs {
    #[serde(rename = "type")]
    pub kind: String,
    pub tick: f64,
    pub t: f64,
    pub alive: f64,
    #[serde(rename = "self")]
    pub me: SelfObs,
    pub mechs: Vec<ActorObs>,
    pub projectiles: Vec<ProjObs>,
    pub crates: Vec<ItemObs>,
    pub cubes: Vec<ItemObs>,
    pub pickups: Vec<PickupObs>,
    pub sounds: Vec<SoundObs>,
    pub beams: Vec<BeamObs>,
    pub events: Vec<Event>,
    pub drops: Vec<DropObs>,
    pub zone: ZoneObs,
    pub offer: Option<Offer>,
}

#[derive(Serialize, Default, Clone, Debug)]
pub struct DebugLine {
    pub a: [f64; 3],
    pub b: [f64; 3],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Serialize, Default, Clone, Debug)]
pub struct DebugPoint {
    pub at: [f64; 3],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
}

#[derive(Serialize, Default, Clone, Debug)]
pub struct DebugDraw {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<DebugLine>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<DebugPoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Serialize, Default, Clone, Debug)]
pub struct Action {
    pub tick: Value,
    pub r#move: [f64; 2],
    pub boost: bool,
    pub jump: bool,
    pub aim: [f64; 2],
    pub fire: [bool; 2],
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub reload: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pick: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub take: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug: Option<DebugDraw>,
}

/// Echo a tick exactly as an integer when it is one.
pub fn tick_value(t: f64) -> Value {
    if t.fract() == 0.0 && t.abs() < 9.0e15 {
        Value::from(t as i64)
    } else {
        Value::from(t)
    }
}

/// Typed accessors for hello.config with documented defaults.
#[derive(Clone, Debug)]
pub struct Config {
    pub raw: Value,
}

impl Config {
    pub fn new(raw: Value) -> Self {
        Config { raw }
    }
    pub fn f(&self, sec: &str, key: &str, d: f64) -> f64 {
        self.raw.get(sec).and_then(|s| s.get(key)).and_then(|v| v.as_f64()).unwrap_or(d)
    }
    pub fn w(&self, weapon: &str, key: &str, d: f64) -> f64 {
        self.raw
            .get("weapons")
            .and_then(|s| s.get(weapon))
            .and_then(|s| s.get(key))
            .and_then(|v| v.as_f64())
            .unwrap_or(d)
    }
    pub fn sound_range(&self, kind: &str, d: f64) -> f64 {
        self.raw
            .get("sound")
            .and_then(|s| s.get("range"))
            .and_then(|s| s.get(kind))
            .and_then(|v| v.as_f64())
            .unwrap_or(d)
    }
    pub fn drop_times(&self) -> Vec<f64> {
        self.raw
            .get("drops")
            .and_then(|d| d.get("times"))
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_f64()).collect())
            .unwrap_or_default()
    }
    /// Zone phases: (hold_s, shrink_s, radius, dps).
    pub fn zone_phases(&self) -> Vec<(f64, f64, f64, f64)> {
        self.raw
            .get("zone")
            .and_then(|z| z.get("phases"))
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .map(|p| {
                        let g = |k: &str| p.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0);
                        (g("hold_s"), g("shrink_s"), g("radius"), g("dps"))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}
