#!/bin/bash
# PRLs for the renderer gates (C7, C10 per-probe both runs, AC 1 rest pairs). All --release, fresh caches.
F=/c/Users/danhi/AppData/Local/Temp/claude/80d2a19114937a96/8904fe45-929f-4f76-b328-129186a2a8bd/scratchpad/amend2/final
R=/c/Users/danhi/Projects/Personal/postretro
O=$F/test; mkdir -p $O
cd $R
unset POSTRETRO_PROTO_ENCLOSED_DARK POSTRETRO_PROTO_MOVER_SH
bake() {  # bake <bin> <map> <out-name> [flags...]
  local bin=$1 map=$2 name=$3; shift 3
  local C=$F/cache/t-$name; rm -rf $C; mkdir -p $C
  local s=$(date +%s)
  $bin "$(cygpath -w $R/content/dev/maps/$map.map)" -o "$(cygpath -w $O/$name.prl)" --release --no-tui \
    --cache-dir "$(cygpath -w $C)" --baked-root "$(cygpath -w $R/baked)" "$@" > $F/logs/tbake-$name.log 2>&1
  echo "$name rc $? $(( $(date +%s) - s ))s"
  rm -rf $C
}
NEW=$F/bin/prl-build.exe
BASE=$F/base/prl-build-parent.exe
bake $NEW mover-closet new-mover-closet
cp $F/ws/content/dev/maps/head-mover-setpiece.prl $O/new-mover-setpiece.prl
cp $F/ws/content/dev/maps/head-enemy-movers.prl $O/new-enemy-movers.prl
bake $NEW mover-setpiece new-mover-setpiece-bar0 --mover-payload-resident-bar 0
for m in mover-closet mover-setpiece enemy-movers; do bake $NEW $m unc-$m --uncompressed-irradiance; done
for m in mover-closet mover-setpiece enemy-movers; do bake $BASE $m base-$m; done
$F/venv/Scripts/python.exe $F/tools/relabel.py $O/base-mover-closet.prl $O/base-mover-setpiece.prl $O/base-enemy-movers.prl
echo TESTBAKES DONE
