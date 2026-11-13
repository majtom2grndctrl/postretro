#!/bin/bash
# fbake.sh <col> <map>  -- cold --release bake with the final prl-build, fresh cache dir, deleted after.
# head reads the repo map; truth reads final/ws/.../truth-<map>.map.
F=/c/Users/danhi/AppData/Local/Temp/claude/80d2a19114937a96/8904fe45-929f-4f76-b328-129186a2a8bd/scratchpad/amend2/final
R=/c/Users/danhi/Projects/Personal/postretro
col=$1; map=$2; W=$F/ws/content/dev/maps
if [ "$col" = head ]; then src=$R/content/dev/maps/$map.map; else src=$W/$col-$map.map; fi
C=$F/cache/$col-$map; rm -rf $C; mkdir -p $C
cd $R
unset POSTRETRO_PROTO_ENCLOSED_DARK POSTRETRO_PROTO_MOVER_SH
start=$(date +%s%N)
$F/bin/prl-build.exe "$(cygpath -w $src)" -o "$(cygpath -w $W/$col-$map.prl)" --release --no-tui -v \
  --cache-dir "$(cygpath -w $C)" --baked-root "$(cygpath -w $R/baked)" > $F/logs/bake-$col-$map.log 2>&1
rc=$?
echo "baked $col-$map rc $rc in $(python -c "print(round(($(date +%s%N) - $start)/1e9,2))")s"
rm -rf $C
