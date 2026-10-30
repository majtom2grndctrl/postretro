#!/bin/bash
# usage: run1.sh <tag> <prl> <baked_root> <x,y,z,yaw,pitch> [cpu_windows] [trace_secs]
# One H3 run: launch the frozen engine at a start pose under caffeinate, wait for the
# level's first CpuTiming window, discard SETTLE windows, keep CPU_WIN untraced windows,
# then attach a Metal System Trace for TRACE_S seconds, export the GPU intervals, kill.
set -u
S=/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/25609316-dd73-4754-993f-da7529caa8f7/scratchpad/h3
R=/Users/dhiester/Projects/Personal/postretro
TAG=$1; PRL=$2; BR=$3; POSE=$4; CPU_WIN=${5:-8}; TRACE_S=${6:-6}; SETTLE=2
L=$S/logs/$TAG.log; M=$S/logs/$TAG.meta; T=$S/trace/$TAG
if pgrep -x ScreenSaverEngine >/dev/null; then echo "SCREENSAVER_PRE" > $M; exit 2; fi
{ echo "tag=$TAG prl=$PRL baked=$BR pose=$POSE start=$(date '+%F %T')"; echo "pre: $($S/gpu.sh)"; } > $M
cd $R
POSTRETRO_CPU_TIMING=1 POSTRETRO_GPU_TIMING=1 RUST_LOG=info $S/bin/postretro --mod dev --baked-root "$BR" --start-pose="$POSE" "$PRL" > $L 2>&1 &
P=$!
caffeinate -dimsu -w $P &
need=$((SETTLE+CPU_WIN)); t=0
while :; do
  sleep 1; t=$((t+1))
  n=$(grep -c "\[CpuTiming\]" $L)
  [ "$n" -ge "$need" ] && break
  kill -0 $P 2>/dev/null || { echo "ENGINE_EXITED t=$t" >> $M; break; }
  [ $t -ge 240 ] && { echo "TIMEOUT windows=$n" >> $M; break; }
  [ $t -eq 20 ] && echo "during: $($S/gpu.sh)" >> $M
done
echo "cpu_done t=$t windows=$n at $(date -u '+%FT%TZ')" >> $M
pgrep -x ScreenSaverEngine >/dev/null && echo "SCREENSAVER_DURING" >> $M
if [ "$TRACE_S" -gt 0 ] && kill -0 $P 2>/dev/null; then
  rm -rf $T.trace
  echo "trace_start $(date -u '+%FT%TZ')" >> $M
  xcrun xctrace record --template 'Metal System Trace' --attach $P --time-limit ${TRACE_S}s --no-prompt --output $T.trace > $T.rec.log 2>&1 &
  X=$!; i=0
  while kill -0 $X 2>/dev/null && [ $i -lt 120 ]; do sleep 1; i=$((i+1)); done
  kill -0 $X 2>/dev/null && { kill $X; sleep 2; kill -9 $X 2>/dev/null; echo "XCTRACE_WATCHDOG" >> $M; }
  echo "trace_end $(date -u '+%FT%TZ')" >> $M
  echo "during_trace_end: $($S/gpu.sh)" >> $M
fi
pgrep -x ScreenSaverEngine >/dev/null && echo "SCREENSAVER_DURING" >> $M
kill $P 2>/dev/null; sleep 2; kill -9 $P 2>/dev/null
sleep 1
echo "post: $($S/gpu.sh)" >> $M
pgrep -x ScreenSaverEngine >/dev/null && echo "SCREENSAVER_POST" >> $M
if [ -d $T.trace ]; then
  xcrun xctrace export --input $T.trace --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]' > $T.gpu.xml 2>/dev/null
  echo "export_bytes=$(stat -f %z $T.gpu.xml 2>/dev/null)" >> $M
fi
echo "end=$(date '+%F %T')" >> $M
