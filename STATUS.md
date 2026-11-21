# STATUS: xero-mech (Mech Royale)

_Last updated: Fri 2026-11-20 night (v5.4)_

## Versions
| Version | Commit | Practice (48 vs house squad) | Notes |
| --- | --- | --- | --- |
| v1 | 232756c | **2.63** (rank 45/51) | first robust version: nav + basic combat |
| v2 | 686ba81 | **2.67** (rank 48/79) | concealment hold spots, canopy model, engagement policy, loot/stuck fixes |
| v3 / v3.1 | 4fded0d / afe5742 | **3.00** (A) | laser-first loot, persistent cover + re-chosen hold spot, bullet/mortar dodging |
| v3.1-duel | 81cb33a (branch variant-duel) | **3.19** (B) | same + `ENV XERO_TUNE=solo_duel=1` |
| v4 | 8dfdbd6 | **3.35** | blind return fire (ghost tracks), laser-charge ducking, wall-at-back hold scanning, pickup memory + travel sweep |
| v4.1 | 51c8fd4 | **2.98** | v4 + solo duel ON + proactive solo engagement (HP>70%) + NPC drone farming for cubes |

## Submittable?
**Yes.** The `Dockerfile` at the top level builds the Rust bot and passes the sandboxed `check.py`.

## Conformance (check.py), v4.1
| Run | Result | answer time ms (p50 / p90 / p99 / max) |
| --- | --- | --- |
| Local (Windows release exe) | PASS 300/300 in time | 0.2 / 0.5 / 14.3 / 14.4 |
| Docker sandbox (`--network none --cpus 1 --memory 512m --read-only --tmpfs /tmp --user 65534:65534`) | PASS 300/300 in time, 0 late | 1.0 / 1.5 / 15.7 / 16.6 |

- `ready` arrives in ~0.4 s in Docker. The world build takes ~170 ms.
- The p99 comes from the time-sliced Dijkstra flow field, which runs last in each decision and
  stops at 14 ms after the observation arrived. Normal decisions take ~0.1-1 ms.

| v5 | 5b3579f | **3.75, 3.90** (two runs) | v4 behaviour + aiming execution fixes (v4.1 policy changes off by default) |
| v5-duel | 2ff5d30 | 3.52 | v5 + solo duel |
| v5-strafe | eb68039 | 3.06 | v5 + faster strafe reversals |
| v5.0.1 | 50ea7de | (same behaviour as v5) | optional knobs: tac_reload, cover_heal_hp, shoot_close |
| v5 | 5b3579f | 3.75 / 3.90 / 3.67 | (re-runs: practice noise ~0.1-0.15) |
| v5.1 | 127088b | rejected (archive > 20 MB: build dirs were committed by mistake) | v5 + trigger timing fix |
| v5.2-clean | c65dc6e | 3.81 | v5.1 + latency-compensated gun lead (build output removed) |
| v5.2-slowstrafe | 8881ad3 | 3.69 | v5.2 + strafe_min=2,strafe_max=4 |
| master cleanup | 301c9c6 | - | build output removed from the tree; `/target*/` ignored |
| v5.3 | 3fdf07e | - | more XERO_TUNE switches (defaults = v5.2) |
| v53-avoid110 | 6c12301 | pending | v5.3 + avoid_r=110 |
| v53-gunfar100 / v53-gunfar240 | 1212471 / fd86015 | pending | v5.3 + gun_far=100 / 240 (experiments) |
| v54-avoidsnd | 783b6dd | pending | e5984a2 + avoid_r=110,avoid_sounds=1 |
| v54-avoid200 | c5416cb | pending | e5984a2 + avoid_r=200,avoid_sounds=1 |
| **v5.4** | see `git log` (the "v5.4:" commit) | - | **avoidance as default: avoid_r=200, avoid_sounds on** (same behaviour as v54-avoid200) |

## Enemy avoidance (v53-avoid110, v54-avoidsnd)
Cause-of-death diagnostics in the practice-like sims:
- Most deaths are multi-attacker: 82% in squad2 have 2+ distinct attackers in the last 10 s.
- Lasers do 60-69% of the killing damage.
- The first hit comes early (median 9-17 s).
- About 77% of deaths happen while retreating.
The extra attackers are mostly enemies we walk near. Avoidance: when not fighting, steer away from
any known non-engaging enemy within `avoid_r` (never toward them, staying in the zone), and
penalise hold spots near known enemies. `avoid_sounds` also treats footsteps (<= 40 m) and nearby
gunfire/laser sounds from the last 3 s as threats at their bearing.
Sim, fresh seeds, vs v5.2 behaviour (512 squad / 256 aggro matches):

| Config | squad2 | squad3 | aggro |
| --- | --- | --- | --- |
| v5.2 | 1.18 | 7.27 | 8.80 |
| avoid_r=110 | 1.43 | 7.60 | 9.11 |
| avoid_r=110 + avoid_sounds | **1.48** | **7.65** | **9.14** |

On an earlier seed set avoid_r=110 was only +0.09 / +0.14 / -0.09, so the true effect is probably
about +0.2-0.3.

Radius sweep (avoid_sounds on, 512 squad / 256 FFA matches per cell, two seed sets for 110-200):

| Radius | squad2 | squad3 | aggro | mixed |
| --- | --- | --- | --- | --- |
| 0 (v5.2) | ~1.18 | ~7.3 | 9.48 | 11.82 |
| 110 | 1.42 / 1.48 | 7.91 / 8.03 | | |
| 150 | 1.65 / 1.62 | 7.97 / 8.10 | | |
| 200 | 1.76 / 1.73 / 1.81 | 7.99 / 8.05 / 8.11 | **10.14** | **12.01** |
| 260 | 1.69 | 8.27 | 10.31 | 12.22 |
| 330 | 1.63 | 8.25 | 10.56 | 11.91 |

200 m was picked as the default (best in squad2, which is structurally closest to practice, and
strong in the FFA mixes). Conformance (server, sandboxed docker, 1 pinned core): PASS 300/300,
answer ms p50 0.6 / p90 1.0 / p99 15.0 / max 16.7. Negative or neutral and not shipped:
- Rooftop holding: worse everywhere, even vs non-climbing opponents.
- Kill-spot looting, slew-credited fire check, preferred-range changes, tactical reload,
  heal-in-cover.
- Solo duel and drone fight-back: clearly negative.

## Changes in v5.2 (vs v5.1)
- **Latency-compensated gun lead.** Shots leave 1-2 ticks (0.05-0.1 s) after the observed state.
  The gun intercept is now solved from the target's position advanced by 0.05 s and from our own
  muzzle advanced by our velocity. The laser release aim also accounts for our motion.
  (`gun_lat=0.05`, `self_comp=1`.)
- Sim (vs v5.1): 1v1 vs gun hunter 56.1% -> 58.0% wins, squad3 7.27 -> 7.40.
- Tuning checked and left unchanged:
  - Gun tolerance: 2 m is near the optimum. 1.2 m costs 16 points of duel win rate; 3 m is equal.
  - Laser tolerance 1.0-2.4 m: equal.
  - Firing at moving targets beyond 140 m: neutral to slightly positive (kept at 140 for now).
- Conformance (server, sandboxed docker, 1 pinned core): PASS 300/300, answer ms p50 0.4 / p90 0.7
  / p99 14.9 / max 15.1.

## Changes in v5.1 (vs v5)
- **Trigger timing fix (execution bug).** We used to pull the trigger only when a weapon's cooldown
  was already 0 at decision time. Decisions come every 0.1 s and the gun's cooldown is 0.24 s, so a
  shot that would become available mid-window waited for the next decision. The gun effectively
  fired every ~0.3 s instead of 0.24 s, about 20% less DPS. The server fires as soon as the cooldown
  ends while the trigger is held. Now we hold the trigger when the cooldown ends within the
  decision window. The same applies to laser charge start, lightning and mortar.
- Sim A/B (fire_early off -> on): 1v1 vs gun hunter 46.0% -> **56.3%** wins (1024 matches each,
  about 7 SE). squad3 7.18 -> 7.35, squad2 1.10 -> 1.17, aggro 9.10 -> 8.88 (under 1 SE).
- Conformance (server, sandboxed docker, 1 pinned core): PASS 300/300, answer ms p50 0.6 / p90 1.1
  / p99 15.1 / max 15.6.

Sim lessons since v5 (512 matches per config unless noted):
- Solo duel: -0.39 squad3, -0.72 aggro (practice agreed: 3.52 vs 3.75).
- NPC drone fight-back: -0.68 aggro. Never retreating: -1.3 aggro.
- Not initiating close fights: -0.15 squad2.
- Tactical reload, heal-in-cover, other retreat thresholds, and strafe frequency (even against
  non-leading opponents): neutral.
- Rear damage is 10-14% of damage taken.

## Changes in v5 (vs v4; the v4.1 policy changes are switched off again)
Practice ranked v4 (3.35) above v4.1 (2.98), so v5 keeps v4's behaviour: solo duel off, no proactive
engagement, no drone farming or drone fight-back. All of those remain available as `XERO_TUNE`
switches. The changes are pure execution fixes, found with clean accuracy stats from the simulator:
- **LOS-aware aim point**: aim at the first body point (center, +2 m, +3.6 m) with a clear line. At
  range, terrain often hides the lower body, and shots at the center hit the ground.
- **Laser/gun aim hand-off**: the torso follows the gun's lead aim while the laser charges, then
  switches to the laser aim (with a 75 ms release lead) once the charge is 80% full. The release
  tolerance is tightened to ~1.6 m at the target. Laser accuracy in sim laser duels went from 31% to ~65%.
- **No wasted laser shots**: a charge is held, not dumped, when the target drops out of view.
  During peek-fighting the bot steps out before the server's auto-release would fire it into the wall.
- **No gun spam beyond 140 m at strafing targets** (sim hit rate there ~10%). This saves the
  magazine for closer range.
- World: LOS uses bilinear terrain instead of the nearest vertex.
- Sim: frozen opponent copy (`src/oppbot.rs`) so our changes don't alter the opponents. Practice-like
  `squad2` mode, NPC drones + cubes, accuracy stats, shared flow fields, server scripts
  (`tools/inner_bench.sh`, `tools/server_bench.sh`).

Conformance (server, sandboxed docker, 1 pinned core): PASS 300/300, answer ms p50 0.4 / p90 0.7 /
p99 14.7 / max 17.1.

## Changes in v4.1 (vs v4)
**Practice score is now the gate (top 32 of 93 qualify).** Practice ≈ number of hunters that die
before us, so this version is tuned for duels.
- **Solo duel ON by default.** When at most one enemy is known around us (seen within 4 s, or hit us
  within 6 s), fight it head-on: no cover runs, retreat only below 25% HP. Real A/B: 3.19 vs 3.00.
- **Proactive solo engagement.** With HP above 70%, attack a lone visible enemy within 220 m first
  instead of waiting for its first shot.
- **NPC drones.** Kill drones that shoot us. When calm (no mech seen for 6 s, HP > 60%, t < 300 s),
  farm drones within 70 m: each gives 2 cubes, i.e. +8% max HP and damage. This used to be ignored completely.
- FFA sim (now with 12 NPC drones and cubes): neutral for FFA (aggro 9.42 vs 9.39, mixed 10.99 vs
  11.08 over 240/200 matches) and more wins vs mixed (83 vs 73).
- Sim: NPC drones (leash/chase/shoot), cubes (+4% HP/damage, pickup radius, death drops),
  `squad2` practice-like squad (graded aim noise, weakest hunts first, lasers from grade 4).

## Changes in v4 (vs v3.1)
- **Blind return fire / ghost tracks.** Duel traces showed the hunter killing us through tree
  canopy. Trees block sight but not shots, so it could hit us while we couldn't see it. Laser and
  lightning `beams` give the shooter's muzzle position, and `damaged` bearings give its direction.
  Both create ghost tracks that we aim at and shoot (buildings still block, trees don't).
- **Duck laser telegraphs.** When a visible enemy's laser is `charging` and aimed at us (within
  ~6 m at our range), move to the nearest spot out of its LOS. The peek logic hides while their
  laser charges, steps out to shoot during their cooldown, and pre-charges our own laser in cover.
- **Back to the wall.** Hold spots record the open direction (away from walls within 9 m). While
  holding, the torso sweeps +-70 deg around it instead of spinning 360 deg, so rear hits (x1.5) come
  only through the wall side.
- **Pickup memory + travel sweep.** Weapon pickups seen once are remembered (forgotten when we're
  within 30 m and don't see them, or after a `took`/`gone`). While travelling, the torso sweeps
  +-60 deg around the heading to spot loot and threats.
- Sim: pickups follow the vision rules; laser beams are emitted; per-tick duel trace (`FFA_TRACE=1`).

## Changes in v3 (vs v2)
- **Loot priority laser > lightning > mortar.** The recorded practice stream shows the house hunter
  arriving with gun + laser and fighting at 100-150 m. In the sim duel vs such a hunter, our win
  rate is 8% with gun only, 30% with a laser and 6% with lightning (the hunter never comes within 45 m).
- **Fight back at range** when we carry a laser too and our HP is at least 90% of theirs.
- **Persistent cover**: when shot from beyond fight range, pick a spot out of the threat's LOS and
  stay there for 8 s after the last hit (regen starts after 5 s). Afterwards the hold spot is
  re-chosen instead of walking back into the exposed one.
- **Dodging**: predict mortar-shell impacts (ballistic integration to ground/roof) and step out of
  the splash radius. Sidestep bullets whose closest approach is under 3 m in 0.12-0.8 s.
- Sim: squad mode (hunters with lasers, teammates don't fight, +1 hunter per kill, all hunt after
  300 s), 1v1 duel modes (`duel` = laser hunter, `duelg` = gun hunter), lightning "short" effect.

## Changes in v2 (vs v1)
- **Canopy model**: exact segment-vs-ellipsoid crown path length from `world.json` trees. A sight
  line is blocked by more than ln(1/canopyBlockT)/canopyDensity ≈ 4 m of foliage (beyond the 45 m
  near range).
- **Concealed hold spots**: among cells reachable on foot (BFS, 45 m), score how many of 16 far
  viewpoints (8 directions x 70/120 m) can't see us, plus wall proximity and canopy. The search is
  deadline-aware.
- **Engagement policy** (tunable `Tune`): fight when <55 m, when attacked and healthy within 90 m,
  or when the enemy is weak. Otherwise, against a far attacker, go to cover (break LOS) and shoot
  back while moving. Retreat to cover at low HP.
- **Fixes**:
  - Stuck detection no longer fires while strafing in a fight. It now compares actual speed with
    commanded speed.
  - Loot: weapon swaps send `drop`; a loot target that takes more than 14 s is blacklisted.
  - "Go direct" shortcut no longer skips roof climbs.
  - A hold spot that gets the bot stuck is replaced by the current cell.

## Simulators
- `sim nav N SEED`: navigation smoke test (one mech, approximate physics, server-like zone).
  v2: 30/30 runs alive until the final circle closes, 9 stuck events (max 5.1 s), worst decision 20.6 ms.
- `sim ffa N SEED OURS MIX` and `tools/bench.sh RUNS PROCS OURS MIX [SEED]` (parallel). Mixes:
  - `mixed`: kit-example-like / rusher / camper scripted bots.
  - `aggro`: 15 copies of our own bot with an aggressive tune. These navigate well, lead shots,
    and engage anything within 200 m.

| Bench | v1 | v2 | v3 | v4 |
| --- | --- | --- | --- | --- |
| 1 ours vs mixed, 200 matches | 8.85 (60 matches) | 9.67 (44 wins) | 10.75 (65 wins) | **10.86** (67 wins) |
| 1 ours vs 15 aggro, 100 matches | 7.94 (80 matches) | 8.69 | 9.40 | |
| 1 ours vs 15 aggro, 240 matches, seeds 60000 | | | 9.17 | **9.60** |
| 1v1 vs gun hunter (`duelg`), win % | | | 46% |
| 1v1 vs laser hunter (`duel`), win %, gun only | | | 8% (30% with a laser) |

(Random placement = 7.5. The SE of the mean is about 0.35 at 200 matches and 0.45 at 100.)

## Known weaknesses / TODO
- Practice is a duel test: the squad sends 1 hunter at first, more as we kill them, all of them
  late. Squad bots don't kill each other, so points ≈ hunters that die before us. The **duel**
  needs improving: weapon pickups early (lightning/laser), cubes, cover use.
- No projectile dodging and no dodging of enemy laser telegraphs yet.
- The mortar is not modelled in the FFA sim.
- Upgrade choice is a fixed priority list (plating > armor > nanites > ...).
