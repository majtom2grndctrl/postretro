#!/bin/sh
# usage: cap1.sh tag prl x y z yaw pitch   (eye position, engine m)
D=/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/8f487b87-e062-49e0-9ae8-817344ae0304/scratchpad/frame-cost
cat > $D/cap/$1.json <<J
{ "map": "$2", "camera": { "position": [$3, $4, $5], "yaw_deg": $6, "pitch_deg": $7 },
  "resolution": [1280, 720], "output": "$D/cap/$1.png",
  "measurement": { "report": "$D/cap/$1.report.json", "warmup_frames": 180, "sample_frames": 1 } }
J
cd /Users/dhiester/Projects/Personal/postretro
POSTRETRO_SH_STREAMING=sync-proof RUST_LOG=info ./target/debug/postretro --mod dev --capture $D/cap/$1.json > $D/cap/$1.log 2>&1; echo rc=$?
