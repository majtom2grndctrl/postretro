#!/bin/sh
# Live H3 batch driver (Task 9d's owed live runs), on before / 9l cap26 / 9l cap19.
#
# usage: batch.sh [--check] [--pilot] <GATE_TEMP_C> [MIN_PAUSE_S] [MAX_WAIT_S]
#   --check  print the plan (of the pilot, with --pilot) and check every input;
#            launches nothing, touches no GPU, writes nothing
#   --pilot  regime sentinel + one pose (PILOT_POSE, default rd:promenade) on each side
#   (none)   the full batch: every pose in POSES on each side, then the in-process walk
#            timing (CPU only) and the summary
# GATE_TEMP_C  run only once the AMD GPU is at or below this temperature (calibrate with
#              idle_temp.sh); MIN_PAUSE_S (default 30) minimum pause before every run;
#              MAX_WAIT_S (default 600) gate cap, after which the run goes ahead flagged
#              GATE_TIMEOUT.
# env: BATCH (output dir name under target/occluder-h3live/runs/; re-using a name resumes,
#      skipping runs whose meta says verdict=OK), POSES (default live/poses.live),
#      PILOT_POSE, REPS (1), SENTINEL_EVERY (9 runs), SETTLE (2 windows), CPU_WIN (8
#      windows), TRACE_S (6), REGIME_FACTOR (3), SENTINEL_MAX_BASE_MS (30), KEEP_XML (0),
#      ALLOW_BUSY (0: refuse to start while another engine, bake or build runs).
# Sides rotate per pose (before-cap26-cap19, cap26-cap19-before, cap19-before-cap26, ...)
# so clock and thermal drift spread over the sides, as batch_cap.py did.
# The whole batch holds one caffeinate -dimsu assertion (the screen-lock delay on this
# Mac is immediate; gaps between per-run assertions blanked the display before).
# Stops: REGIME_ONSET (sentinel > REGIME_FACTOR x the batch-start sentinel), REGIME_AT_START,
# SCREEN (saver or lock), NO_FRAMES, GATE_NO_TEMP, LOW_DISK. Exit 0 only when complete.
. /Users/dhiester/Projects/Personal/postretro/context/plans/in-progress/occluder-first-partition/measurement/h3/live/env.sh

MODE=full; CHECK=0
while :; do
  case "$1" in --check) CHECK=1; shift ;; --pilot) MODE=pilot; shift ;; *) break ;; esac
done
GATE=$1; MIN=${2:-30}; MAX=${3:-600}
usage() { sed -n '2,24p' "$0" >&2; exit 64; }
case "$GATE" in ''|*[!0-9]*) usage ;; esac
case "$MIN$MAX" in *[!0-9]*) usage ;; esac
POSES=${POSES:-$L/poses.live}; PILOT_POSE=${PILOT_POSE:-rd:promenade}
REPS=${REPS:-1}; SENTINEL_EVERY=${SENTINEL_EVERY:-9}; SETTLE=${SETTLE:-2}; CPU_WIN=${CPU_WIN:-8}
TRACE_S=${TRACE_S:-6}; FACTOR=${REGIME_FACTOR:-3}; MAXBASE=${SENTINEL_MAX_BASE_MS:-30}
BATCH=${BATCH:-$MODE-$(date +%Y%m%d-%H%M)}; O=$RUNS/$BATCH

plan() { # rep label map pose side, sides rotating one step per pose
  if [ $MODE = pilot ]; then src=$(grep "^$PILOT_POSE " "$POSES"); reps=1; else src=$(cat "$POSES"); reps=$REPS; fi
  echo "$src" | awk -v reps="$reps" -v sides="$SIDES" '
    BEGIN { ns = split(sides, S, " ") }
    /^#/ || NF < 4 { next }
    { n++; lab[n] = $1; map[n] = $2; pose[n] = $3 }
    END { for (r = 1; r <= reps; r++) for (i = 1; i <= n; i++) { k = (r - 1) * n + i - 1
            for (j = 0; j < ns; j++) print r, lab[i], map[i], pose[i], S[(k + j) % ns + 1] } }'
}
tag_of() { echo "$3.$(safe "$2").$5.r$1"; } # rep label map pose side
gt() { awk -v a="$1" -v b="$2" 'BEGIN { exit !(a != "nan" && a != "" && a + 0 > b + 0) }'; }

# ---- check mode: plan and inputs only -------------------------------------------------
if [ $CHECK = 1 ]; then
  bad=0
  echo "== check (nothing is launched); batch dir would be $O"
  for f in "$B/postretro" "$B/scripts-build" "$B/walktime-capped" "$POSES"; do
    if [ -e "$f" ]; then echo "ok  $f $(shasum "$f" | cut -c1-12)"; else echo "MISSING $f"; bad=1; fi
  done
  for s in $SIDES; do for m in occlusion-test stress-street; do
    p=$(prl_of $s $m); if [ -f "$p" ]; then echo "ok  $s $m $p ($(stat -f %z "$p") B)"; else echo "MISSING $p"; bad=1; fi
  done; [ -d "$(baked_of $s)/materials" ] && echo "ok  $s baked $(baked_of $s)" || { echo "MISSING $(baked_of $s)/materials"; bad=1; }; done
  python3 "$L/cpuwin.py" --self-test || bad=1
  python3 -c "import xml.etree.ElementTree" || bad=1
  xcrun xctrace version >/dev/null 2>&1 && echo "ok  xctrace $(xcrun xctrace version 2>&1 | head -1)" || { echo "MISSING xctrace"; bad=1; }
  echo "state: $("$L/machstate.sh")"
  t=$(amd_temp); echo "gpu temp now ${t:-unreadable} C, gate $GATE C ($( [ -n "$t" ] && [ "$t" -le "$GATE" ] && echo would pass || echo would wait))"
  screen_locked && echo "WARNING screen is locked: every live run would render nothing"
  [ "$(free_gb)" -lt 15 ] && { echo "LOW DISK $(free_gb) GB"; bad=1; }
  for p in postretro prl-build rustc cargo; do pgrep -x $p >/dev/null && echo "WARNING $p is running (the batch refuses to start unless ALLOW_BUSY=1)"; done
  n=$(plan | wc -l | tr -d ' ')
  echo "== plan: $n runs (mode $MODE, reps $REPS), settle $SETTLE + $CPU_WIN windows, trace ${TRACE_S}s, sentinel every $SENTINEL_EVERY runs"
  plan | while read -r rep lab map pose side; do
    echo "  $(tag_of $rep $lab $map $pose $side)  --start-pose=$pose  $(prl_of $side $map)"
  done
  ns=$(( (n + SENTINEL_EVERY - 1) / SENTINEL_EVERY + 1 ))
  echo "== estimate: ~$(( (n * (MIN + 80) + ns * (MIN + 30)) / 60 )) min at the gate floor ($MIN s pause + ~80 s per traced run, ~30 s per sentinel; $ns sentinels), longer while the gate waits on temperature"
  [ $bad = 0 ] && echo "CHECK_OK" || echo "CHECK_FAILED"
  exit $bad
fi

# ---- live modes -------------------------------------------------------------------------
mkdir -p "$O" || exit 1
ST=$O/status
log() { echo "$(date +%T) $*" | tee -a "$ST"; }
stop() { log "$*"; log "BATCH_STOPPED"; kill $CAF 2>/dev/null; exit "${STOP_RC:-1}"; }

caffeinate -dimsu -w $$ < /dev/null &
CAF=$!
log "batch $BATCH mode=$MODE gate=${GATE}C min_pause=${MIN}s max_wait=${MAX}s reps=$REPS sentinel_every=$SENTINEL_EVERY caffeinate_pid=$CAF"

if [ "${ALLOW_BUSY:-0}" != 1 ]; then
  for p in postretro prl-build rustc cargo; do
    pgrep -x $p >/dev/null && { STOP_RC=16; stop "BUSY: $p is running; a timed run owns the machine (ALLOW_BUSY=1 overrides)"; }
  done
fi
screen_locked && { STOP_RC=11; stop "SCREEN: locked at start; unlock and re-run (same BATCH resumes)"; }

[ -f "$O/batch.meta" ] || {
  echo "started=$(stamp) head=$(git -C "$W" rev-parse --short=9 HEAD) dirty=$(git -C "$W" status --short | wc -l | tr -d ' ')"
  echo "model=$(sysctl -n hw.model) macos=$(sw_vers -productVersion) build=$(sw_vers -buildVersion)"
  for f in "$B/postretro" "$B/scripts-build" "$B/walktime-capped"; do echo "bin $(shasum "$f" | cut -c1-12) $f"; done
  for s in $SIDES; do for m in occlusion-test stress-street; do p=$(prl_of $s $m); echo "prl $s $m $(stat -f %z "$p") $(shasum "$p" | cut -c1-12)"; done; done
  echo "pmset: $(pmset -g | grep -E ' (displaysleep|sleep|lowpowermode|powernap) ' | tr -s ' ' | tr '\n' ';')"
  echo "displays: $(system_profiler SPDisplaysDataType 2>/dev/null | grep -E 'Chipset Model|Resolution|Main Display' | tr -s ' ' | tr '\n' ';')"
  echo "state: $("$L/machstate.sh")"
} > "$O/batch.meta"
cp "$HOME/Library/Application Support/postretro/settings.toml" "$O/settings.toml" 2>/dev/null

gate() { # label -> logs the gate line; returns gate.sh's code
  g=$("$L/gate.sh" "$GATE" "$MIN" "$MAX" "$O/gate.log"); grc=$?
  log "gate $1: $g"
  [ $grc = 3 ] && { STOP_RC=13; stop "GATE_NO_TEMP: amdstat.sh returns no temperature"; }
  GATE_LINE=$g
  return $grc
}

S_N=0
sentinel() { # why -> sets S_TOT
  S_N=$((S_N + 1)); st=sentinel$(printf %02d $S_N)
  gate "$st"
  out=$("$L/sentinel.sh" "$O" "$st"); src=$?
  log "SENTINEL $1: $out"
  S_TOT=$(echo "$out" | grep -oE 'total_ms=[^ ]+' | cut -d= -f2)
  case $src in
    0) ;;
    2|3) STOP_RC=11; stop "SCREEN: screen saver or lock during $st" ;;
    *) STOP_RC=12; stop "SENTINEL_FAILED: $st rc=$src (no frames; see $O/$st.meta)" ;;
  esac
}

regime_check() { # after a sentinel
  if gt "$S_TOT" "$(awk -v b="$BASE" -v f="$FACTOR" 'BEGIN { print b * f }')"; then
    STOP_RC=10
    stop "REGIME_ONSET sentinel=${S_TOT}ms base=${BASE}ms (> ${FACTOR}x) | $(grep '^pre:' "$O/sentinel$(printf %02d $S_N).meta" | sed 's/^pre: /pre /') | $(grep '^post:' "$O/sentinel$(printf %02d $S_N).meta" | sed 's/^post: /post /')"
  fi
}

# Batch-start sentinel: the regime reference.
if [ -f "$O/base_sentinel" ]; then
  BASE=$(cat "$O/base_sentinel"); S_N=$(ls "$O"/sentinel*.meta 2>/dev/null | wc -l | tr -d ' ')
  log "resume: base sentinel ${BASE}ms"
  sentinel resume; regime_check
else
  sentinel batch-start; BASE=$S_TOT
  gt "$BASE" "$MAXBASE" && { STOP_RC=15; stop "REGIME_AT_START base sentinel ${BASE}ms > ${MAXBASE}ms (healthy is vsync ~16.7 ms); do the cool-down-then-reprobe procedure"; }
  case "$BASE" in nan|'') STOP_RC=12; stop "SENTINEL_FAILED: no total from the base sentinel" ;; esac
  echo "$BASE" > "$O/base_sentinel"
fi

since=0; done_n=0; total_n=$(plan | wc -l | tr -d ' ')
plan > "$O/plan.txt"
while read -r rep lab map pose side <&3; do
  tag=$(tag_of "$rep" "$lab" "$map" "$pose" "$side")
  done_n=$((done_n + 1))
  if grep -q '^verdict=OK' "$O/$tag.meta" 2>/dev/null; then log "skip $tag (already OK)"; continue; fi
  [ "$(free_gb)" -lt 15 ] && { STOP_RC=14; stop "LOW_DISK $(free_gb) GB free"; }
  if [ $since -ge "$SENTINEL_EVERY" ]; then sentinel periodic; regime_check; since=0; fi
  gate "$tag"; grc_run=$?; gflag=""; [ $grc_run = 1 ] && gflag=" GATE_TIMEOUT"
  "$L/run_live.sh" "$O" "$tag" "$side" "$map" "$pose" "$SETTLE" "$CPU_WIN" "$TRACE_S"; rc=$?
  since=$((since + 1))
  echo "gate: $GATE_LINE" >> "$O/$tag.meta"
  c=$(python3 "$L/cpuwin.py" "$O/$tag.log" "$SETTLE" 2>/dev/null)
  tot=$(echo "$c" | grep -oE '(^| )total=[^ ]+' | cut -d= -f2)
  g=$(head -1 "$O/$tag.gpu.txt" 2>/dev/null | grep -oE 'gpu_ms_per_frame=[^ ]+')
  log "[$done_n/$total_n] $tag rc=$rc$gflag total=${tot:-nan} $(echo "$c" | grep -oE '(portal_walk|walk_considered|us_per_test|walk_step_limit|work|render_submit)=[^ ]+' | tr '\n' ' ')${g:-gpu=none}"
  case $rc in
    0) ;;
    2|3) STOP_RC=11; stop "SCREEN: $(grep '^verdict=' "$O/$tag.meta") at $tag; unlock or wake, then re-run with BATCH=$BATCH to resume" ;;
    4) # Too few windows in 240 s: the regime (~225 ms frames) does this too, so check
       # the windows that did arrive before calling it a display problem.
       t_all=$(python3 "$L/cpuwin.py" "$O/$tag.log" 0 --field total 2>/dev/null)
       if gt "$t_all" "$(awk -v b="$BASE" -v f="$FACTOR" 'BEGIN { print b * f }')"; then
         log "NO_FRAMES at $tag with slow windows (total ${t_all}ms): confirming with a sentinel"
         sentinel confirm; regime_check
       fi
       STOP_RC=12; stop "NO_FRAMES at $tag ($(grep '^verdict=' "$O/$tag.meta")); is the engine window in front?" ;;
    *) STOP_RC=12; stop "RUN_FAILED rc=$rc at $tag ($(grep '^verdict=' "$O/$tag.meta"))" ;;
  esac
  # A run whose frame total jumps past the regime threshold gets an immediate sentinel.
  if gt "$tot" "$(awk -v b="$BASE" -v f="$FACTOR" 'BEGIN { print b * f }')"; then
    log "run total ${tot}ms > ${FACTOR}x base: confirming with a sentinel"
    sentinel confirm; regime_check; since=0
  fi
done 3< "$O/plan.txt"

sentinel closing; regime_check
if [ $MODE = full ]; then
  log "in-process walk timing (CPU only)"
  "$L/walk_inproc.sh" "$O" >> "$ST" 2>&1
fi
python3 "$L/collect.py" "$O" > "$O/summary.md" 2>&1
log "summary: $O/summary.md"
log "BATCH_DONE"
kill $CAF 2>/dev/null
exit 0
