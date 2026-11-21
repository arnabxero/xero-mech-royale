"""Aggregate `sim ffa` summaries: python tools/agg.py <label> < outputs"""
import re
import sys

label = sys.argv[1] if len(sys.argv) > 1 else ""
tot = n = wins = ez = 0
places = [0] * 16
for line in sys.stdin:
    m = re.search(r"runs (\d+) .*mean score ([\d.]+) wins (\d+) early-zone-deaths (\d+) places \[([\d, ]+)\]", line)
    if not m:
        continue
    r = int(m.group(1))
    tot += float(m.group(2)) * r
    n += r
    wins += int(m.group(3))
    ez += int(m.group(4))
    for i, v in enumerate(int(x) for x in m.group(5).split(",")):
        places[i] += v
print(f"BENCH {label} matches={n} mean={tot / max(n, 1):.2f} wins={wins} early_zone_deaths={ez} places={places}")
