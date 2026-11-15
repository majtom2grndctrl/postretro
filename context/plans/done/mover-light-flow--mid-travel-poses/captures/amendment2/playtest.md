# Amendment 2: in-engine A/B for ruling A (Windows)

Run in PowerShell from the repo root. Each launch is windowed. Quit the engine before you switch forms: the switch is read once, when the mover pipeline is built.

## What runs

- **Engine.** `target\a2-playtest\postretro.exe` is a release build of HEAD plus the uncommitted `AMEND2-PROTO` switch, built with `--features capture`. It is a hard link to the Amendment 2 worker's scratch binary, so a rebuild of `target\release` doesn't replace it.
- **Maps.** `target\a2-playtest\maps\{mover-setpiece,enemy-movers}.prl` are cold `--release` bakes with a fresh cache.
  - They are byte-identical to `content\dev\maps\*.prl` as of 2026-11-12.
  - They are HEAD-equivalent. The only compiler change since that `prl-build` (task 23's nudge) touches authored `light_poses`, and neither map sets one.
- **Forms.**

  | Form | `POSTRETRO_PROTO_MOVER_SH` |
  |---|---|
  | Today: cosine, 0.1-cell lookup | unset |
  | `r40`: surface-point ramp, 0.4-cell lookup (recommended) | `form=1;ramp=0.4;offset=0.4` |
  | `r25`: the fallback | `form=1;ramp=0.25;offset=0.25` |

- **What the switch covers.** It changes only the mover pass, which lights mover faces: the sweeper, the wall door, and the rising room's ceiling and floor. Static surfaces are unchanged.
- **Unlike the clips.** SH streaming here is the default, not `sync-proof`, and reflections are at your settings.

## Commands

```powershell
cd C:\Users\danhi\Projects\Personal\postretro
$exe  = "$PWD\target\a2-playtest\postretro.exe"
$flags = @('--mod','dev','--app-name','postretro-dev','--baked-root',"$PWD\baked",'--core-root',"$PWD\core",'--windowed')

# r40 (recommended)
$env:POSTRETRO_PROTO_MOVER_SH = 'form=1;ramp=0.4;offset=0.4'
& $exe @flags "$PWD\target\a2-playtest\maps\mover-setpiece.prl"
& $exe @flags "$PWD\target\a2-playtest\maps\enemy-movers.prl"

# Today's form
Remove-Item Env:POSTRETRO_PROTO_MOVER_SH -ErrorAction SilentlyContinue
& $exe @flags "$PWD\target\a2-playtest\maps\mover-setpiece.prl"
& $exe @flags "$PWD\target\a2-playtest\maps\enemy-movers.prl"

# r25 (fallback)
$env:POSTRETRO_PROTO_MOVER_SH = 'form=1;ramp=0.25;offset=0.25'
& $exe @flags "$PWD\target\a2-playtest\maps\enemy-movers.prl"

# Done: clear the switch
Remove-Item Env:POSTRETRO_PROTO_MOVER_SH -ErrorAction SilentlyContinue
```

- **Confirming the form.** With the switch set, the log has one line, `WARN ... kinematic_brush] AMEND2-PROTO mover SH constants: form=1;ramp=0.4;offset=0.4`. No such line means today's form.
- **The log.** Add `$env:RUST_LOG = 'info'` to see it in the console.

## Where to look

- **`mover-setpiece`.** Both movers cycle unattended.
  - The wall door takes about 30 s a cycle. Watch its trailing end face and the north face beside it as it opens.
  - The rising room takes about 21 s a cycle. Ride it, or stand on the roof, and watch two places as it rises:
    - the ceiling along the west wall, and above the doorway;
    - the floor (the old lattice dots).
- **`enemy-movers`.** In bay 2 (x 1280–1536), the sweeper ping-pongs at 4.5 m/s with a 1.5 s wait. Watch its east face from the east side, outside the enemies' 16 m aggro range, or reload once they close in.
- **Known `r40` costs to look for.**
  - The above-door ceiling sub-step near t′ 0.525 of the rise (4.9 levels; the truth has it too).
  - Small rest-frame shifts: the sweeper at its far end, and the closed wall's owner view (worst blocks 7–10).
