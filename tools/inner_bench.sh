#!/usr/bin/env bash
# Runs INSIDE the rust container: tools/inner_bench.sh <mix> <procs> <runs-per-proc> <seed-base> <label>
# Uses XERO_TUNE / SQUAD_NOISE / SIM_* from the environment. Prints per-config mean and SE.
set -u
MIX=$1; PROCS=$2; RUNS=$3; BASE=$4; LABEL=$5
OUT=$(mktemp -d)
for p in $(seq 0 $((PROCS - 1))); do
  ./target-docker/release/sim ffa "$RUNS" $((BASE + p * 1000)) 1 "$MIX" > "$OUT/o$p.txt" 2>/dev/null &
done
wait
cat "$OUT"/o*.txt | grep -h '^seed' > "$OUT/all.txt"
sed -E 's/.* place ([0-9]+) .*/\1/' "$OUT/all.txt" | \
  awk -v l="$LABEL" -v mix="$MIX" '{s=16-$1; n++; t+=s; q+=s*s} END {m=t/n; sd=sqrt(q/n-m*m); printf "RESULT %s mix=%s matches=%d mean=%.3f se=%.3f\n", l, mix, n, m, sd/sqrt(n)}'
grep -h "ACCURACY" "$OUT"/o*.txt | head -3
mkdir -p /w/logs/matches && cp "$OUT/all.txt" "/w/logs/matches/$(echo "$LABEL" | tr -c 'A-Za-z0-9_=.-' '_').txt"
rm -rf "$OUT"
