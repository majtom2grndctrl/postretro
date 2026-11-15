#!/bin/bash
# Task 25c captures, in order: rest, exit/entry windows, whole-trip band, hand-over series, blotch views.
cd /c/Users/danhi/AppData/Local/Temp/claude/80d2a19114937a96/8904fe45-929f-4f76-b328-129186a2a8bd/scratchpad/amend2/final/tools
P=../venv/Scripts/python.exe
J=${J:-5}
set -e
# f. Rest frames (final HEAD; the pre-amendment and ramp-alone references are in amend2/cap).
for v in door_ceiling sweepfix sweepwest walledge floor east west wall_owner sweeper_east; do $P fcap.py batch $J $v head 0,1; done
# d. Lintel crossings, every window at frame/8 (HEAD, ramp-alone, prototype dark + r40).
EX=$($P fexit.py ts)
$P fcap.py batch $J door_ceiling head,ohead,odark "$EX"
# b/e. Whole trip at room frame spacing, mask 02 and full frame.
$P fcap.py batch $J door_ceiling head,truth,odark,ohead 0:1:269 02
$P fcap.py batch $J door_ceiling head,truth,odark,ohead 0:1:269
# a. Hand-over series (HEAD and the new-rule truth).
$P fcap.py batch $J sweepfix head,truth 0:0.5:129
$P fcap.py batch $J walledge head,truth 0.1:0.45:146
$P fcap.py batch $J door_ceiling head,truth 0.1:0.6:135
# c. Blotch views.
$P fcap.py batch $J sweepwest head,truth 0:0.5:129
$P fcap.py batch $J east head,truth 0:1:101
$P fcap.py batch $J west head,truth 0:1:101
echo CAPS DONE
