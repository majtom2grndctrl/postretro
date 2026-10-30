#!/bin/sh
# usage: run1.sh <tag> <prl> <x,y,z,yaw,pitch> [secs] [novsync] [shot]
R=/Users/dhiester/Projects/Personal/postretro
D=/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/8f487b87-e062-49e0-9ae8-817344ae0304/scratchpad/frame-cost
if pgrep ScreenSaverEngine >/dev/null; then echo "SCREENSAVER PRESENT - discard" ; exit 2; fi
echo "pre: $($D/gpu.sh)" > $D/logs/$1.meta
cd $R
POSTRETRO_CPU_TIMING=1 POSTRETRO_GPU_TIMING=1 RUST_LOG=info /private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/8f487b87-e062-49e0-9ae8-817344ae0304/scratchpad/frame-cost/bin/postretro --mod dev --start-pose "$3" "$2" > $D/logs/$1.log 2>&1 &
P=$!
N=${4:-30}
i=0; while [ $i -lt $N ]; do sleep 1; i=$((i+1))
 [ $i -eq $((N-3)) ] && echo "during: $($D/gpu.sh)" >> $D/logs/$1.meta
 if [ "$6" = shot ] && [ $i -eq $((N-5)) ]; then wid=$(osascript -l JavaScript $D/winid.js $P | awk '$3=="layer=0"{split($2,a,"x"); if (a[2]>m){m=a[2]; id=$1}} END{print id}'); echo "wid=$wid" >> $D/logs/$1.meta; [ -n "$wid" ] && screencapture -x -o -l$wid $D/logs/$1.png; fi
 pgrep ScreenSaverEngine >/dev/null && echo "SCREENSAVER APPEARED" >> $D/logs/$1.meta
done
kill $P; sleep 2; kill -9 $P 2>/dev/null
echo "post: $($D/gpu.sh)" >> $D/logs/$1.meta
