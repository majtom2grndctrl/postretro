#!/bin/sh
# One live H3 run: launch the frozen windowed engine (dev, dev-tools) at a start pose,
# wait for its [CpuTiming] windows (SETTLE discarded + CPU_WIN kept), then attach a
# Metal System Trace for TRACE_S seconds (0 = none), export the GPU intervals, reduce
# them with gpulive.py, kill the engine. The AMD GPU is polled every second throughout.
# usage: run_live.sh <outdir> <tag> <side> <map> <start_pose x,y,z,yaw,pitch> <settle> <cpu_windows> <trace_s>
# writes <outdir>/<tag>.{log,meta,amd,cpu.txt[,gpu.txt]}
# exit: 0 run complete (meta has verdict=OK), 2 screen saver, 3 screen locked,
#       4 engine produced too few windows / exited / timed out, 5 bad input
. /Users/dhiester/Projects/Personal/postretro/context/plans/in-progress/occluder-first-partition/measurement/h3/live/env.sh
O=$1; TAG=$2; SIDE=$3; MAP=$4; POSE=$5; SETTLE=$6; CPU_WIN=$7; TRACE_S=$8
PRL=$(prl_of "$SIDE" "$MAP"); BR=$(baked_of "$SIDE")
LOGF=$O/$TAG.log; M=$O/$TAG.meta; A=$O/$TAG.amd; T=$O/trace/$TAG
mkdir -p "$O/trace"
[ -f "$PRL" ] || { echo "verdict=BAD_INPUT no PRL $PRL" > "$M"; exit 5; }
{ echo "tag=$TAG side=$SIDE map=$MAP pose=$POSE settle=$SETTLE cpu_windows=$CPU_WIN trace_s=$TRACE_S"
  echo "prl=$PRL baked=$BR"
  echo "start=$(stamp)"
  echo "pre: $("$L/machstate.sh")"; } > "$M"
if pgrep -x ScreenSaverEngine >/dev/null; then echo "verdict=SCREENSAVER_PRE" >> "$M"; exit 2; fi
if screen_locked; then echo "verdict=SCREEN_LOCKED_PRE" >> "$M"; exit 3; fi

: > "$A"
( while :; do echo "$(date +%T) $("$L/amdstat.sh")"; sleep 1; done ) >> "$A" &
APOLL=$!

cd "$W" || exit 5
POSTRETRO_CPU_TIMING=1 POSTRETRO_GPU_TIMING=1 RUST_LOG=info \
  "$B/postretro" --mod dev --baked-root "$BR" --start-pose="$POSE" "$PRL" > "$LOGF" 2>&1 < /dev/null &
P=$!
caffeinate -dimsu -w $P < /dev/null &
need=$((SETTLE + CPU_WIN)); t=0; n=0; why=""
while :; do
  sleep 1; t=$((t + 1))
  n=$(grep -c '\[CpuTiming\]' "$LOGF")
  [ "$n" -ge "$need" ] && break
  kill -0 $P 2>/dev/null || { why=ENGINE_EXITED; break; }
  [ $t -ge 240 ] && { why=TIMEOUT; break; }
done
echo "cpu_done t=$t windows=$n at $(stamp) ${why}" >> "$M"
pgrep -x ScreenSaverEngine >/dev/null && echo "SCREENSAVER_DURING" >> "$M"
screen_locked && echo "SCREEN_LOCKED_DURING" >> "$M"

if [ -z "$why" ] && [ "$TRACE_S" -gt 0 ]; then
  rm -rf "$T.trace"
  echo "$(date +%T) trace_start" >> "$A"
  echo "trace_start $(stamp)" >> "$M"
  xcrun xctrace record --template 'Metal System Trace' --attach $P --time-limit "${TRACE_S}s" \
    --no-prompt --output "$T.trace" > "$T.rec.log" 2>&1 < /dev/null &
  X=$!; i=0
  while kill -0 $X 2>/dev/null && [ $i -lt 120 ]; do sleep 1; i=$((i + 1)); done
  if kill -0 $X 2>/dev/null; then kill $X; sleep 2; kill -9 $X 2>/dev/null; echo "XCTRACE_WATCHDOG" >> "$M"; fi
  echo "$(date +%T) trace_end" >> "$A"
  echo "trace_end $(stamp) record_s=$i" >> "$M"
  pgrep -x ScreenSaverEngine >/dev/null && echo "SCREENSAVER_DURING" >> "$M"
  screen_locked && echo "SCREEN_LOCKED_DURING" >> "$M"
fi

kill $P 2>/dev/null; sleep 2; kill -9 $P 2>/dev/null
kill $APOLL 2>/dev/null
sleep 1
echo "post: $("$L/machstate.sh")" >> "$M"

python3 "$L/cpuwin.py" "$LOGF" "$SETTLE" > "$O/$TAG.cpu.txt" 2>&1
if [ -d "$T.trace" ]; then
  xcrun xctrace export --input "$T.trace" \
    --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]' > "$T.gpu.xml" 2>/dev/null
  echo "export_bytes=$(stat -f %z "$T.gpu.xml" 2>/dev/null)" >> "$M"
  fms=$(python3 "$L/cpuwin.py" "$LOGF" "$SETTLE" --field total)
  python3 "$L/gpulive.py" "$T.gpu.xml" "${fms:-nan}" > "$O/$TAG.gpu.txt" 2>&1
  if [ "${KEEP_XML:-0}" = 1 ]; then gzip -f "$T.gpu.xml"; else rm -f "$T.gpu.xml"; fi
  rm -rf "$T.trace"
fi
clean_ktrace

{ echo "extents: $(grep -m1 'render extents:' "$LOGF" | sed 's/.*render extents: //')"
  echo "vsync: $(grep -m1 -oE 'vsync (on|off)' "$LOGF")"
  echo "warn_lines=$(grep -c ' WARN ' "$LOGF") error_lines=$(grep -c ' ERROR ' "$LOGF")"
  grep -q 'invalid --start-pose' "$LOGF" && echo "INVALID_START_POSE"
  echo "end=$(stamp)"; } >> "$M"
if grep -q 'INVALID_START_POSE' "$M"; then echo "verdict=BAD_INPUT start pose rejected" >> "$M"; exit 5; fi
if [ -n "$why" ] || [ "$n" -lt "$need" ]; then echo "verdict=NO_FRAMES windows=$n $why" >> "$M"; exit 4; fi
if grep -q 'SCREENSAVER_DURING' "$M"; then echo "verdict=SCREENSAVER_DURING" >> "$M"; exit 2; fi
if grep -q 'SCREEN_LOCKED_DURING' "$M"; then echo "verdict=SCREEN_LOCKED_DURING" >> "$M"; exit 3; fi
echo "verdict=OK" >> "$M"
exit 0
