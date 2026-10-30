#!/bin/bash
# Task 9g batch. Re-measures the before PRLs with HEAD's harness (fast maps first,
# hallway in the background), then for each config bakes the fast maps and the
# hallway in turn (bakes never overlap), measuring each map in the background once
# its bake finishes and its before records exist. A hallway PRL is deleted once its
# records are written. Aborts the batch on an invalid run.
# usage: batch.sh <config>...      (occlusion-test cap26 was the pilot and is skipped)
set -u
W=/Users/dhiester/Projects/Personal/postretro
R=$W/context/plans/done/occluder-first-partition/measurement/9g/run9g.sh
G=$W/target/occluder-9g
H=stress-warren-hallway-inspection
FAST="occlusion-test campaign-test stress-warren-showcase stress-street"
done_before() { grep -q 'bake-gap record written' $G/before/$1/bakegap.out 2>/dev/null; }
measured() { grep -q 'bake-gap record written' $G/$1/$2/bakegap.out 2>/dev/null; }
check() { # abort on an invalid run
  if grep -q "INVALID" $G/$1/$2/bakegap.out $G/$1/$2/diff-bakegap.out 2>/dev/null || ! grep -q '^verdict' $G/$1/$2/sweep.out; then
    echo "ABORT: invalid run $1 $2" | tee -a $G/batch.status; kill 0; fi; }
for M in $FAST; do done_before $M || $R before $M; done
( done_before $H || $R before $H; echo "before $H measured" ) &
before_hw=$!
hw_pid=""
for C in "$@"; do
  for M in $FAST; do
    if measured $C $M; then echo "$C $M: already measured, skipped"; continue; fi
    $R bake $C $M | grep -E 'exit|wall|Data'
    grep -q '^exit: 0' $G/$C/$M/bake.time || { echo "ABORT: bake $C $M failed" | tee -a $G/batch.status; kill 0; }
    ( $R measure $C $M > $G/$C/$M/measure.log 2>&1; check $C $M; echo "$C $M measured" | tee -a $G/batch.status ) &
  done
  [ -n "$hw_pid" ] && wait $hw_pid
  df -h $W | tail -1
  $R bake $C $H | grep -E 'exit|wall|Data'
  grep -q '^exit: 0' $G/$C/$H/bake.time || { echo "ABORT: bake $C $H failed" | tee -a $G/batch.status; kill 0; }
  ( wait $before_hw 2>/dev/null; while ! done_before $H; do sleep 30; done
    $R measure $C $H > $G/$C/$H/measure.log 2>&1; check $C $H
    rm -f $G/$C/$H/$H.prl
    echo "$C $H measured, PRL deleted" | tee -a $G/batch.status ) &
  hw_pid=$!
done
wait
echo "batch done: $*" | tee -a $G/batch.status
