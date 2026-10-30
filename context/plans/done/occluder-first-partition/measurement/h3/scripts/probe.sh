#!/bin/bash
# Regime probe: short 4K capture of occlusion-test P1 cap26 (normal ~34 ms, regime ~225 ms).
i=$1
cat > /private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/25609316-dd73-4754-993f-da7529caa8f7/scratchpad/h3/capruns/probe$i.json <<J
{ "map": "/Users/dhiester/Projects/Personal/postretro/target/occluder-9g/cap26/occlusion-test/occlusion-test.prl", "camera": { "position": [-17.78, 1.5, -13.8176], "yaw_deg": 90, "pitch_deg": 0 },
  "resolution": [3840, 2160], "output": "/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/25609316-dd73-4754-993f-da7529caa8f7/scratchpad/h3/capruns/probe$i.png",
  "measurement": { "report": "/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/25609316-dd73-4754-993f-da7529caa8f7/scratchpad/h3/capruns/probe$i.report.json", "warmup_frames": 60, "sample_frames": 60 } }
J
pre=$(/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/25609316-dd73-4754-993f-da7529caa8f7/scratchpad/h3/amdstat.sh)
cd /Users/dhiester/Projects/Personal/postretro; POSTRETRO_SH_STREAMING=sync-proof RUST_LOG=warn caffeinate -dimsu /private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/25609316-dd73-4754-993f-da7529caa8f7/scratchpad/h3/bin/postretro-capture --mod dev --baked-root /Users/dhiester/Projects/Personal/postretro/target/occluder-9g/baked --capture /private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/25609316-dd73-4754-993f-da7529caa8f7/scratchpad/h3/capruns/probe$i.json > /private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/25609316-dd73-4754-993f-da7529caa8f7/scratchpad/h3/capruns/probe$i.log 2>&1
rm -f /private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/25609316-dd73-4754-993f-da7529caa8f7/scratchpad/h3/capruns/probe$i.png
python3 -c "import json;c=json.load(open('/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/25609316-dd73-4754-993f-da7529caa8f7/scratchpad/h3/capruns/probe$i.report.json'))['cpu_completion'];print('$(date +%T) probe$i median',round(c['median_ms'],1),'| idle pre:','$pre')"
