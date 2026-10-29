#!/bin/sh
# In-process walk timing at the live poses (CPU only, no GPU, no window): the frozen
# walktime-capped tool (h3/tools/walktime.main.rs, dev profile, crates/visibility at HEAD)
# on each side's PRL, 3 repeats. Gives the denominator of the live walk factor
# (live portal_walk us per test / in-process us per test).
# Two eyes per pose: <label> at floor+1.31 (the settled live eye) and <label>~e150 at
# floor+1.5 (the sweep convention, comparable with h3/records/walk/).
# usage: walk_inproc.sh <batch dir>    writes <batch dir>/walk/<side>.<map>.r<rep>.txt
. /Users/dhiester/Projects/Personal/postretro/context/plans/in-progress/occluder-first-partition/measurement/h3/live/env.sh
O=$1; WD=$O/walk; mkdir -p "$WD"
P=${POSES:-$L/poses.live}
for m in occlusion-test stress-street; do
  { echo "postretro-world-poses 1"
    echo "# live H3 poses ($P): eye = floor+1.31 (settled live eye) and floor+1.5 (~e150)"
    awk -v m="$m" '!/^#/ && NF >= 4 && $2 == m {
      split($3, p, ","); printf "pose %s %s %.4f %s %s %s\n", $1, p[1], $4 + 1.31, p[3], p[4], p[5]
      printf "pose %s~e150 %s %.4f %s %s %s\n", $1, p[1], $4 + 1.5, p[3], p[4], p[5] }' "$P"
  } > "$WD/$m.poses"
done
for rep in 1 2 3; do
  for s in $SIDES; do
    for m in occlusion-test stress-street; do
      caffeinate -i "$B/walktime-capped" "$(prl_of $s $m)" "$WD/$m.poses" > "$WD/$s.$m.r$rep.txt" 2>&1 < /dev/null
      echo "walk $s $m r$rep rc=$? $(grep -vc '^#' "$WD/$s.$m.r$rep.txt") poses"
    done
  done
done
echo WALK_INPROC_DONE
