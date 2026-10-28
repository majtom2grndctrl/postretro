#!/bin/bash
# Task 9g driver: M1/M2 re-run under HEAD's pool policy (9f) for before, cap19, cap26.
# usage: run9g.sh bake    <cap19|cap26> <map>   (one bake, private cold cache deleted after;
#                                                hallway and arena --no-cache, see README)
#        run9g.sh measure <cap19|cap26> <map>   (sweep, structure, readers, bake gap, diffs;
#                                                street adds rooftop sweep and uncapped walks)
#        run9g.sh before  <map>                  (before PRL re-measured with HEAD's harness)
# Binaries: target/occluder-9g/bin/ (prl-build-<config>, scripts-build, postretro-tests = HEAD
# d2efc11bb harness, mandatory-reader from 9c, uncap9g scratch walk with the step cap raised).
# Before PRLs and records: target/occluder-baselines/<map>/. Outputs: target/occluder-9g/<side>/<map>/.
set -u
W=/Users/dhiester/Projects/Personal/postretro
N=$W/context/plans/in-progress/occluder-first-partition/measurement
BIN=$W/target/occluder-9g/bin
T=$BIN/postretro-tests
U=$BIN/uncap9g
RP=$N/9c/stress-street.rooftop.poses
cd $W
flags_for() {
  case $1 in
    stress-warren-hallway-inspection) echo "--sh-probe-spacing 4.0 --no-cache" ;;
    stress-warren-showcase) echo "--sh-probe-spacing 10.0 --lightmap-density 0.25 --no-cache" ;;
    stress-street) echo "--sh-probe-spacing 4.0 --lightmap-density 0.25" ;;
    *) echo "" ;;
  esac
}
run() { "$T" "$1" --ignored --nocapture --exact "session::lightmap_residency::walk_measurement::world_poses::$1"; }
WHAT=$1
case $WHAT in
bake)
  C=$2; M=$3; O=$W/target/occluder-9g/$C/$M; mkdir -p $O
  F=$(flags_for $M); CF=""
  case "$F" in *--no-cache*) ;; *) rm -rf $O/cache; CF="--cache-dir $O/cache";; esac
  s=$(date +%s)
  { echo "flags: $F $CF"; echo "binary: $BIN/prl-build-$C $(shasum $BIN/prl-build-$C | cut -c1-12)"; echo "start: $(date '+%F %T')"; df -h . | tail -1; } > $O/bake.time
  RUST_LOG=info $BIN/prl-build-$C content/dev/maps/$M.map -o $O/$M.prl --baked-root $W/target/occluder-9g/baked --no-tui -v $F $CF > $O/bake.log 2>&1
  rc=$?
  { echo "exit: $rc"; echo "wall_seconds: $(( $(date +%s) - s ))"; du -sh $O/cache 2>/dev/null; } >> $O/bake.time
  rm -rf $O/cache
  df -h . | tail -1 >> $O/bake.time
  cat $O/bake.time ;;
measure)
  C=$2; M=$3; O=$W/target/occluder-9g/$C/$M; B=$W/target/occluder-9g/before/$M; P=$N/baselines/$M/$M.poses
  POSTRETRO_WORLD_POSES_PRL=$O/$M.prl POSTRETRO_WORLD_POSES_FILE=$P POSTRETRO_WORLD_POSES_BEFORE=$B/$M.before.sweep POSTRETRO_WORLD_POSES_OUT=$O/$M.after.sweep run world_poses_ray_sweep > $O/sweep.out 2>&1; echo "$M sweep rc $?"
  grep -q "^verdict" $O/sweep.out || { echo "ABORT: $C $M sweep has no verdict"; exit 3; }
  POSTRETRO_WORLD_POSES_PRL=$O/$M.prl run world_poses_structure > $O/structure.out 2>&1; echo "$M structure rc $?"
  $BIN/mandatory-reader $O/$M.prl 16 > $O/mandatory.out 2>&1; echo "$M mandatory rc $?"
  POSTRETRO_SH_REACH_PRL=$O/$M.prl POSTRETRO_SH_REACH_LEADS=16 "$T" sh_reach --ignored --nocapture --exact sh_streaming::reach_measurement::sh_reach_mandatory_bytes_from_prl > $O/shreach.out 2>&1; echo "$M shreach rc $?"
  if [ $M = stress-street ]; then
    POSTRETRO_WORLD_POSES_PRL=$O/$M.prl POSTRETRO_WORLD_POSES_FILE=$RP POSTRETRO_WORLD_POSES_BEFORE=$B/rooftop.sweep POSTRETRO_WORLD_POSES_OUT=$O/rooftop.sweep run world_poses_ray_sweep > $O/rooftop-sweep.out 2>&1; echo "$M rooftop rc $?"
    $U $O/$M.prl $P > $O/uncapped-street.txt; $U $O/$M.prl $RP > $O/uncapped-rooftop.txt; echo "$M uncapped done"
  fi
  if [ $M = stress-warren-showcase ]; then
    POSTRETRO_WORLD_POSES_PRL=$O/$M.prl POSTRETRO_WORLD_POSES_REGION=-45.5168,1.6256,-61.7728,13.0048,17.8816,61.7728 run world_poses_structure > $O/region-arena.out 2>&1
    POSTRETRO_WORLD_POSES_PRL=$O/$M.prl POSTRETRO_WORLD_POSES_REGION=6.5024,1.6256,35.7632,13.0048,11.3792,58.5216 run world_poses_structure > $O/region-stairs.out 2>&1
    echo "$M regions done"
  fi
  POSTRETRO_WORLD_POSES_PRL=$O/$M.prl POSTRETRO_WORLD_POSES_BEFORE=$B/$M.before.bakegap POSTRETRO_WORLD_POSES_OUT=$O/$M.after.bakegap run world_poses_bake_gap > $O/bakegap.out 2>&1; echo "$M bakegap rc $?"
  grep -q "INVALID" $O/bakegap.out && echo "ABORT: $C $M bake-gap pairing INVALID"
  POSTRETRO_WORLD_POSES_BEFORE=$B/$M.before.sweep POSTRETRO_WORLD_POSES_AFTER=$O/$M.after.sweep run world_poses_diff > $O/diff-sweep.out 2>&1; echo "$M diff sweep rc $?"
  POSTRETRO_WORLD_POSES_BEFORE=$B/$M.before.bakegap POSTRETRO_WORLD_POSES_AFTER=$O/$M.after.bakegap run world_poses_diff > $O/diff-bakegap.out 2>&1; echo "$M diff bakegap rc $?" ;;
before)
  M=$2; O=$W/target/occluder-9g/before/$M; B=$W/target/occluder-baselines/$M; P=$N/baselines/$M/$M.poses; mkdir -p $O
  # Sweep: same poses, paired against the baselines' own record (a harness-drift check).
  POSTRETRO_WORLD_POSES_PRL=$B/$M.prl POSTRETRO_WORLD_POSES_FILE=$P POSTRETRO_WORLD_POSES_BEFORE=$B/$M.before.sweep POSTRETRO_WORLD_POSES_OUT=$O/$M.before.sweep run world_poses_ray_sweep > $O/sweep.out 2>&1; echo "before $M sweep rc $?"
  # Bake gap: replay the baselines' frames under HEAD's pool policy; this record is every config's BEFORE.
  POSTRETRO_WORLD_POSES_PRL=$B/$M.prl POSTRETRO_WORLD_POSES_BEFORE=$B/$M.before.bakegap POSTRETRO_WORLD_POSES_OUT=$O/$M.before.bakegap run world_poses_bake_gap > $O/bakegap.out 2>&1; echo "before $M bakegap rc $?"
  if [ $M = stress-street ]; then
    POSTRETRO_WORLD_POSES_PRL=$B/$M.prl POSTRETRO_WORLD_POSES_FILE=$RP POSTRETRO_WORLD_POSES_BEFORE=$W/target/occluder-9c/before/$M/rooftop.sweep POSTRETRO_WORLD_POSES_OUT=$O/rooftop.sweep run world_poses_ray_sweep > $O/rooftop-sweep.out 2>&1; echo "before rooftop rc $?"
    $U $B/$M.prl $P > $O/uncapped-street.txt; $U $B/$M.prl $RP > $O/uncapped-rooftop.txt; echo "before uncapped done"
  fi ;;
esac
