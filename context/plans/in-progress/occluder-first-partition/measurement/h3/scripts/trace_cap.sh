#!/bin/bash
# usage: trace_cap.sh <tag>   (re-runs capruns/<tag>.json under a Metal System Trace)
S=/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/25609316-dd73-4754-993f-da7529caa8f7/scratchpad/h3
H=/Users/dhiester/Projects/Personal/postretro/context/plans/in-progress/occluder-first-partition/measurement/h3
T=$1; J=$S/capruns/$T.json; O=$S/trace/$T
pgrep -x ScreenSaverEngine >/dev/null && { echo SCREENSAVER; exit 2; }
BR=$(grep -o 'baked=[^ ]*' $S/capruns/$T.meta | cut -d= -f2)
sed "s#$S/capruns/$T.report.json#$O.report.json#" $J > $O.json
rm -rf $O.trace
cd /Users/dhiester/Projects/Personal/postretro
( while :; do $S/amdstat.sh >> $O.amd; sleep 1; done ) & W=$!
caffeinate -dimsu xcrun xctrace record --template 'Metal System Trace' --time-limit 90s --no-prompt --output $O.trace \
  --env POSTRETRO_SH_STREAMING=sync-proof --env POSTRETRO_CPU_TIMING=1 --env RUST_LOG=info \
  --launch -- $S/bin/postretro-capture --mod dev --baked-root $BR --capture $O.json > $O.rec.log 2>&1
kill $W
xcrun xctrace export --input $O.trace --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]' > $O.gpu.xml 2>/dev/null
python3 $H/gpuframes.py $O.gpu.xml 60 > $O.frames.txt
python3 -c "import json;c=json.load(open('$O.report.json'))['cpu_completion'];print(f'completion median {c[\"median_ms\"]:.2f} p95 {c[\"p95_ms\"]:.2f}')" >> $O.frames.txt
rm -rf $O.trace $O.gpu.xml; rm -f $TMPDIR/instruments*.ktrace
cat $O.frames.txt
