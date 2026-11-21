#!/usr/bin/env bash
# Heavy-compute benchmark for a many-core Linux box with Docker (no Rust/Python needed on the host).
#
#   tools/server_bench.sh <mix> <procs> <runs-per-proc> <seed-base> "<tune A>" ["<tune B>" ...]
#   e.g. tools/server_bench.sh squad2 200 5 90000 "aim_v2=0" "aim_v2=1"
#
# Builds the simulator once inside rust:1.91-slim-bookworm (cached in ./target-docker), then for each
# tune string runs <procs> parallel processes of <runs-per-proc> matches and prints the mean score and
# its standard error. Extra env for the simulator (SQUAD_NOISE, SIM_NO_NPC, ...) is passed through.
set -eu
MIX=${1:?mix}; PROCS=${2:?procs}; RUNS=${3:?runs}; BASE=${4:?seed}; shift 4
IMG=rust:1.91-slim-bookworm
ROOT=$(cd "$(dirname "$0")/.." && pwd)
docker run --rm -v "$ROOT":/src -w /src -e CARGO_TARGET_DIR=/src/target-docker "$IMG" \
  cargo build --release --locked --bin sim >/dev/null
for TUNE in "$@"; do
  OUT=$(mktemp -d)
  docker run --rm -v "$ROOT":/src -w /src -v "$OUT":/out \
    -e XERO_TUNE="$TUNE" -e SQUAD_NOISE="${SQUAD_NOISE:-0.09}" -e SIM_NO_NPC="${SIM_NO_NPC:-}" \
    -e ARENA=/src/arena "$IMG" bash -c "
      for p in \$(seq 0 $((PROCS - 1))); do
        ./target-docker/release/sim ffa $RUNS \$(( $BASE + p * 1000 )) 1 $MIX > /out/o\$p.txt 2>/dev/null &
      done; wait"
  # per-match scores: 16 - place, from the 'place N' field of each match line
  cat "$OUT"/o*.txt | grep -h '^seed' | sed -E 's/.* place ([0-9]+) .*/\1/' | \
    awk -v tune="$TUNE" -v mix="$MIX" '{s=16-$1; n++; t+=s; q+=s*s} END {m=t/n; sd=sqrt(q/n-m*m); printf "SERVER %s [%s] matches=%d mean=%.3f se=%.3f\n", mix, tune, n, m, sd/sqrt(n)}'
  rm -rf "$OUT"
done
