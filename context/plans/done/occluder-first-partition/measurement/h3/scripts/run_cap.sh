#!/bin/bash
# usage: run_cap.sh <tag> <prl> <baked_root> <x> <y> <z> <yaw> <pitch>
# One H3 offline run: a 4K static capture (GPU saturated, rendering_pipeline.md §12) at an
# eye pose, under caffeinate, with the AMD GPU polled every second. Writes
# capruns/<tag>.{json,report.json,log,amd,meta}. Exit 2 on a screen saver.
set -u
S=/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/25609316-dd73-4754-993f-da7529caa8f7/scratchpad/h3
R=/Users/dhiester/Projects/Personal/postretro
TAG=$1; PRL=$2; BR=$3; O=$S/capruns/$TAG
if pgrep -x ScreenSaverEngine >/dev/null; then echo "SCREENSAVER_PRE" > $O.meta; exit 2; fi
cat > $O.json <<J
{ "map": "$PRL", "camera": { "position": [$4, $5, $6], "yaw_deg": $7, "pitch_deg": $8 },
  "resolution": [3840, 2160], "output": "$O.png",
  "measurement": { "report": "$O.report.json", "warmup_frames": 240, "sample_frames": 300 } }
J
locked=$(ioreg -n Root -d1 -a | grep -A1 CGSSessionScreenIsLocked | tail -1 | tr -d '\t<>/')
{ echo "tag=$TAG prl=$PRL baked=$BR eye=$4,$5,$6 yaw=$7 pitch=$8 start=$(date '+%F %T') screen_locked=$locked"; echo "pre: $($S/amdstat.sh)"; } > $O.meta
: > $O.amd
( while :; do $S/amdstat.sh >> $O.amd; sleep 1; done ) & W=$!
cd $R
s=$(date +%s)
POSTRETRO_SH_STREAMING=sync-proof POSTRETRO_CPU_TIMING=1 RUST_LOG=info caffeinate -dimsu $S/bin/postretro-capture --mod dev --baked-root "$BR" --capture $O.json > $O.log 2>&1
rc=$?
kill $W 2>/dev/null; wait $W 2>/dev/null
rm -f $O.png
{ echo "rc=$rc wall=$(( $(date +%s) - s ))"; echo "post: $($S/amdstat.sh)"; } >> $O.meta
pgrep -x ScreenSaverEngine >/dev/null && { echo "SCREENSAVER_POST" >> $O.meta; exit 2; }
exit 0
