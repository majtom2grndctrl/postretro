#!/bin/bash
# Task 9c driver: bake one map with one frozen config binary, then measure it.
# usage: run9c.sh bake    <config> <map>
#        run9c.sh measure <config> <map>      (sweep, bake gap, structure, diffs, readers)
#        run9c.sh rooftop <config|before|task9> (stress-street rooftop sweep only)
# Binaries: target/occluder-9c/bin/prl-build-<config> (scripts-build beside it),
# harness target/occluder-9c/bin/postretro-tests (= Task 9's, engine side unchanged).
# Outputs: target/occluder-9c/<config>/<map>/. Before records: target/occluder-baselines/.
set -u
W=/Users/dhiester/Projects/Personal/postretro
N=$W/context/plans/done/occluder-first-partition/measurement
BIN=$W/target/occluder-9c/bin
T=$BIN/postretro-tests
cd $W
flags_for() {
  case $1 in
    stress-warren-hallway-inspection) echo "--sh-probe-spacing 4.0" ;;
    stress-warren-showcase) echo "--sh-probe-spacing 10.0 --lightmap-density 0.25 --no-cache" ;;
    stress-street) echo "--sh-probe-spacing 4.0 --lightmap-density 0.25" ;;
    *) echo "" ;;
  esac
}
run() { "$T" "$1" --ignored --nocapture --exact "session::lightmap_residency::walk_measurement::world_poses::$1"; }
WHAT=$1; C=$2; M=${3:-stress-street}
O=$W/target/occluder-9c/$C/$M
B=$W/target/occluder-baselines/$M
P=$N/baselines/$M/$M.poses
mkdir -p $O
case $WHAT in
bake)
  F=$(flags_for $M); CF=""
  case "$F" in *--no-cache*) ;; *) rm -rf $O/cache; CF="--cache-dir $O/cache";; esac
  s=$(date +%s)
  { echo "flags: $F $CF"; echo "binary: $BIN/prl-build-$C $(shasum $BIN/prl-build-$C | cut -c1-12)"; echo "start: $(date '+%F %T')"; } > $O/bake.time
  RUST_LOG=info $BIN/prl-build-$C content/dev/maps/$M.map -o $O/$M.prl --baked-root $W/target/occluder-9c/baked --no-tui -v $F $CF > $O/bake.log 2>&1
  rc=$?
  { echo "exit: $rc"; echo "wall_seconds: $(( $(date +%s) - s ))"; du -sh $O/cache 2>/dev/null; } >> $O/bake.time
  rm -rf $O/cache
  df -h . | tail -1 >> $O/bake.time
  cat $O/bake.time ;;
measure)
  POSTRETRO_WORLD_POSES_PRL=$O/$M.prl POSTRETRO_WORLD_POSES_FILE=$P POSTRETRO_WORLD_POSES_BEFORE=$B/$M.before.sweep POSTRETRO_WORLD_POSES_OUT=$O/$M.after.sweep run world_poses_ray_sweep > $O/sweep.out 2>&1; echo "$M sweep rc $?"
  POSTRETRO_WORLD_POSES_PRL=$O/$M.prl run world_poses_structure > $O/structure.out 2>&1; echo "$M structure rc $?"
  $BIN/mandatory-reader $O/$M.prl 16 > $O/mandatory.out 2>&1; echo "$M mandatory rc $?"
  POSTRETRO_SH_REACH_PRL=$O/$M.prl POSTRETRO_SH_REACH_LEADS=16 "$T" sh_reach --ignored --nocapture --exact sh_streaming::reach_measurement::sh_reach_mandatory_bytes_from_prl > $O/shreach.out 2>&1; echo "$M shreach rc $?"
  POSTRETRO_WORLD_POSES_PRL=$O/$M.prl POSTRETRO_WORLD_POSES_BEFORE=$B/$M.before.bakegap POSTRETRO_WORLD_POSES_OUT=$O/$M.after.bakegap run world_poses_bake_gap > $O/bakegap.out 2>&1; echo "$M bakegap rc $?"
  POSTRETRO_WORLD_POSES_BEFORE=$B/$M.before.sweep POSTRETRO_WORLD_POSES_AFTER=$O/$M.after.sweep run world_poses_diff > $O/diff-sweep.out 2>&1; echo "$M diff sweep rc $?"
  POSTRETRO_WORLD_POSES_BEFORE=$B/$M.before.bakegap POSTRETRO_WORLD_POSES_AFTER=$O/$M.after.bakegap run world_poses_diff > $O/diff-bakegap.out 2>&1; echo "$M diff bakegap rc $?"
  if [ $M = stress-warren-showcase ]; then
    POSTRETRO_WORLD_POSES_PRL=$O/$M.prl POSTRETRO_WORLD_POSES_REGION=-45.5168,1.6256,-61.7728,13.0048,17.8816,61.7728 run world_poses_structure > $O/region-arena.out 2>&1
    POSTRETRO_WORLD_POSES_PRL=$O/$M.prl POSTRETRO_WORLD_POSES_REGION=6.5024,1.6256,35.7632,13.0048,11.3792,58.5216 run world_poses_structure > $O/region-stairs.out 2>&1
    echo "$M regions done"
  fi ;;
rooftop)
  RP=$N/9c/stress-street.rooftop.poses
  case $C in
    before) PRL=$B/$M.prl; TT=$W/target/occluder-baselines/bin/postretro-tests; BEF="" ;;
    task9) PRL=$W/target/occluder-after/$M/$M.prl; TT=$T; BEF=$W/target/occluder-9c/before/$M/rooftop.sweep ;;
    *) PRL=$O/$M.prl; TT=$T; BEF=$W/target/occluder-9c/before/$M/rooftop.sweep ;;
  esac
  O=$W/target/occluder-9c/$C/$M; mkdir -p $O
  POSTRETRO_WORLD_POSES_PRL=$PRL POSTRETRO_WORLD_POSES_FILE=$RP POSTRETRO_WORLD_POSES_BEFORE=$BEF POSTRETRO_WORLD_POSES_OUT=$O/rooftop.sweep \
    "$TT" world_poses_ray_sweep --ignored --nocapture --exact session::lightmap_residency::walk_measurement::world_poses::world_poses_ray_sweep > $O/rooftop-sweep.out 2>&1
  echo "rooftop $C rc $?" ;;
esac
