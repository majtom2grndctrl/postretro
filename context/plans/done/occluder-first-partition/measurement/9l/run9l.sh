#!/bin/bash
# Task 9l driver: 9k's run9k.sh `bake` and `lite` for the narrowed-prism variants, street and
# occlusion-test only. Harness binaries are 9k's frozen ones (target/occluder-9k/bin/), unchanged.
# usage: run9l.sh bake  <config> <map>    (private fresh --cache-dir, deleted after)
#        run9l.sh lite  <config> stress-street   (sweep, structure, rooftop sweep, uncapped walks, diff)
#        run9l.sh c18   <config>          (occlusion-test map C18 sweep with 9k's test harness)
# Configs: bfwv (free vertical end planes of occluding walls' prisms), free (free side planes,
# brush and section prisms); binaries target/occluder-9l/bin/prl-build-<config>.
set -u
W=/Users/dhiester/Projects/Personal/postretro
N=$W/context/plans/done/occluder-first-partition/measurement
BIN9K=$W/target/occluder-9k/bin
BIN=$W/target/occluder-9l/bin
G=$W/target/occluder-9l
T=$BIN9K/postretro-tests
U=$BIN9K/uncap9g
RP=$N/9c/stress-street.rooftop.poses
cd $W
flags_for() { case $1 in stress-street) echo "--sh-probe-spacing 4.0 --lightmap-density 0.25" ;; *) echo "" ;; esac; }
run() { "$T" "$1" --ignored --nocapture --exact "session::lightmap_residency::walk_measurement::world_poses::$1"; }
case $1 in
bake)
  C=$2; M=$3; O=$G/$C/$M; mkdir -p $O; F=$(flags_for $M); rm -rf $O/cache
  s=$(date +%s)
  { echo "flags: $F --cache-dir $O/cache"; echo "binary: $BIN/prl-build-$C $(shasum $BIN/prl-build-$C | cut -c1-12)"; echo "start: $(date '+%F %T')"; df -h . | tail -1; } > $O/bake.time
  RUST_LOG=info $BIN/prl-build-$C content/dev/maps/$M.map -o $O/$M.prl --baked-root $G/baked --no-tui -v $F --cache-dir $O/cache > $O/bake.log 2>&1
  rc=$?
  { echo "exit: $rc"; echo "wall_seconds: $(( $(date +%s) - s ))"; du -sh $O/cache 2>/dev/null; } >> $O/bake.time
  rm -rf $O/cache; df -h . | tail -1 >> $O/bake.time; cat $O/bake.time ;;
lite)
  C=$2; M=stress-street; O=$G/$C/$M; B=$W/target/occluder-9g/before/$M; P=$N/baselines/$M/$M.poses
  POSTRETRO_WORLD_POSES_PRL=$O/$M.prl POSTRETRO_WORLD_POSES_FILE=$P POSTRETRO_WORLD_POSES_BEFORE=$B/$M.before.sweep POSTRETRO_WORLD_POSES_OUT=$O/$M.after.sweep run world_poses_ray_sweep > $O/sweep.out 2>&1; echo "sweep rc $?"
  grep -q "^verdict" $O/sweep.out || { echo "ABORT: no sweep verdict"; exit 3; }
  POSTRETRO_WORLD_POSES_PRL=$O/$M.prl run world_poses_structure > $O/structure.out 2>&1; echo "structure rc $?"
  POSTRETRO_WORLD_POSES_PRL=$O/$M.prl POSTRETRO_WORLD_POSES_FILE=$RP POSTRETRO_WORLD_POSES_BEFORE=$B/rooftop.sweep POSTRETRO_WORLD_POSES_OUT=$O/rooftop.sweep run world_poses_ray_sweep > $O/rooftop-sweep.out 2>&1; echo "rooftop rc $?"
  grep -q "^verdict" $O/rooftop-sweep.out || { echo "ABORT: no rooftop verdict"; exit 3; }
  $U $O/$M.prl $P > $O/uncapped-street.txt; $U $O/$M.prl $RP > $O/uncapped-rooftop.txt; echo "uncapped done"
  POSTRETRO_WORLD_POSES_BEFORE=$B/$M.before.sweep POSTRETRO_WORLD_POSES_AFTER=$O/$M.after.sweep run world_poses_diff > $O/diff-sweep.out 2>&1; echo "diff rc $?" ;;
c18)
  C=$2; O=$G/$C/occlusion-test
  ( cd $W/crates/level-compiler && POSTRETRO_LEAK_SWEEP_PRL=$O/occlusion-test.prl $BIN9K/prl-build-tests-cap26 behind_wall_leak_sweep --ignored --nocapture ) > $O/c18-sweep.out 2>&1; echo "c18 rc $?"; grep -E "leak mean" $O/c18-sweep.out ;;
esac
