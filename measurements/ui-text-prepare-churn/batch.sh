#!/bin/bash
# usage: batch.sh <label-prefix> <spec>...   spec = before:campaign | after:hallway [ :sample ]
# Runs each spec in order under one caffeinate hold, with a cool-down gap between runs.
# Env: S (scratchpad holding bin-*/, wt-*/, foreground, lockstate), GAP (seconds, default 45).
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
prefix=$1; shift
export FOREGROUND_BIN=$S/foreground LOCKSTATE_BIN=$S/lockstate
for spec in "$@"; do
  IFS=: read -r which map extra <<<"$spec"
  case $map in
    campaign) prl=campaign-test.prl ;;
    hallway) prl=stress-warren-hallway-inspection.prl ;;
  esac
  flag=""; [ "${extra:-}" = sample ] && flag=--sample
  label="$prefix-$which-$map"
  python3 "$HERE/run.py" "$label" "$S/bin-$which" "$S/wt-$which" "$prl" $flag \
    | python3 -c "import json,sys;d=json.loads(sys.stdin.read());print({k:d.get(k) for k in ['label','elapsed','exit','timing_windows','invalid','foreground_all_true_after_activate','cpu_speed_limit_before']},flush=True)"
  sleep "${GAP:-45}"
done
