#!/usr/bin/env bash
# Runs INSIDE the rust container: tools/inner_bench.sh <mix> <procs> <runs-per-proc> <seed-base> <label>
# Uses XERO_TUNE / SQUAD_NOISE / SIM_* from the environment. Prints per-config mean and SE.
set -u
MIX=$1; PROCS=$2; RUNS=$3; BASE=$4; LABEL=$5
OUT=$(mktemp -d)
for p in $(seq 0 $((PROCS - 1))); do
  ${SIM_BIN:-./target-docker/release/sim} ffa "$RUNS" $((BASE + p * 1000)) 1 "$MIX" > "$OUT/o$p.txt" 2>/dev/null &
done
wait
cat "$OUT"/o*.txt | grep -h '^seed' > "$OUT/all.txt"
sed -E 's/.* place ([0-9]+) .*/\1/' "$OUT/all.txt" | \
  awk -v l="$LABEL" -v mix="$MIX" '{s=16-$1; n++; t+=s; q+=s*s} END {m=t/n; sd=sqrt(q/n-m*m); printf "RESULT %s mix=%s matches=%d mean=%.3f se=%.3f\n", l, mix, n, m, sd/sqrt(n)}'
grep -h "^ACC_RAW" "$OUT"/o*.txt | awk '{for(i=2;i<=17;i++) a[i]+=$i} END {
  printf "ACC ours gun <60 %.0f%% <120 %.0f%% >=120 %.0f%% laser %.0f%% (%d) | opp gun <60 %.0f%% <120 %.0f%% >=120 %.0f%% laser %.0f%% (%d)\n",
    100*a[3]/(a[2]+1e-9), 100*a[5]/(a[4]+1e-9), 100*a[7]/(a[6]+1e-9), 100*a[9]/(a[8]+1e-9), a[8],
    100*a[11]/(a[10]+1e-9), 100*a[13]/(a[12]+1e-9), 100*a[15]/(a[14]+1e-9), 100*a[17]/(a[16]+1e-9), a[16] }'
grep -h "^REAR_RAW" "$OUT"/o*.txt | awk '{r+=$2; t+=$3} END {printf "REAR damage from behind: %.1f%% of %.0f mech damage taken\n", 100*r/(t+1e-9), t}'
mkdir -p /w/logs/matches && cp "$OUT/all.txt" "/w/logs/matches/$(echo "$LABEL" | tr -c 'A-Za-z0-9_=.-' '_').txt"
rm -rf "$OUT"
