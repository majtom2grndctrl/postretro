#!/bin/zsh
# Mac checkpoint driver for camera-path-playback (mac-checkpoint.md steps 3-8).
# Usage: cp_run.sh <out_dir>   (run from repo root)
set -u
OUT=$1
mkdir -p "$OUT"
REPO=$PWD
if ioreg -n Root -d1 -a | grep -A1 CGSSessionScreenIsLocked | grep -q '<true/>'; then
  echo "SCREEN LOCKED: windowed run would stall at Window ready; unlock first"; exit 4
fi

# Step 3: route
target/release/postretro-tool generate-camera-route content/dev/maps/campaign-test.prl \
  --kind far-point --steps 40 -o "$OUT/cp-far.txt" > "$OUT/route-gen.log" 2>&1 || { echo "ROUTE GEN FAILED"; cat "$OUT/route-gen.log"; exit 2; }
echo "route lines: $(wc -l < "$OUT/cp-far.txt") (poses = lines - header)"

# Step 4: ioreg poll (BSD date has no %N; use perl for sub-second stamps)
( while true; do perl -MTime::HiRes=time -e 'printf "%.3f\n", time'; \
    ioreg -r -c IOAccelerator -d 1 | grep -o '"Core Clock(MHz)"=[0-9]*\|"Temperature(C)"=[0-9]*\|"Device Utilization %"=[0-9]*'; \
    sleep 0.5; done ) > "$OUT/cp-ioreg.log" 2>&1 &
IOREG_PID=$!

caffeinate -dimsu -w $$ &

# Step 5: engine
RUST_LOG=info target/profiling/postretro --windowed --camera-route "$OUT/cp-far.txt" \
  --camera-route-hold 900 --camera-route-report "$OUT/cp-far.playback.json" \
  content/dev/maps/campaign-test.prl > "$OUT/cp-engine.log" 2>&1 &
ENGINE_PID=$!
echo "engine pid $ENGINE_PID  t=$(date +%T)"

# Step 6: wait for reveal, then attach
for i in {1..1200}; do
  grep -q '\[Settling\] entry' "$OUT/cp-engine.log" && break
  kill -0 $ENGINE_PID 2>/dev/null || break
  sleep 0.25
done
if ! grep -q '\[Settling\] entry' "$OUT/cp-engine.log"; then
  echo "NO SETTLING ENTRY (engine alive: $(kill -0 $ENGINE_PID 2>/dev/null && echo yes || echo no))"
  kill $ENGINE_PID 2>/dev/null; wait $ENGINE_PID; echo "engine exit $?"
  kill $IOREG_PID; tail -60 "$OUT/cp-engine.log"; exit 3
fi
sleep 8; echo "settled+8s t=$(date +%T); attaching"
xcrun xctrace record --template 'Metal System Trace' --attach $ENGINE_PID --time-limit 20s --no-prompt \
  --output "$OUT/cp-far.trace" > "$OUT/xctrace-record.log" 2>&1 < /dev/null &
XC_PID=$!

# Step 7: engine exit
wait $ENGINE_PID; ENGINE_RC=$?
echo "engine exit $ENGINE_RC t=$(date +%T)" | tee "$OUT/engine-exit.txt"
# watchdog: xctrace gets 1800 s after engine exit (ktrace processing is slow)
for i in {1..1800}; do kill -0 $XC_PID 2>/dev/null || break; sleep 1; done
if kill -0 $XC_PID 2>/dev/null; then echo "xctrace watchdog: killing"; kill -INT $XC_PID; sleep 10; kill -9 $XC_PID 2>/dev/null; fi
wait $XC_PID; echo "xctrace exit $?"
sleep 3; kill $IOREG_PID
ls -la "$OUT"
