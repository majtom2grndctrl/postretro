#!/bin/sh
# 9m release driver (from 9m/tools/drive.sh). usage: drive_rel.sh <outdir> <gate_C> <spec>...
# spec = side,label,rep,trace_s   (release binary at $BIN; sentinel = frozen dev H3 binary, fc:P1 before, as in 9m)
. /Users/dhiester/Projects/Personal/postretro/context/plans/done/occluder-first-partition/measurement/h3/live/env.sh
RB=/Users/dhiester/Projects/Personal/postretro/context/plans/done/occluder-first-partition/measurement/9m/tools/run_bin.sh
export BINREL=/Users/dhiester/Projects/Personal/postretro/target/occluder-9m/bin
O=$1; G=$2; shift 2; mkdir -p "$O"; ST=$O/status
regime() { awk -v t="$1" 'BEGIN{exit !(t+0 > 49.5 || t=="nan" || t=="")}'; }
sent() {
  g=$("$L/gate.sh" "$G" 30 600 "$O/gate.log"); echo "$(date +%T) $g" >> "$ST"
  unset BIN
  r=$("$L/sentinel.sh" "$O" "$1"); echo "$(date +%T) SENTINEL $r" >> "$ST"
  tot=$(echo "$r" | grep -oE 'total_ms=[0-9.na]+' | cut -d= -f2)
  if regime "$tot"; then echo "$(date +%T) REGIME_ONSET sentinel=$tot" >> "$ST"; exit 9; fi
}
n=0; [ -f "$O/sentinel_a.meta" ] && grep -q verdict=OK "$O/sentinel_a.meta" || sent sentinel_a
for spec in "$@"; do
  side=${spec%%,*}; rest=${spec#*,}; label=${rest%%,*}; rest=${rest#*,}; rep=${rest%%,*}; tr=${rest#*,}
  line=$(grep "^$label " "$L/poses.live"); map=$(echo "$line" | awk '{print $2}'); pose=$(echo "$line" | awk '{print $3}')
  tag=$(echo "$map.$label.$side.rel.r$rep" | tr ':/@' '___')
  [ -f "$O/$tag.meta" ] && grep -q verdict=OK "$O/$tag.meta" && { echo "skip $tag" >> "$ST"; continue; }
  g=$("$L/gate.sh" "$G" 30 600 "$O/gate.log")
  cw=0; while ! "$L/machstate.sh" | grep -q 'cpu_speed_limit=100 ' && [ $cw -lt 300 ]; do sleep 5; cw=$((cw+5)); done; g="$g cpuwait=$cw"
  BIN=$BINREL POSTRETRO_UPLOAD_TIMING=1 "$RB" "$O" "$tag" "$side" "$map" "$pose" 2 6 "$tr"; rc=$?
  c=$O/$tag.cpu.txt
  f() { grep -oE "(^| )$1=[0-9.na]+" "$c" | head -1 | cut -d= -f2; }
  gpu=$(grep -oE 'gpu_ms_per_frame[a-z_]*=[0-9.na]+' "$O/$tag.gpu.txt" 2>/dev/null | head -1)
  echo "$(date +%T) RUN $tag rc=$rc $g total=$(f total) work=$(f work) submit=$(f render_submit) record=$(f render_record) walk=$(f portal_walk) tests=$(f walk_considered) $gpu" >> "$ST"
  [ $rc -ne 0 ] && { echo "STOP rc=$rc" >> "$ST"; exit $rc; }
  tot=$(f total); if regime "$tot"; then sent confirm_$n; fi
  n=$((n+1)); [ $((n % 6)) -eq 0 ] && sent sentinel_$n
done
sent sentinel_end
echo "$(date +%T) DONE" >> "$ST"
