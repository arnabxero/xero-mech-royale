# STATUS: xero-mech (Mech Royale)

_Last updated: Fri 2026-11-20 night (v2)_

## Versions
| Version | Commit | Practice (48 vs house squad) | Notes |
| --- | --- | --- | --- |
| v1 | 232756c | **2.63** (rank 45/51) | first robust version: nav + basic combat |
| v2 | see `git log` (the "v2:" commit) | pending | concealment hold spots, canopy model, engagement policy, loot/stuck fixes |

## Submittable?
**Yes.** The `Dockerfile` at the top level builds the Rust bot and passes the sandboxed `check.py`.

## Conformance (check.py), v2
| Run | Result | answer time ms (p50 / p90 / p99 / max) |
| --- | --- | --- |
| Local (Windows release exe) | PASS 300/300 in time | 0.2 / 0.4 / 14.4 / 14.5 |
| Docker sandbox (`--network none --cpus 1 --memory 512m --read-only --tmpfs /tmp --user 65534:65534`) | PASS 300/300 in time, 0 late | 1.0 / 1.2 / 15.9 / 16.7 |

- `ready` arrives in ~0.4 s in Docker. The world build takes ~170 ms.
- The p99 comes from the time-sliced Dijkstra flow field, which runs last in each decision and
  stops at 14 ms after the observation arrived. Normal decisions take ~0.1-1 ms.

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

| Bench | v1 | v2 |
| --- | --- | --- |
| 1 ours vs mixed, 200 matches | 8.85 (60 matches) | **9.67** (44 wins) |
| 1 ours vs 15 aggro, 100 matches | 7.94 (80 matches) | **8.69** |

(Random placement = 7.5. The SE of the mean is about 0.35 at 200 matches and 0.45 at 100.)

## Known weaknesses / TODO
- Practice is a duel test: the squad sends 1 hunter at first, more as we kill them, all of them
  late. Squad bots don't kill each other, so points ≈ hunters that die before us. The **duel**
  needs improving: weapon pickups early (lightning/laser), cubes, cover use.
- No projectile dodging and no dodging of enemy laser telegraphs yet.
- The mortar is not modelled in the FFA sim.
- Upgrade choice is a fixed priority list (plating > armor > nanites > ...).
