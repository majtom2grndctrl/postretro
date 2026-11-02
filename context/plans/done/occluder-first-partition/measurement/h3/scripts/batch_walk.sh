#!/bin/bash
# H3 walk-cost batch (CPU only, one process at a time, nothing else running).
# For before / cap19 / cap26: the walk-timing tool (tools/walktime.main.rs) over
#  - the H3 pose sets (poses/*.poses), 3 repeats, dev profile, capped and uncapped;
#  - the full 9g street (2,960) and rooftop (608) sets, once, dev uncapped, dev capped
#    and release uncapped (the time-per-portal-test regression).
# Output: <scratch>/walk/<side>.<set>.<variant>.<profile>.r<rep>.txt
set -u
S=/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/25609316-dd73-4754-993f-da7529caa8f7/scratchpad/h3
W=/Users/dhiester/Projects/Personal/postretro
N=$W/context/plans/done/occluder-first-partition/measurement
O=$S/walk; mkdir -p $O
prl() { # side map
  case $1 in
    before) echo $W/target/occluder-baselines/$2/$2.prl ;;
    *) if [ $2 = stress-warren-hallway-inspection ]; then echo $W/target/occluder-h3/$1/$2/$2.prl; else echo $W/target/occluder-9g/$1/$2/$2.prl; fi ;;
  esac; }
tool() { echo $S/walktime-$1/target/$2/walktime-$1; } # variant profile(debug|release)
for rep in 1 2 3; do
  for M in occlusion-test stress-street campaign-test stress-warren-showcase stress-warren-hallway-inspection; do
    for side in before cap19 cap26; do
      p=$(prl $side $M); [ -f $p ] || { echo "skip $side $M (no PRL)"; continue; }
      for v in capped uncapped; do
        caffeinate -i $(tool $v debug) $p $N/h3/poses/$M.poses > $O/$side.$M.$v.debug.r$rep.txt 2>&1
      done
    done
  done
  echo "rep $rep done $(date '+%T')"
done
for side in before cap19 cap26; do
  p=$(prl $side stress-street)
  for set in street rooftop; do
    [ $set = street ] && F=$N/baselines/stress-street/stress-street.poses || F=$N/9c/stress-street.rooftop.poses
    caffeinate -i $(tool uncapped debug) $p $F > $O/$side.full-$set.uncapped.debug.txt 2>&1
    caffeinate -i $(tool capped debug) $p $F > $O/$side.full-$set.capped.debug.txt 2>&1
    caffeinate -i $(tool uncapped release) $p $F > $O/$side.full-$set.uncapped.release.txt 2>&1
    echo "$side $set full done $(date '+%T')"
  done
done
echo WALK_BATCH_DONE
