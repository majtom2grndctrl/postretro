#!/bin/bash
# bake.sh <col> <map>  -- cold --release bake of ws/content/dev/maps/<col>-<map>.map
# (head bakes read the repo map directly), fresh cache dir, deleted after.
A=/c/Users/danhi/AppData/Local/Temp/claude/80d2a19114937a96/8904fe45-929f-4f76-b328-129186a2a8bd/scratchpad/amend2
R=/c/Users/danhi/Projects/Personal/postretro
col=$1; map=$2; W=$A/ws/content/dev/maps
if [ "$col" = head ]; then src=$R/content/dev/maps/$map.map; else src=$W/$col-$map.map; fi
C=$A/cache/$col-$map; rm -rf $C; mkdir -p $C
cd $R
start=$(date +%s)
$A/bin/prl-build.exe "$(cygpath -w $src)" -o "$(cygpath -w $W/$col-$map.prl)" --release --no-tui -v \
  --cache-dir "$(cygpath -w $C)" --baked-root "$(cygpath -w $R/baked)" > $A/logs/bake-$col-$map.log 2>&1
rc=$?
echo "baked $col-$map rc $rc in $(( $(date +%s) - start ))s"
rm -rf $C
