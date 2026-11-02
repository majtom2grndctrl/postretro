#!/bin/sh
# usage: trace1.sh tag prl pose
D=/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/8f487b87-e062-49e0-9ae8-817344ae0304/scratchpad/frame-cost
pgrep ScreenSaverEngine && { echo SCREENSAVER; exit 2; }
cd /Users/dhiester/Projects/Personal/postretro
rm -rf $D/trace/$1.trace
xcrun xctrace record --template 'Metal System Trace' --time-limit 10s --output $D/trace/$1.trace --env POSTRETRO_CPU_TIMING=1 --env RUST_LOG=info --launch -- $D/bin/postretro --mod dev --start-pose "$3" "$2" > $D/trace/$1.rec.log 2>&1 &
X=$!
i=0; while kill -0 $X 2>/dev/null && [ $i -lt 400 ]; do sleep 1; i=$((i+1)); done
kill -0 $X 2>/dev/null && { kill $X; sleep 2; kill -9 $X 2>/dev/null; echo WATCHDOG; }
pkill -f frame-cost/bin/postretro
sleep 1; echo done
