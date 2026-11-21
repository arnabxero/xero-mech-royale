# STATUS: xero-mech (Team-Xero, Mech Royale)

_Last updated: Fri 2026-11-20 evening (v1)_

## Submittable?
**Yes.** The `Dockerfile` at the top level builds the Rust bot and passes the sandboxed `check.py`.

## Conformance (check.py)
| Run | Result | answer time ms (p50 / p90 / p99 / max) |
| --- | --- | --- |
| Local (Windows, release exe), 14 ms slice | PASS 300/300 in time | 0.1 / 0.3 / 14.3 / 14.4 |
| Docker sandbox (`--network none --cpus 1 --memory 512m --read-only --tmpfs /tmp --user 65534:65534`), 18 ms slice | PASS 300/300 in time, 0 late | 0.9 / 1.2 / 19.7 / 21.1 |

- `ready` arrives in ~0.2 s locally and ~0.4 s in Docker. The world build takes ~250 ms.
- The p99 comes from the time-sliced Dijkstra flow field (it fills its slice budget). Normal
  decisions take ~0.1-0.6 ms.

## What works
- World model: terrain, prisms on a 1 m grid, floor layer (roofs, decks, walk-under bridges),
  standability with mech clearance, 180k jump edges (jet climbs up to 45 m, drops), LOS, canopy cover.
- Zone flow fields (time-sliced Dijkstra, ~120 ms total) to an inner disc of the next circle.
  The disc grows when the circle is on roofs or off the map.
- Path following with string pulling, climb maneuvers, and stuck recovery (jump+jet, then side-steps).
- Final phase: tracks the moving center of the last circle to within 0.3 m.
- Hold spots reachable on foot, chosen by BFS: next to a wall and/or under canopy.
- Combat: gun lead, lightning cone, laser charge/release, mortar ballistic (high arc without LOS),
  strafing, facing the attacker, retreating to cover at low HP.
- Loot (weapons, cubes), crates (shoot when calm and early), supply drops (only uncontested),
  and upgrade priorities.

## Smoke simulator (`target/release/sim nav N SEED`)
Approximate physics, one mech, random spawn, server-like zone schedule (centers kept on the map).
- 30/30 runs survive until the final circle closes (radius 0 at 415 s, when everyone takes 260 dps).
  Final distance to the end point is 0.1-0.4 m.
- Stuck events (>3 s without progress while moving): 10 across 30 matches, all recovered.
  The longest was 4.2 s.
- Worst decision in the sim is 18-24 ms (the flow-field slice).

## FFA simulator (`target/release/sim ffa N SEED OURS MIX`)
16 mechs. The scripted opponents are kit-example-like ("example"), "rusher" and "camper". Guns
have travel time and spread; vision uses FOV, near sensors, LOS, approximate canopy and 5 s
memory; lasers and lightning are floor loot. The opponents are dumb, so these numbers are optimistic.
- v1, 1 ours vs mixed, 60 matches: **mean score 8.85**, 12 wins.
- v1, 4 ours vs mixed, 30 matches: 8.83 per bot.
- v1, 1 ours vs rushers, 30 matches: 11.03.

## Known weaknesses / TODO
- Combat tuning is untested against real bots. Fight/flee thresholds are guesses.
- No projectile dodging and no dodging of enemy laser telegraphs yet.
- Sound reasoning is minimal: the bot only faces recent sounds.
- Mortar is not modelled in the FFA sim. Self-splash avoidance for our own mortar is crude
  (minimum range 22 m only).
- Rooftop camping is not used (jump edges exist, but hold spots are ground/BFS only).
- Upgrade choice is a fixed priority list.
