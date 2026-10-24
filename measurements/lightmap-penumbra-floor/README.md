# lightmap-penumbra-floor capture stations

Scene JSONs for the brief's two manual capture rows
(`context/plans/in-progress/lightmap-penumbra-floor/index.md`, Acceptance
§Manual). Each station is a fixed camera about 2 m from a baked shadow edge, at
2560×1440. The same station is captured against several bakes of its map, so a
reviewer compares like with like.

## Variants

A scene names its level by `map`, a path to a `.prl`, so each variant is its own
JSON, `scenes/<station>.<variant>.scene.json`, writing
`<station>.<variant>.png`.

| Variant | Bake |
|---|---|
| `floor` | `prl-build` from this branch, floor on (the default) |
| `off` | the same binary with `--no-penumbra-floor` (centre-point visibility) |
| `d001` | floored, `--lightmap-density 0.01` (the owner's reference for hard-light softness) |
| `base` | the pre-change `prl-build` (`lpf-bin/prl-build-base.exe`), specular station only |

The PRLs and PNGs are scratch and not committed. They live beside this
worktree, in `../lpf-bin/capture/` (relative to the workspace root, which is the
working directory `xtask capture` sets):

```
lpf-bin/capture/content/dev/maps/<map>.<variant>.prl
lpf-bin/capture/baked/materials/            .prm sidecars (--baked-root)
lpf-bin/capture/png/<station>.<variant>.png
lpf-bin/capture/logs/                       bake and capture logs
```

The `content/dev/maps` layout is required. Capture finds a level's `.prm`
sidecars from the PRL's own path (`<PRL dir>/../../../baked/materials`, in
`derive_prm_root_dev_layout`) and has no `--baked-root` flag, so a PRL anywhere
else renders every texture as the placeholder.

## Stations

Coordinates are Quake map units (inches, Z up); the JSON holds engine metres
(`engine = (-y, z, -x) * 0.0254`). Yaw equals the Quake angle in degrees,
pitch is positive up, FOV is horizontal. Only the camera comes from the table
below; the geometry notes are what each station looks at.

| Station | Map | Camera (Quake) | yaw / pitch / FOV | What it shows |
|---|---|---|---|---|
| `sst-fan-se` | `soft_shadow_test` | (185, 15, 34) | 137.7 / -24.6 / 80 | The box's south-east base corner (160, 96) on the floor. The five east lights each cast a soft diagonal shadow edge that starts at that contact corner (34° to 60° off the x axis), so the penumbra is near zero at the corner and widens away from it. The key light's axis-aligned ring edge (y = 83.2) crosses the fan. |
| `sst-fan-nw` | `soft_shadow_test` | (60, 214, 36) | -90.0 / -28.6 / 80 | The north-west base corner (96, 160). Same fan, but the edges leave at shallow angles (14° to 40°), the worst case for a bilinear staircase. Runs west to the wall. |
| `smg-wall-diagonal` | `shadowmask-groups-capture` | (550, 475, 345) | 152.1 / -5.1 / 70 | North wall (y = 512). The red light's shadow of the blocker has a 35° diagonal edge from (446, z 328) up to the ceiling at (525, z 384), a vertical edge at x = 446 below it, and the green light's horizontal top edge at z = 334 crosses the frame. All hard (`_light_size 0`). |
| `smg-wall-tip` | `shadowmask-groups-capture` | (575, 462, 45) | 139.3 / -26.6 / 70 | The foot of the north wall near x = 525, where the red shadow's tip reaches the floor: its two wall diagonals converge on (525, 512, 0), and the floor shadow edge x = 525 runs to the wall. The green shadow's edge at x = 528 sits 3 in beside it, an inside-corner case with two hard edges 2 texels apart. |
| `spec-wall-edge` | `specular-shadowmask-capture` | (244.6, 441.3, 190) | 63.4 / 0 / 80 | North wall at the left edge (x = 280) of the blocker's hard shadow. The camera is placed on the mirror line of the static light (416, 240, 192), so the specular highlight is centred on the edge and the edge cuts through it. All terms on. |
| `spec-wall-edge-specular-only` | `specular-shadowmask-capture` | same | same, plus `light_term_mask` 64 | The same view with only the specular term (`LightTermMask::SPECULAR`, bit 6), so the highlight's ramp across the edge and any banding stand alone. |

Review (from the brief):

- `sst-*` and `smg-*`, `floor` vs `off`: no texel staircase on any baked edge with the floor on. The owner judges hard-light softness of `floor` against `d001`.
- `spec-*`, `floor` vs `base`: the highlight ramps across the new edge with no step and no visible rise in edge banding. `off` should match `base` and separates the floor's effect from any other change since the base commit.

## Re-run

From the workspace root, in Git Bash, with nothing else building (one bake or
capture at a time; the preflight build shares `target/`):

```bash
cargo build -p postretro-level-compiler --bin prl-build   # floored prl-build in target/debug
C:/Users/danhi/Projects/Personal/postretro/.claude/worktrees/lpf-bin/capture/run.sh          # bake, then capture all
C:/Users/danhi/Projects/Personal/postretro/.claude/worktrees/lpf-bin/capture/run.sh bake
C:/Users/danhi/Projects/Personal/postretro/.claude/worktrees/lpf-bin/capture/run.sh capture sst-fan-se
FORCE=1 .../run.sh bake                                    # re-bake PRLs that exist
```

A single capture by hand:

```bash
cargo run -p xtask -- capture measurements/lightmap-penumbra-floor/scenes/sst-fan-se.floor.scene.json
```

`run.sh` bakes `soft_shadow_test` and `shadowmask-groups-capture` three ways
each (`floor`, `off`, `d001`) and `specular-shadowmask-capture` three ways
(`floor`, `off`, `base`) with `--no-cache --no-tui --baked-root`, then captures
all 18 scenes into `lpf-bin/capture/png/`. The `d001` bakes are 16× the texels
of the default and may run into the compile working-set budget
(`--working-set-max-size`); pass it through `prl-build` by hand if a bake stops
by name.

To point the scenes at another root, replace `../lpf-bin/capture` in the
`map` and `output` fields (`sed -i 's#\.\./lpf-bin/capture#/new/root#'`), and
keep the `content/dev/maps` and `baked/materials` layout under it.
