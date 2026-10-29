#!/bin/sh
# Regime sentinel: a short live run, before build, occlusion-test fc:P1, no trace
# (1 settle window + 3 kept, about 8 s of frames when healthy). Normal frame total is
# vsync-bound near 16.7 ms; in the slowdown regime the offline probes ran ~225 ms.
# usage: sentinel.sh <outdir> <tag>
# stdout: "<tag> total_ms=<mean CpuTiming total over kept windows> rc=<run_live exit> <post machstate>"
. /Users/dhiester/Projects/Personal/postretro/context/plans/in-progress/occluder-first-partition/measurement/h3/live/env.sh
O=$1; TAG=$2
"$L/run_live.sh" "$O" "$TAG" before occlusion-test "$SENTINEL_POSE" 1 3 0
rc=$?
tot=$(python3 "$L/cpuwin.py" "$O/$TAG.log" 1 --field total 2>/dev/null)
echo "$TAG total_ms=${tot:-nan} rc=$rc $(grep '^post:' "$O/$TAG.meta" | sed 's/^post: //')"
exit $rc
