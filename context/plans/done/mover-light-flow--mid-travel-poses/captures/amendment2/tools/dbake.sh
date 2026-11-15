#!/bin/bash
# dbake.sh <col> <srccol> <map> [dark]  -- cold --release bake with the darkb prl-build.
# Source map: repo map for srccol=head, else ws/<srccol>-<map>.map. Output ws/<col>-<map>.prl.
A=/c/Users/danhi/AppData/Local/Temp/claude/80d2a19114937a96/8904fe45-929f-4f76-b328-129186a2a8bd/scratchpad/amend2
R=/c/Users/danhi/Projects/Personal/postretro
col=$1; srccol=$2; map=$3; dark=$4; W=$A/ws/content/dev/maps
if [ "$srccol" = head ]; then src=$R/content/dev/maps/$map.map; else src=$W/$srccol-$map.map; fi
C=$A/cache/darkb-$col-$map; rm -rf $C; mkdir -p $C
cd $R
if [ -n "$dark" ]; then export POSTRETRO_PROTO_ENCLOSED_DARK=1; else unset POSTRETRO_PROTO_ENCLOSED_DARK; fi
start=$(date +%s)
$A/darkb/bin/prl-build.exe "$(cygpath -w $src)" -o "$(cygpath -w $W/$col-$map.prl)" --release --no-tui -v \
  --cache-dir "$(cygpath -w $C)" --baked-root "$(cygpath -w $R/baked)" > $A/darkb/logs/bake-$col-$map.log 2>&1
rc=$?
echo "baked $col-$map (src $srccol, dark=${dark:-0}) rc $rc in $(( $(date +%s) - start ))s"
rm -rf $C
