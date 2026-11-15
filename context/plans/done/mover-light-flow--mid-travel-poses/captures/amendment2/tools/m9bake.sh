#!/bin/bash
# m9bake.sh <col>  -- M9 sightings: cold --release bake of enemy-movers with the HEAD-equivalent
# prl-build (amend3/bin, dbacdfdf2), fresh cache dir, deleted after.
#   head   repo map
#   truth  amend3/ws/.../truth-enemy-movers.map (em_sweeper light_poses 113)
S=/c/Users/danhi/AppData/Local/Temp/claude/80d2a19114937a96/8904fe45-929f-4f76-b328-129186a2a8bd/scratchpad
B=$S/amend3; M=$S/m9class; R=/c/Users/danhi/Projects/Personal/postretro
col=$1; map=enemy-movers; W=$B/ws/content/dev/maps
case $col in
  head)  src=$R/content/dev/maps/$map.map ;;
  truth) src=$W/truth-$map.map ;;
esac
C=$M/cache-$col; rm -rf $C; mkdir -p $C $M/logs
cd $R
unset POSTRETRO_PROTO_ENCLOSED_DARK POSTRETRO_PROTO_MOVER_SH
start=$(date +%s%N)
$B/bin/prl-build.exe "$(cygpath -w $src)" -o "$(cygpath -w $W/$col-$map.prl)" --release --no-tui -v \
  --cache-dir "$(cygpath -w $C)" --baked-root "$(cygpath -w $R/baked)" > $M/logs/bake-$col-$map.log 2>&1
rc=$?
echo "baked $col-$map rc $rc in $(python3 -c "print(round(($(date +%s%N) - $start)/1e9,2))")s"
rm -rf $C
