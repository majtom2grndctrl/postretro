# character-clip-library — research

Read at a82073138. Figures computed from the committed glTFs with throwaway FK scripts, not from the engine. Nothing here was observed in a running build.

## Rig comparison: `exo_red` vs `limitator`

| Property | `exo_red` | `limitator` |
|---|---|---|
| Skin joints | 66 | 78 |
| Extra joints | `Throat` | 13 `*_end` leaf joints |
| Facing | −Z (left arm −X) | +Z (left arm +X) |
| Hips rest height | 0.964 m | 1.004 m |
| Clips | 50 | 53 |

Joint order diverges at index 6, so positional clip sharing would drive wrong bones without any error. Rest rotations differ per joint: median 4°, feet about 29°, thumbs up to 59°, Hips 180°. Hips rest rotation correction is exactly a 180° yaw.

## Same clips, different storage

49 clip names are shared. `limitator`-only: `firing_rifle`, `hit_reaction`, `reloading`, plus each file's own rest clip (`exo_red`, `limitator`).

| Check, over all 49 shared clips | Result |
|---|---|
| Max local rotation error of `rest_lim * rest_exo⁻¹ * q_exo` vs `q_lim` | 0.225° |
| Tracks with mismatched key counts | 0 |
| Root translation: `rest_lim + C * (t - rest_exo) * 0.68` residual | 0.000 cm |
| Non-root translation tracks leaving rest | 18, max 0.12 mm (float noise on finger tips) |
| Scale tracks away from 1 | 0 |
| `exo_red` clips whose keys start at frame 1 | 49 of 49 |

The result follows from the pipeline: `mixamo_to_gltf.py` stores a pose delta per bone, and the exporter composes it with each character's rest.

## Clip-sharing methods compared

Error against `limitator`'s own bake, relative to hips, mean/max cm. Four clips, 24 samples each.

| Method | Hands | Feet | Bone direction |
|---|---|---|---|
| Name match only, plus facing | 4–9 / 5–11 | 5–6 / 5–6 | 6.3–6.9° |
| Model-space rest delta | 2–9 / 3–10 | 1–4 / 2–6 | 4.8–5.4° |
| Calibrated per-joint offset | 1–7 / 1–13 | 0–4 / 0–6 | 1.8–4.4° |
| **Local rest correction (chosen)** | 0 / 0 | 0 / 0 | 0.0° |

## Root scale

`limitator`'s Hips delta is exactly 0.68× `exo_red`'s in every clip: the conversion `--scale`, not the 1.042 hip-height ratio. Effect at the end of each clip:

| Clip | Lowest joint today | Lowest joint at `k = 1.042` |
|---|---|---|
| `death_from_front_headshot` (used by the descriptor) | 0.296 m | −0.006 m |
| `death_from_the_front` | 0.209 m | −0.066 m |
| `death_from_right` | 0.256 m | −0.030 m |
| `idle_crouching` | 0.182 m | −0.008 m |

`exo_red`'s own death poses end at about 0 m, so its motion is the unscaled reference. `limitator`'s `walk` and `run` author `travelSpeed`, so their speed does not change. Clips relying on load-derived speed do.

## Size

| Model | Disk | Estimated mesh-only | Tris |
|---|---|---|---|
| `exo_red` | 5.70 MB | 1.36 MB (24%) | 31,696 |
| `limitator` | 5.61 MB | 0.23 MB (4%) | 4,790 |

Of `limitator`'s 12,402 tracks, 3,356 vary, carrying 1.14 MB of keyframes. Dropping non-root translation and scale removes most of the rest, and most of the JSON. `limitator`'s descriptor uses 8 of its 53 clips.

Load time is not the motive. `level-load-perf/findings.md` on `campaign-test`: glTF JSON parse 27 ms, mesh and clips 16 ms, model textures about 250 ms.

## Seam map

Clip data assumes one model in these places. Bind-at-load leaves all of them as they are, because they consume `LoadedModel.clips`.

| Consumer | Assumption |
|---|---|
| `AnimationClip.joints` | Positional, topological joint index of that model |
| `anim/mod.rs`, `anim/blend.rs` | `clip.joints.get(i)`; wrong order fails silently |
| `derive_and_neutralize_root_motion` | Per model, needs `locomotion_root` |
| `compute_conservative_animation_bounds` | Mesh × clip set |
| `ModelHitZones`, `derive_bound` | Own clone of skeleton and clips |
| `MeshClipTables`, `ModelClipTable` | Keyed by model handle |
| `ClipSample.clip_index`, `MeshSampleParams::stateless` | Index into the model's list; an animated state with an unresolved clip falls back to clip 0 in the renderer. A stateless entity holds rest |
| `WireMeshAnimationState` | State name only; no clip data on the wire |
| `predict_level_models` | Returns handles only |
| `mount.rs`, `socket_dump` | Reach clips through `load_model` |

Places that do change: `read_model_sources`, `ModelSources`, `load_model_from_sources`, `model_content_key`, `MODEL_CONTENT_KEY_VERSION`, `tools/mixamo_to_gltf.py`.

## Prior commitments

No prior decision covers clip separation or shared skeletons. Embedded clips are the shipped layout, not a debated one. No model asset budget exists.

One commitment touches this work: `plans/done/weapon-mount-frame-solver` rules engine-loader bone conversion out of scope as non-negotiable, citing `resource_management.md` §7. Its target was axis and frame conversion of a model's own data. Binding leaves a model's joint and rest data raw and composes only library tracks, but it is still bone math in the loader, so the brief names the divergence. The loader already rewrites a model's own root tracks during root-motion neutralization.

Binding policy lives in the engine: the rest-relative form and `k` are not neutral glTF semantics. Only the name vocabulary stays out. `model_content_key`'s own doc already states the rule this brief extends: references resolve relative to the document, and the key covers what they resolve to.

## Source FBXs

On the owner's machine, in `Downloads`: `Pro Rifle Pack (fbx-bin)` (50 FBXs including `exo_red.fbx`), `Basic Shooter Pack` (17 FBXs including `exo_red.fbx`), `Limitator.fbx`. A library can be rebuilt from these.

## New character rigs

Rigged by Mixamo's auto-rigger, exported as FBX Binary in T-pose. The FBXs sit in `sources/`, outside the mod tree so bundles leave them out. Each is converted to `model.gltf` in its model folder. Neither has played a clip in the engine.

| Property | `rodin_sci-fi_trooper_m` | `cyberpunk_warrior_f` |
|---|---|---|
| Rigged FBX | `sources/trooper_tpose.fbx` | `sources/warrior_tpose.fbx` |
| Joints | 25, no fingers | 25, no fingers |
| Tris | 3,431 | 3,961 |
| Height | 2.00 m | 1.90 m |
| Hips rest height | 1.116 m | 1.045 m |
| Expected `k` against `exo_red` | 1.158 | 1.084 |
| Mesh source | Committed mesh, weights from the source rig for the T-pose | 490k-tri Sketchfab source, posed then decimated and re-baked |
| Removed | Held gun | Sheathed sword |
| Known flaws | Mitt hands | Dark marks on the right leg, curled right hand, small holes at the sword cuts |

Both carry only `mixamorig:` joints that `exo_red` also has, so finger tracks drop as library joints with no mesh joint. Both models are CC-BY; their `license.txt` credits travel with them.

AI turns a model's +Z to its target. Both rigs convert facing −Z at the default yaw, as `exo_red` does, so the staged bakes use `--yaw 180`. Toe joints confirm +Z in the glTF. A re-conversion keeps that flag.

Each `scene.gltf` has no skin, so a static model ignores a library. The first slice binds the staged `model.gltf`.

## Rejected shapes

Rest correction in the sampler shares clip memory but touches the sampler, snapshots and hit zones for a saving nothing is bound by.

## Pin table

| ID | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | A library re-bake changes only keyframe bytes in its buffer | Key derives after the library document and every buffer it references are read | Key changes; the model misses |
| P2 | A library is edited or deleted after a model's sources are read and before its parse | Read, key, edit, parse | The parse uses the bytes that were keyed. Any shared per-load library parse obeys the same rule |
| P3 | A static model names an unreadable library | Skin check precedes the library read | Warns and loads; the ignored library enters neither the read nor the key |
| P4 | A library clip raises the root, with height ratio above 1 | Bind, then derivation, then bounds and reach | Bounds and reach cover the scaled height |
| P5 | A library clip's longest track sits on a joint the mesh lacks or on a dropped channel | Duration comes from the library clip before tracks drop | Same duration on every mesh that binds it |
| P6 | A model carries embedded and library clips | Bind touches library clips only; each clip derives once | Embedded tracks and travel speed equal a load with no library |
| P7 | A numbered-prefix rig goes through the tool, tagging on or off | Normalization precedes tagging; stamping does not depend on tagging | Same masks, sockets, zones and skin root as a canonical rig; `clipLibrary` is present with tagging off |
| P8 | A library is edited between two levels that keep a model | Old set resident while the next load keys | The model misses; the old set draws the old entry until release |
| P9 | Parity sampling against pre-migration bakes | `exo_red`'s baseline runs one frame later than its bound clip; `limitator`'s runs at the same time | Rotations agree at the aligned time |
| P10 | A new character's rifle is mounted | Library bound, facing confirmed and mesh final before the solve; a re-bake re-solves | The check passes on the shipped character and rifle |

## Adjacent drafts

- `E10--enemy-stagger`: needs a pain clip. `hit_reaction` in a shared library reaches every consumer.
- `player-descriptor-composition`: `mesh.model` plus per-state `clip` shape is preserved.
- `viewmodel-motion-layer`: viewmodel clips excluded; no interaction.
- `model-import-hit-zone-hardening`: hardens clip and channel validation. A library parses through the same channel validation, so its rules apply to both.
