pub mod bot;
pub mod ffa;
pub mod nav;
pub mod proto;
pub mod world;
pub mod sim;

use std::path::{Path, PathBuf};

/// Load the world from the hello's map dir, falling back to the copy baked into the image.
pub fn load_world(map_dir: Option<&str>, p: world::Params) -> Option<&'static world::World> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = map_dir {
        dirs.push(PathBuf::from(d));
    }
    dirs.push(PathBuf::from("/arena"));
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.join("arena"));
            dirs.push(parent.join("../../arena"));
        }
    }
    dirs.push(PathBuf::from("/bot/arena"));
    dirs.push(PathBuf::from("arena"));
    for d in dirs {
        if !Path::new(&d).join("v2/world.json").exists() {
            continue;
        }
        match world::World::load(&d, p) {
            Ok(w) => return Some(Box::leak(Box::new(w))),
            Err(e) => eprintln!("[xero] map load from {} failed: {e}", d.display()),
        }
    }
    eprintln!("[xero] no map found: running without a world model");
    None
}

pub fn params_from(cfg: &proto::Config) -> world::Params {
    world::Params {
        radius: cfg.f("mech", "radius", 2.5) as f32,
        height: cfg.f("mech", "height", 9.0) as f32,
        step_up: cfg.f("mech", "step_up", 1.2) as f32,
        max_slope_deg: cfg.f("mech", "max_slope_deg", 50.0) as f32,
        max_climb: 45.0,
    }
}
