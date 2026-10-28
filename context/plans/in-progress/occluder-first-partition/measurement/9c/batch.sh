#!/bin/bash
# Task 9c batch: for each config, bake the four fast maps then the hallway in
# turn (bakes never overlap), measuring each map in the background as soon as
# its bake finishes. A hallway bake waits for the previous hallway measure, so
# at most one hallway PRL and one hallway cache exist at a time (disk). The
# hallway PRL is deleted once its records are written.
# usage: batch.sh <config>...
set -u
W=/Users/dhiester/Projects/Personal/postretro
R=$W/context/plans/in-progress/occluder-first-partition/measurement/9c/run9c.sh
H=stress-warren-hallway-inspection
hw_pid=""
for C in "$@"; do
  for M in occlusion-test campaign-test stress-warren-showcase stress-street; do
    if [ -f $W/target/occluder-9c/$C/$M/sweep.out ] && grep -q 'bake-gap record written' $W/target/occluder-9c/$C/$M/bakegap.out 2>/dev/null; then
      echo "$C $M: already measured, skipped"; continue
    fi
    $R bake $C $M | grep -E 'exit|wall|Data'
    ( $R measure $C $M > $W/target/occluder-9c/$C/$M/measure.log 2>&1
      [ $M = stress-street ] && $R rooftop $C >> $W/target/occluder-9c/$C/$M/measure.log 2>&1
      echo "$C $M measured" ) &
  done
  [ -n "$hw_pid" ] && wait $hw_pid
  df -h $W | tail -1
  $R bake $C $H | grep -E 'exit|wall|cache|Data'
  ( $R measure $C $H > $W/target/occluder-9c/$C/$H/measure.log 2>&1
    rm -f $W/target/occluder-9c/$C/$H/$H.prl
    echo "$C $H measured, PRL deleted" ) &
  hw_pid=$!
done
wait
echo "batch done: $*"
