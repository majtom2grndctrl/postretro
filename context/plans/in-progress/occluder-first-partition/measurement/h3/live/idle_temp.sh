#!/bin/sh
# Idle calibration for the thermal gate: polls the AMD GPU every 5 s for N minutes
# (default 10) with nothing else running, logs every sample, then prints min / median /
# max temperature, clock and VRAM in use, and a suggested GATE_TEMP_C (idle median + 4 C).
# Holds caffeinate -dimsu so the display stays awake, as the batch will.
# usage: idle_temp.sh [minutes]     log: target/occluder-h3live/idle-<date>.log
. /Users/dhiester/Projects/Personal/postretro/context/plans/in-progress/occluder-first-partition/measurement/h3/live/env.sh
MINS=${1:-10}; LOGF=$W/target/occluder-h3live/idle-$(date +%Y%m%d-%H%M).log
mkdir -p "$(dirname "$LOGF")"
caffeinate -dimsu -w $$ < /dev/null &
echo "start: $("$L/machstate.sh")" > "$LOGF"
end=$(( $(date +%s) + MINS * 60 ))
while [ "$(date +%s)" -lt "$end" ]; do
  echo "$(date +%T) $("$L/amdstat.sh")" >> "$LOGF"; sleep 5
done
echo "end: $("$L/machstate.sh")" >> "$LOGF"
python3 - "$LOGF" <<'PY'
import re, statistics, sys
t, mhz, vram = [], [], []
for l in open(sys.argv[1]):
    m = re.search(r"temp=(\d+)", l)
    if m and not l.startswith(("start:", "end:")):
        t.append(int(m.group(1)))
        c = re.search(r"mhz=(\d+)", l); v = re.search(r"inUseVidMemoryBytes=(\d+)", l)
        if c: mhz.append(int(c.group(1)))
        if v: vram.append(int(v.group(1)) / 2**30)
if not t:
    sys.exit("no temperature samples")
q = lambda v: f"{min(v):.0f} / {statistics.median(v):.1f} / {max(v):.0f}" if v else "-"
print(f"samples {len(t)}  temp C min/median/max {q(t)}  clock MHz {q(mhz)}")
print(f"VRAM in use GiB min/median/max {min(vram):.2f} / {statistics.median(vram):.2f} / {max(vram):.2f}" if vram else "VRAM -")
print(f"suggested GATE_TEMP_C = {round(statistics.median(t)) + 4}")
PY
echo "log: $LOGF"
