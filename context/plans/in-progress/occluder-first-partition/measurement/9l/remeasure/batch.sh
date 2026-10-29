#!/bin/bash
# Task 9l re-measure batch (9k's batch.sh, re-rooted to target/occluder-9l/; full suite on both configs).
# For each config, bakes the fast maps then the hallway in turn (bakes never overlap; one
# hallway PRL measured while the next config's fast maps bake), measuring each map in the
# background once its bake finishes. Once its records are written, a PRL is deleted unless it is
# kept for live H3 (occlusion-test, stress-street).
# Aborts the batch on an invalid run (missing sweep verdict, INVALID bake-gap pairing).
# usage: batch.sh <config>...     (occlusion-test cap26 was the pilot and is skipped)
set -u
W=/Users/dhiester/Projects/Personal/postretro
R=$W/context/plans/in-progress/occluder-first-partition/measurement/9l/remeasure/run9l-remeasure.sh
G=$W/target/occluder-9l
H=stress-warren-hallway-inspection
MAPS="occlusion-test campaign-test stress-warren-showcase stress-street $H"
measured() { grep -q '^verdict' $G/$1/$2/sweep.out 2>/dev/null && grep -q 'bake-gap record written' $G/$1/$2/bakegap.out 2>/dev/null; }
check() { # abort on an invalid run
  if grep -q "INVALID" $G/$1/$2/bakegap.out $G/$1/$2/diff-bakegap.out 2>/dev/null || ! grep -q '^verdict' $G/$1/$2/sweep.out || ! grep -q 'bake-gap record written' $G/$1/$2/bakegap.out; then
    echo "ABORT: invalid run $1 $2" | tee -a $G/batch.status; kill 0; fi; }
drop_prl() { case $2 in occlusion-test|stress-street) ;; *) rm -f $G/$1/$2/$2.prl; echo "$1 $2 PRL deleted" | tee -a $G/batch.status ;; esac; }
hw_pid=""
for C in "$@"; do
  for M in $MAPS; do
    if measured $C $M; then echo "$C $M: already measured, skipped"; continue; fi
    if [ $M = $H ]; then [ -n "$hw_pid" ] && wait $hw_pid; fi
    df -h $W | tail -1
    $R bake $C $M | grep -E 'exit|wall|Data'
    grep -q '^exit: 0' $G/$C/$M/bake.time || { echo "ABORT: bake $C $M failed" | tee -a $G/batch.status; kill 0; }
    ( $R measure $C $M > $G/$C/$M/measure.log 2>&1; check $C $M; drop_prl $C $M
      echo "$C $M measured" | tee -a $G/batch.status ) &
    [ $M = $H ] && hw_pid=$!
  done
done
wait
df -h $W | tail -1
echo "batch done: $*" | tee -a $G/batch.status
