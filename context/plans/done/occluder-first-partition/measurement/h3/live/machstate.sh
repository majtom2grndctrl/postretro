#!/bin/sh
# One line of machine state, read-only: AMD GPU (util, clock, temp, VRAM in use/free),
# CPU thermal (pmset -g therm limits and warnings, xcpm thermal level), screen saver
# and screen lock. usage: machstate.sh
. /Users/dhiester/Projects/Personal/postretro/context/plans/done/occluder-first-partition/measurement/h3/live/env.sh
amd=$("$L/amdstat.sh")
therm=$(pmset -g therm 2>/dev/null | awk '
  /CPU_Speed_Limit/ {s=$NF} /CPU_Scheduler_Limit/ {c=$NF} /CPU_Available_CPUs/ {n=$NF}
  /[Tt]hermal warning/ {tw=($0 ~ /No thermal warning/) ? "none" : $0; gsub(/[ \t]+/, "_", tw)}
  /[Pp]erformance warning/ {pw=($0 ~ /No performance warning/) ? "none" : $0; gsub(/[ \t]+/, "_", pw)}
  END {printf "cpu_speed_limit=%s cpu_sched_limit=%s cpus=%s thermal_warn=%s perf_warn=%s", s, c, n, tw, pw}')
xcpm=$(sysctl -n machdep.xcpm.cpu_thermal_level 2>/dev/null || echo na)
if pgrep -x ScreenSaverEngine >/dev/null; then ss=yes; else ss=no; fi
lk=$(ioreg -n Root -d1 -a 2>/dev/null | grep -A1 CGSSessionScreenIsLocked | tail -1 | tr -d '\t<>/ ')
echo "$(stamp) ${amd}| ${therm} xcpm_thermal=${xcpm} screensaver=${ss} locked=${lk:-absent}"
