# xero-mech: a survival-first Mech Royale bot

Team-Xero's bot for the Builders Buildathon **Mech Royale** track (protocol `mechfight/1`).
Score is placement only (16 - place), so the bot is built to **outlive** the lobby rather than
top the kill feed: it never gets stuck on walls, never loses to the zone, avoids early fights,
fights back hard when engaged, and arrives at the final circle with as much HP as possible.

Written in Rust: one static binary, about 0.3 ms per decision, with heavy work time-sliced to
stay under an 18 ms per-decision budget (the server allows 50 ms).

## How it works

### World model (`src/world.rs`)
Built from the arena files at `hello` (~250 ms; the image also carries a copy of the map as a
fallback when `hello.map.dir` is missing):

- **Terrain**: int16 cm heights sampled bilinearly.
- **Prisms** (buildings, stadium, bridges): scanline-rasterized onto a 1 m grid aligned with the
  terrain vertices. Each cell keeps the vertical intervals of the solids above it.
- **Floor layer**: for each cell, the lowest surface a 9 m mech can stand on (terrain, a roof or a
  bridge deck), with head clearance. High bridges are walked under; low overhangs count as walls.
- **Standability**: a cell is standable when no obstruction (anything rising more than the 1.2 m
  `step_up`, a low overhang, or the map edge) lies within 3.2 m, which is the mech radius plus margin.
- **Jump edges**: from cells next to a wall, the bot looks 16 directions for the first standable
  cell across the wall band. Climbs up to 45 m become jump+jet edges, weighted by climb height.
  Drops off roofs become cheap edges. That lets paths go onto and off roofs.
- **LOS**: segment sampling against the cell intervals and terrain, used for shots and cover.
- **Canopy cover** per cell, from tree crowns, used to pick hiding spots.

### Navigation (`src/nav.rs`)
- **Zone flow field**: a multi-source Dijkstra seeded from every standable cell in an inner disc
  of the *next* circle, time-sliced across decisions (~120 ms total, about 8 slices). The mech
  follows `next[]` pointers from wherever it is, so knock-backs and detours need no replanning.
  If the disc holds too few standable cells (a circle on roofs or off the map), it grows.
- **Bounded A\*** with reusable dense scratch arrays, for short trips to loot, cover and hold spots.
- **Path following**: string-pulled lookahead along the path. Climb edges run a jump+jet
  maneuver: wait for enough energy, jet until the feet clear the roof, then move on. Drops just
  walk off.
- **Stuck recovery**: if the bot is commanding movement but making less than 18% of the expected
  progress, it jumps and jets, then side-steps, rotating through directions.
- **Final circle**: the last phase shrinks to a point. Only the moving center stays safe, so the
  bot tracks `c(t+1.5 s)` to within 0.3 m.

### Behaviour (`src/bot.rs`)
- **Rotate** into the next circle early, then **hold** a spot that is reachable on foot (BFS) and
  scored by **concealment**: how many of 16 far viewpoints cannot see us. The visibility model uses
  exact canopy (tree crowns as ellipsoids; more than ~4 m of foliage blocks sight beyond 45 m) and
  buildings. Hold spots prefer a wall at our back. The torso then sweeps only the open sector, so
  the rear (hits there do x1.5) faces the wall. The bot moves silently (< 5 m/s) near enemies.
- **Engagement policy** (`Tune`, all parameters overridable with `XERO_TUNE=key=val,...`):
  - **Solo duel**: when at most one enemy is around, fight it head-on. With HP above 70% the bot
    attacks a lone visible enemy first.
  - With several threats, use **cover**: break line of sight from far attackers and stay hidden
    8 s after the last hit (regen starts after 5 s), then re-choose the hold spot.
  - Fight anything within 55 m. Hunt weak enemies. Retreat to cover below 45% HP.
- **Peek-fighting against lasers**: hide while an enemy laser is `charging` (or held) and aimed at
  us, step out during its cooldown, and pre-charge our own laser behind cover.
- **Blind return fire**: laser beams reveal the shooter's muzzle, and damage events give a bearing.
  Both become "ghost" targets we shoot back at through trees (trees block sight, not shots).
- **Dodging**: predicted mortar impacts (ballistic integration) and incoming bullets.
- **Weapons**: gun with lead (intercept solve); laser charged then released on target; lightning
  in range and inside its cone; mortar on a ballistic solution, with a high arc over buildings.
  Loot priority is laser > lightning > mortar, and seen pickups are remembered.
- **Cubes**: kill NPC drones that attack us. Farm drones when calm (2 cubes each, +4% HP and damage
  per cube). Shoot crates early. Collect cubes.
- **Upgrades**: plating > armor > nanites > weapon upgrades > the rest.

### Timing / robustness (`src/main.rs`)
A reader thread queues stdin lines. The main loop answers only the newest observation, merging the
events, sounds and beams of skipped ones. `act()` runs under `catch_unwind`: if it panics, the bot
re-sends the last continuous action. The flow-field slice runs last, using whatever time is left
of the 18 ms budget.

## Local tools
```sh
cargo build --release
# conformance check (kit)
python3 <kit>/check/check.py -- target/release/xero-mech
# smoke simulator: approximate physics (accel, walls+sliding, step-up, slopes, gravity, jump/jet, energy)
# and a server-like zone schedule. One mech per run, random spawn.
ARENA=arena target/release/sim nav 30 1        # 30 matches from seed 1
ARENA=arena target/release/sim stats           # world build + flow field timing
ARENA=arena target/release/sim probe X Y Z     # dump cells around a point, simulate a fall
# 16-mech FFA simulator: vision (FOV/near/LOS/canopy/memory), sounds, guns with travel time,
# lasers, lightning, NPC drones, cubes, zone. Opponent mixes: mixed | aggro | squad | squad2 | duel | duelg
target/release/sim ffa 20 1000 1 aggro
tools/bench.sh 12 20 1 aggro 60000             # 20 parallel processes x 12 matches, prints the mean score
```

## Docker
```sh
docker build --platform linux/amd64 -t xero-mech .
python3 <kit>/check/check.py --map-dir /arena -- docker run --rm -i --network none --cpus 1 \
  --memory 512m --read-only --tmpfs /tmp:rw,size=64m --user 65534:65534 -v "<kit>/arena:/arena:ro" xero-mech
```

## License
MIT, Copyright (c) 2026 Team-Xero
