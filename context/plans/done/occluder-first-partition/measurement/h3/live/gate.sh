#!/bin/sh
# Thermal gate before a run. Polls the AMD GPU temperature once a second and returns
# once BOTH temp <= GATE_TEMP_C and at least MIN_PAUSE_S have passed. Gives up after
# MAX_WAIT_S and prints GATE_TIMEOUT (the caller runs anyway, flagged).
# Every poll is appended to <poll_log> as "<time> <elapsed_s> <amdstat line>".
# usage: gate.sh <GATE_TEMP_C> <MIN_PAUSE_S> <MAX_WAIT_S> <poll_log>
# stdout: one line, "GATE_OK waited=<s> temp=<C>" | "GATE_TIMEOUT waited=<s> temp=<C>"
#         | "GATE_NO_TEMP waited=<s>" (temperature unreadable for 10 s; exit 3)
# exit: 0 ok, 1 timeout, 3 no temperature
. /Users/dhiester/Projects/Personal/postretro/context/plans/done/occluder-first-partition/measurement/h3/live/env.sh
G=$1; MIN=$2; MAX=$3; LOG=$4
t0=$(date +%s); miss=0
while :; do
  line=$("$L/amdstat.sh")
  now=$(date +%s); e=$((now - t0))
  echo "$(date +%T) $e $line" >> "$LOG"
  t=$(echo "$line" | grep -oE 'temp=[0-9]+' | cut -d= -f2)
  if [ -z "$t" ]; then
    miss=$((miss + 1))
    [ $miss -ge 10 ] && { echo "GATE_NO_TEMP waited=$e"; exit 3; }
  else
    miss=0
    if [ "$e" -ge "$MIN" ] && [ "$t" -le "$G" ]; then echo "GATE_OK waited=$e temp=$t"; exit 0; fi
  fi
  [ "$e" -ge "$MAX" ] && { echo "GATE_TIMEOUT waited=$e temp=${t:-na}"; exit 1; }
  sleep 1
done
