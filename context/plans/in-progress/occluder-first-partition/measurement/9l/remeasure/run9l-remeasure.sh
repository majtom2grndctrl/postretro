#!/bin/bash
# Task 9l re-measure driver: 9k's run9k.sh, re-rooted to target/occluder-9l/, for the narrowed prisms (wall free ends).
# usage: run9l-remeasure.sh bake    <config> <map>   (one bake, private cold cache deleted after;
#                                           hallway and arena --no-cache, as in 9g)
#        run9l-remeasure.sh measure <config> <map>   (full: sweep, structure, readers, bake gap, diffs;
#                                           street adds rooftop sweep and uncapped walks)
#        run9l-remeasure.sh lite    <config> <map>   (structure and walks only: sweep, structure, sweep diff;
#                                           street adds rooftop sweep and uncapped walks; no bake gap)
# Configs: cap26 (HEAD), cap19 (HEAD with CELL_SIZE_CAP_M 19.5072); binaries target/occluder-9l/bin/prl-build-<config>.
# Harness binaries are 9k's frozen copies of 9g's, used in place (no crate they compile changed since d2efc11bb but level-compiler,
# which only the postretro bin's navmesh tests link). Before records: 9g's HEAD-harness re-measure in
# target/occluder-9g/before/<map>/ (byte-identical to the baselines). Outputs: target/occluder-9l/<config>/<map>/.
set -u
W=/Users/dhiester/Projects/Personal/postretro
N=$W/context/plans/in-progress/occluder-first-partition/measurement
BIN=$W/target/occluder-9l/bin            # prl-build-<config> (frozen at HEAD ad3d2315d)
HB=$W/target/occluder-9k/bin             # harness binaries reused from 9k (9g's builds)
G=$W/target/occluder-9l
T=$HB/postretro-tests
U=$HB/uncap9g
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
sweeps() { # M1 sweep (+ rooftop and uncapped on the street), structure, arena regions
  C=$1; M=$2; O=$G/$C/$M; B=$W/target/occluder-9g/before/$M; P=$N/baselines/$M/$M.poses
  POSTRETRO_WORLD_POSES_PRL=$O/$M.prl POSTRETRO_WORLD_POSES_FILE=$P POSTRETRO_WORLD_POSES_BEFORE=$B/$M.before.sweep POSTRETRO_WORLD_POSES_OUT=$O/$M.after.sweep run world_poses_ray_sweep > $O/sweep.out 2>&1; echo "$M sweep rc $?"
  grep -q "^verdict" $O/sweep.out || { echo "ABORT: $C $M sweep has no verdict"; exit 3; }
  POSTRETRO_WORLD_POSES_PRL=$O/$M.prl run world_poses_structure > $O/structure.out 2>&1; echo "$M structure rc $?"
  if [ $M = stress-street ]; then
    POSTRETRO_WORLD_POSES_PRL=$O/$M.prl POSTRETRO_WORLD_POSES_FILE=$RP POSTRETRO_WORLD_POSES_BEFORE=$B/rooftop.sweep POSTRETRO_WORLD_POSES_OUT=$O/rooftop.sweep run world_poses_ray_sweep > $O/rooftop-sweep.out 2>&1; echo "$M rooftop rc $?"
    grep -q "^verdict" $O/rooftop-sweep.out || { echo "ABORT: $C $M rooftop sweep has no verdict"; exit 3; }
    $U $O/$M.prl $P > $O/uncapped-street.txt; $U $O/$M.prl $RP > $O/uncapped-rooftop.txt; echo "$M uncapped done"
  fi
  if [ $M = stress-warren-showcase ]; then
    POSTRETRO_WORLD_POSES_PRL=$O/$M.prl POSTRETRO_WORLD_POSES_REGION=-45.5168,1.6256,-61.7728,13.0048,17.8816,61.7728 run world_poses_structure > $O/region-arena.out 2>&1
    POSTRETRO_WORLD_POSES_PRL=$O/$M.prl POSTRETRO_WORLD_POSES_REGION=6.5024,1.6256,35.7632,13.0048,11.3792,58.5216 run world_poses_structure > $O/region-stairs.out 2>&1
    echo "$M regions done"
  fi
  POSTRETRO_WORLD_POSES_BEFORE=$B/$M.before.sweep POSTRETRO_WORLD_POSES_AFTER=$O/$M.after.sweep run world_poses_diff > $O/diff-sweep.out 2>&1; echo "$M diff sweep rc $?"
}
WHAT=$1
case $WHAT in
bake)
  C=$2; M=$3; O=$G/$C/$M; mkdir -p $O
  F=$(flags_for $M); CF=""
  case "$F" in *--no-cache*) ;; *) rm -rf $O/cache; CF="--cache-dir $O/cache";; esac
  s=$(date +%s)
  { echo "flags: $F $CF"; echo "binary: $BIN/prl-build-$C $(shasum $BIN/prl-build-$C | cut -c1-12)"; echo "start: $(date '+%F %T')"; df -h . | tail -1; } > $O/bake.time
  RUST_LOG=info $BIN/prl-build-$C content/dev/maps/$M.map -o $O/$M.prl --baked-root $G/baked --no-tui -v $F $CF > $O/bake.log 2>&1
  rc=$?
  { echo "exit: $rc"; echo "wall_seconds: $(( $(date +%s) - s ))"; du -sh $O/cache 2>/dev/null; } >> $O/bake.time
  rm -rf $O/cache
  df -h . | tail -1 >> $O/bake.time
  cat $O/bake.time ;;
lite)
  sweeps $2 $3 ;;
measure)
  C=$2; M=$3; O=$G/$C/$M; B=$W/target/occluder-9g/before/$M
  sweeps $C $M
  $HB/mandatory-reader $O/$M.prl 16 > $O/mandatory.out 2>&1; echo "$M mandatory rc $?"
  POSTRETRO_SH_REACH_PRL=$O/$M.prl POSTRETRO_SH_REACH_LEADS=16 "$T" sh_reach --ignored --nocapture --exact sh_streaming::reach_measurement::sh_reach_mandatory_bytes_from_prl > $O/shreach.out 2>&1; echo "$M shreach rc $?"
  POSTRETRO_WORLD_POSES_PRL=$O/$M.prl POSTRETRO_WORLD_POSES_BEFORE=$B/$M.before.bakegap POSTRETRO_WORLD_POSES_OUT=$O/$M.after.bakegap run world_poses_bake_gap > $O/bakegap.out 2>&1; echo "$M bakegap rc $?"
  grep -q "INVALID" $O/bakegap.out && echo "ABORT: $C $M bake-gap pairing INVALID"
  POSTRETRO_WORLD_POSES_BEFORE=$B/$M.before.bakegap POSTRETRO_WORLD_POSES_AFTER=$O/$M.after.bakegap run world_poses_diff > $O/diff-bakegap.out 2>&1; echo "$M diff bakegap rc $?" ;;
esac
