#!/usr/bin/env bash
# Parallel FFA benchmark: tools/bench.sh <runs-per-proc> <procs> <ours> <mix> [seed-base]
# Prints the mean score over all processes; per-match lines go to bench_last.txt.
set -u
RUNS=${1:-30}
PROCS=${2:-6}
OURS=${3:-1}
MIX=${4:-mixed}
BASE=${5:-1000}
BIN=${SIM_BIN:-./target/release/sim}
[ -x "$BIN.exe" ] && BIN="$BIN.exe"
OUT=./bench_out
mkdir -p "$OUT"
for p in $(seq 0 $((PROCS - 1))); do
  "$BIN" ffa "$RUNS" $((BASE + p * 1000)) "$OURS" "$MIX" > "$OUT/out$p.txt" 2>/dev/null &
done
wait
cat "$OUT"/out*.txt | grep -h "FFA SUMMARY" | python tools/agg.py "mix=$MIX ours=$OURS"
grep -h "^seed" "$OUT"/out*.txt > ./bench_last.txt
rm -f "$OUT"/out*.txt
