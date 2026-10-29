#!/bin/sh
# 9m driver. usage: drive.sh <outdir> <gate_C> <spec>...   spec = side,label,mode   mode = def|val0|tp|tpval0
. /Users/dhiester/Projects/Personal/postretro/context/plans/in-progress/occluder-first-partition/measurement/h3/live/env.sh
S=/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/6c31b3a1-1c96-4083-b143-b540adf0af2f/scratchpad/9m
O=$1; G=$2; shift 2; mkdir -p "$O"; ST=$O/status
export TPX=$S/tp; mkdir -p "$TPX"
regime() { awk -v t="$1" 'BEGIN{exit !(t+0 > 49.5 || t=="nan" || t=="")}'; }
sent() {
  "$L/gate.sh" "$G" 30 600 "$O/gate.log" >> "$ST"
  r=$("$L/sentinel.sh" "$O" "$1"); echo "$(date +%T) SENTINEL $r" >> "$ST"
  tot=$(echo "$r" | grep -oE 'total_ms=[0-9.na]+' | cut -d= -f2)
  if regime "$tot"; then echo "$(date +%T) REGIME_ONSET sentinel=$tot" >> "$ST"; exit 9; fi
}
n=0; sent sentinel_a
for spec in "$@"; do
  side=${spec%%,*}; rest=${spec#*,}; label=${rest%%,*}; mode=${rest#*,}
  line=$(grep "^$label " "$L/poses.live"); map=$(echo "$line" | awk '{print $2}'); pose=$(echo "$line" | awk '{print $3}')
  tag=$(echo "$map.$label.$side.$mode" | tr ':/@' '___')
  [ -f "$O/$tag.meta" ] && grep -q verdict=OK "$O/$tag.meta" && { echo "skip $tag" >> "$ST"; continue; }
  g=$("$L/gate.sh" "$G" 30 600 "$O/gate.log")
  case $mode in val0|tpval0) VAL=0 ;; *) VAL= ;; esac
  case $mode in tp|tpval0) RUNNER=$S/run_tp.sh; TR=6 ;; *) RUNNER=$L/run_live.sh; TR=0 ;; esac
  case $mode in head) RUNNER=$S/run_bin.sh; export BIN=/Users/dhiester/Projects/Personal/postretro/target/occluder-9m/bin-head ;; nd) RUNNER=$S/run_bin.sh; export BIN=/Users/dhiester/Projects/Personal/postretro/target/occluder-9m/bin-objc2nd ;; *) unset BIN ;; esac
  if [ -n "$VAL" ]; then
    WGPU_VALIDATION_INDIRECT_CALL=0 POSTRETRO_UPLOAD_TIMING=1 "$RUNNER" "$O" "$tag" "$side" "$map" "$pose" 2 6 $TR; rc=$?
  else
    POSTRETRO_UPLOAD_TIMING=1 "$RUNNER" "$O" "$tag" "$side" "$map" "$pose" 2 6 $TR; rc=$?
  fi
  c=$O/$tag.cpu.txt
  f() { grep -oE "(^| )$1=[0-9.na]+" "$c" | head -1 | cut -d= -f2; }
  echo "$(date +%T) RUN $tag rc=$rc $g total=$(f total) work=$(f work) submit=$(f render_submit) record=$(f render_record) walk=$(f portal_walk) tests=$(f walk_considered) val=$(grep -m1 -i 'indirect' "$O/$tag.log" | grep -io 'validation[^,]*' | head -1)" >> "$ST"
  [ $rc -ne 0 ] && { echo "STOP rc=$rc" >> "$ST"; exit $rc; }
  tot=$(f total); if regime "$tot"; then sent confirm_$n; fi
  n=$((n+1)); [ $((n % 6)) -eq 0 ] && sent sentinel_$n
done
sent sentinel_end
echo "$(date +%T) DONE" >> "$ST"
