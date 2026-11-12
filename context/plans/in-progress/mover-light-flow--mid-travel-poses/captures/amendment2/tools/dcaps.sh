#!/bin/bash
cd /c/Users/danhi/AppData/Local/Temp/claude/80d2a19114937a96/8904fe45-929f-4f76-b328-129186a2a8bd/scratchpad/amend2/darkb/tools
P=../../venv/Scripts/python.exe
J=${J:-5}
set -e
# Rest frames first (quick rest check).
for v in door_ceiling sweepfix sweepwest walledge floor east west; do $P dcap.py batch $J $v head,dark cur,r40 0,1; done
# Ruling B band (static indirect only) and the ceiling.
$P dcap.py batch $J door_ceiling dark,dtruth cur,r40 0.1:0.94:43 02
$P dcap.py batch $J door_ceiling shift r40 0.1:0.94:43 02
$P dcap.py batch $J door_ceiling dark,dtruth cur,r40 0.1:0.6:135
$P dcap.py batch $J door_ceiling dark,dtruth cur,r40 0:1:101
# Sweeper faces.
$P dcap.py batch $J sweepfix dark,dtruth cur,r40 0:0.5:129
$P dcap.py batch $J sweepwest head,dark,truth,dtruth cur,r40 0:0.5:129
$P dcap.py batch $J east head,dark,truth,dtruth cur,r40 0:1:101
$P dcap.py batch $J west head,dark,truth,dtruth cur,r40 0:1:101
# Wall.
$P dcap.py batch $J walledge dark,dtruth cur,r40 0.1:0.45:146
# Floor (eighth-power view).
$P dcap.py batch $J floor head,dark,dtruth cur,r40 0,0.2,0.4,0.5,0.6,0.7,0.75,0.8,0.82,0.84,0.86,0.88,0.9,0.92,0.95,0.97,1.0
echo CAPS DONE
