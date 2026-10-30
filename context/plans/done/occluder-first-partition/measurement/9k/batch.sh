#!/bin/bash
# Task 9k batch (9g's batch.sh without the before re-measure, which 9g's records supply).
# For each config, bakes the fast maps then the hallway in turn (bakes never overlap; one
# hallway PRL measured while the next config's fast maps bake), measuring each map in the
# background once its bake finishes. A hallway PRL is deleted once its records are written.
# Aborts the batch on an invalid run (missing sweep verdict, INVALID bake-gap pairing).
# Full configs (cap26, cap19) run `measure`; the comparison configs run `lite` (no bake gap):
# cap26-noprism on all five maps, cap26-f1 on the street only.
# usage: batch.sh <config>...     (occlusion-test cap26 was the pilot and is skipped)
set -u
W=/Users/dhiester/Projects/Personal/postretro
R=$W/context/plans/done/occluder-first-partition/measurement/9k/run9k.sh
G=$W/target/occluder-9k
H=stress-warren-hallway-inspection
mode_for() { case $1 in cap26|cap19) echo measure ;; *) echo lite ;; esac; }
maps_for() { case $1 in cap26-f1) echo "stress-street" ;; *) echo "occlusion-test campaign-test stress-warren-showcase stress-street $H" ;; esac; }
measured() { grep -q '^verdict' $G/$1/$2/sweep.out 2>/dev/null && { [ $(mode_for $1) = lite ] || grep -q 'bake-gap record written' $G/$1/$2/bakegap.out 2>/dev/null; }; }
check() { # abort on an invalid run
  if grep -q "INVALID" $G/$1/$2/bakegap.out $G/$1/$2/diff-bakegap.out 2>/dev/null || ! grep -q '^verdict' $G/$1/$2/sweep.out; then
    echo "ABORT: invalid run $1 $2" | tee -a $G/batch.status; kill 0; fi; }
hw_pid=""
for C in "$@"; do
  MODE=$(mode_for $C)
  for M in $(maps_for $C); do
    if measured $C $M; then echo "$C $M: already measured, skipped"; continue; fi
    if [ $M = $H ]; then [ -n "$hw_pid" ] && wait $hw_pid; fi
    df -h $W | tail -1
    $R bake $C $M | grep -E 'exit|wall|Data'
    grep -q '^exit: 0' $G/$C/$M/bake.time || { echo "ABORT: bake $C $M failed" | tee -a $G/batch.status; kill 0; }
    if [ $M = $H ]; then
      ( $R $MODE $C $M > $G/$C/$M/measure.log 2>&1; check $C $M
        rm -f $G/$C/$M/$M.prl
        echo "$C $M measured, PRL deleted" | tee -a $G/batch.status ) &
      hw_pid=$!
    else
      ( $R $MODE $C $M > $G/$C/$M/measure.log 2>&1; check $C $M; echo "$C $M measured" | tee -a $G/batch.status ) &
    fi
  done
done
wait
df -h $W | tail -1
echo "batch done: $*" | tee -a $G/batch.status
