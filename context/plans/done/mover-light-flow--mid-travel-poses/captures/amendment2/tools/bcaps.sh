#!/bin/bash
# Big-room re-capture (2026-11-13), in order: rest, exits/entries windows, whole-trip bands,
# hand-over series, blotch views. Columns head, truth, ramp (bcap.py).
S=/c/Users/danhi/AppData/Local/Temp/claude/80d2a19114937a96/8904fe45-929f-4f76-b328-129186a2a8bd/scratchpad
cd /c/Users/danhi/Projects/Personal/postretro/context/plans/in-progress/mover-light-flow--mid-travel-poses/captures/amendment2/tools
P=$S/amend3/venv/Scripts/python.exe
J=${J:-6}
set -e
# 7. Rest frames on every view.
for v in bw bs bfloor broof barch bch bhall door_ceiling walledge; do $P bcap.py batch $J $v head,ramp,truth 0,1; done
# 4. Exit and entry windows at FRAME/5 (head and ramp alone).
for v in bs bw bfloor broof; do $P bcap.py batch $J $v head,ramp "$($P bexit.py ts $v)"; done
# 1/3/6. Whole trip at 60 fps frame spacing, mask 02 and full frame.
for v in bw bs bfloor; do
  $P bcap.py batch $J $v head,truth,ramp trip 02
  $P bcap.py batch $J $v head,truth,ramp trip
done
# 5. Hand-over series (small room, wall door; the big ceiling rides on the whole-trip series).
$P bcap.py batch $J walledge head,truth,ramp 0.1:0.45:146
$P bcap.py batch $J door_ceiling head,truth,ramp 0.1:0.6:135
# 2. Blotch views (fixed and the roof top).
for v in broof barch bch bhall; do $P bcap.py batch $J $v head,truth,ramp 0:1:101; done
echo CAPS DONE
