#!/bin/bash
# bbake.sh <col>  -- big-room re-capture: cold --release bake of mover-setpiece, fresh cache dir, deleted after.
#   head   HEAD prl-build (amend3/bin), repo map
#   truth  HEAD prl-build, amend3/ws/.../truth-mover-setpiece.map (wall 267, room 76, room_big 98)
#   ramp   prototype prl-build (amend2/bin, f712cfc14, pre-dark, epoch 6), repo map
S=/c/Users/danhi/AppData/Local/Temp/claude/80d2a19114937a96/8904fe45-929f-4f76-b328-129186a2a8bd/scratchpad
B=$S/amend3; R=/c/Users/danhi/Projects/Personal/postretro
col=$1; map=mover-setpiece; W=$B/ws/content/dev/maps
case $col in
  head)  exe=$B/bin/prl-build.exe; src=$R/content/dev/maps/$map.map ;;
  truth) exe=$B/bin/prl-build.exe; src=$W/truth-$map.map ;;
  ramp)  exe=$S/amend2/bin/prl-build.exe; src=$R/content/dev/maps/$map.map ;;
esac
C=$B/cache/$col-$map; rm -rf $C; mkdir -p $C $B/logs
cd $R
unset POSTRETRO_PROTO_ENCLOSED_DARK POSTRETRO_PROTO_MOVER_SH
start=$(date +%s%N)
$exe "$(cygpath -w $src)" -o "$(cygpath -w $W/$col-$map.prl)" --release --no-tui -v \
  --cache-dir "$(cygpath -w $C)" --baked-root "$(cygpath -w $R/baked)" > $B/logs/bake-$col-$map.log 2>&1
rc=$?
echo "baked $col-$map rc $rc in $(python3 -c "print(round(($(date +%s%N) - $start)/1e9,2))")s"
rm -rf $C
