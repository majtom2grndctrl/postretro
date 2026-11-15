# emissive-volumes

Brief · resumable · reads: `context/lib/resource_management.md` §4.5 · `context/lib/rendering_pipeline.md` §7.3, §7.8 · `context/lib/scripting.md` §10.2, §12 · `context/lib/networking.md` §Presentation state · `context/lib/build_pipeline.md` §Compiler pipeline step 1, §PRL section IDs · read at eb44499

## Problem

Owner request: mappers set how strongly a surface glows, per placement, and scripts animate it. Today glow strength comes only from the texture-name prefix: `neon_` gives 4.0, every other prefix 0 (`Material::emissive_strength`). So only `neon_` textures glow at all, every face sharing a texture glows alike, and nothing at runtime can change it. A prefix named for one game's look sets glow for every game on the engine. When done:
- An `_e` map glows at face value on any texture, with no prefix.
- A brush volume around a surface sets the range of its glow strength: 0 up to an authored HDR maximum.
- Scripts pulse, fade, flicker or set that glow, and co-op peers see host-driven changes.

## Decisions

- **Retire the `neon` prefix and `Material::Neon`.** Neon's shininess matches Default and its ricochet flag has no reader. Its flat surface depth goes too: a former `neon_` texture with an `_h` map carves as Default does.
  - Glow strength becomes 1.0 wherever a material's `_e` slot is loaded, 0 otherwise. Runtime and bake read the one predicate.
  - `tools/gen_emissive.py` uses one default cutoff whatever the prefix; `--cutoff` stays the lever.
  - Diverges from `resource_management.md` §4.5 "prefix-derived strength"; that section is rewritten at promotion.
- **`_e` stays an RGB colour map in linear light; black is zero glow.** `glow = _e × strength`. The diffuse map is the unlit surface and `_e` the lit one, so fading glow to 0 shows a real off state. Matches glTF `emissiveTexture` × `KHR_materials_emissive_strength` (`research.md`).
- **`emissive_volume` brush entity.** Keys:
  - `emissive_min` (default 0), `emissive_max` (default 1), `_tags`.
  - A negative or non-finite value fails the compile, naming the key. A max below min swaps, with a warning.
  - A scripted **level** in [0, 1] sets strength = min + level × (max − min). With no script the level is 1.
  - Its brushes never render, collide or bake.
- **The map owns the range; scripts own only the level.** Lights and fog take absolute values through the same `pulse`/`fade`/`flicker`; here those methods take level units, so a script cannot push a surface past the mapper's maximum (`index.md` §2, "Defaults for novices").
- **Level writes and animations.** A level write clears any running animation on its targets, as `setSpinRate` does, not as fog's static setters do. Clearing an animation leaves the level at its last presented sample.
- **Membership is compile-time and per static world face.**
  - A face belongs to a volume when its centroid lies inside one of the volume's brush hulls.
  - Overlaps resolve last-in-map-order wins, as `lightmap_scale_region` does.
  - Warnings:
    - once per pair of volumes that both contain a face centroid (volumes that only touch, or overlap in empty space, stay silent)
    - per volume left with no `_e` face, including one emptied by a later volume
  - At most 256 volumes per level; past that the compile fails, naming the count.
- **Static world faces only.**
  - Kinematic movers and fluids keep strength 1.0 with no lever. Accepted regression: a `neon_` face on a mover or fluid drops from 4.0 to 1.0. No dev map has one.
  - Model glow stays 0: models load only their diffuse slot.
- **Runtime owns the level; the bake reads max.** Reflection-probe capture bakes each member face at its volume's max, its reference state (`research.md` §Capture reference state). A scripted pulse does not move a reflection. Bounce means exclude glow texels on any texture with an `_e` map.
- **Photosensitivity.** While `accessibility.flashLimiter` is on, each volume's presented strength passes the `flash_clamp` per-value rules, judged relative to the bloom threshold so HDR range counts. The red-flash rule and any shared budget across sources stay with `E23--photosensitivity-source-floor`, which lists emissive animation as a clamp site.
- **Co-op: presentation state, not gameplay.** A host-only level write replicates bound by emissive record index with the 1 cm origin check, exactly as fog volumes do (`networking.md` §Presentation state). A machine-local write stays local.
- **Migration.** `tools/migrate_neon_prefix.py` renames `neon_x` to `x` for every slot sibling and rewrites `.map` texture references. It ships with `tools/` in the SDK.
  - Dry-run by default. Refuses on a name collision, or when `x` starts with a known material prefix (the rename would change the material).
  - Lists the maps it touched: only a mapper can decide which faces need a volume to keep HDR glow.
  - Names separately any former `neon_` texture on a mover or fluid face, which no volume reaches.
- **Dev content keeps its look.** Every dev map face that used a `neon_` texture sits in an `emissive_volume` with max 4.0, so the combat-demo bloom A/B holds.
- **Non-goals.**
  - Glow colour scripting (a tint channel): add only when content asks.
  - Area lights from glowing faces: a separate brief, not yet drafted, which should read this volume's level for its brightness.

### Scripting surface

```ts
// Proposed design
const signs = getMapEntities("emissive", { tag: "bar_sign" });
for (const sign of signs) {
  // sign.component: { min: number; max: number; level: number }
  reactions.push(defineReaction("levelLoad", { sequence: sign.flicker({ min: 0.3, max: 1, rate: 6 }) }));
}
reactions.push(defineReaction("powerCut", { primitive: "setEmissiveLevel", tag: "bar_sign", args: { level: 0 } }));
// Members also carry pulse({ min, max, periodMs }) and fade({ from, to, periodMs }).
// Every value is a level in [0, 1]; out-of-range samples clamp.
// The animation primitive is setEmissiveAnimation, args { periodMs, playCount?, level: number[] }
// with a non-empty curve, or args null to clear it, as setFogAnimation does.
// A finite playCount settles on its last sample. setEmissiveLevel clears any running animation.
```

TypeScript and Luau twins both ship, wire byte-identical as the fog twins are.

## Acceptance

### Automated
- [ ] A texture with an `_e` map and no prefix glows at strength 1.0. One without `_e` packs 0 and skips the sample. A former `neon_` name is an unknown prefix, warned once (grep gate: no `neon` in the prefix tables). A former `neon_` texture with an `_h` map carves as Default.
- [ ] Bake and runtime agree on the `_e`-loaded predicate for one texture: bounce-mean glow exclusion, reflection-capture term and material uniform. That texture's bounce mean excludes its glow texels.
- [ ] A texture with a known prefix (`metal_`) and an `_e` map glows at 1.0 too: the predicate ignores the material.
- [ ] A skinned model with an `_e` sibling still packs strength 0.
- [ ] `gen_emissive.py` without `--cutoff` uses one default whatever the file's prefix: a `neon_` file and any other give the same `_e`.
- [ ] Strength = min + level × (max − min) at level 0, 0.5 and 1. No script means max. A level outside [0, 1] clamps on both sides. Max < min swaps and warns. A negative or non-finite `emissive_min` or `emissive_max` fails the compile, naming the key; 0 compiles.
- [ ] Membership:
  - A face whose centroid is inside a hull joins; one just outside does not.
  - A face lying in the plane of a volume brush's side (the volume drawn flush on a wall) joins (pin P27).
  - A member face the atlas cuts into sub-faces glows as one: every sub-face carries its membership, and the section indexes the shipped Geometry faces (pin P29).
  - A volume enclosing a mover brush and a fluid lists neither in its faces. Both glow at 1.0 with an `_e` map, at any level.
  - Two volumes that both contain a face centroid: the later wins and one warning names the pair. Volumes that only touch, or overlap where no centroid lies, stay silent.
  - A volume with no `_e` face warns, including one whose every face a later volume takes (pin P28).
  - Volume 256 compiles; volume 257 fails, naming the count.
- [ ] Adding a default volume (min 0, max 1) around `_e` faces changes only the emissive-volumes section: every other section is byte-identical to the same map without the volume.
- [ ] The loader refuses a version other than 1, a count over 256, a face index out of range or descending, and a face owned twice.
- [ ] At runtime a member face presents its volume's strength and a non-member face sharing a vertex with it presents 1.0, at level 0 and at level 1. In a 256-volume level, a face in the last volume presents that volume's strength and a non-member presents 1.0 (pin P36).
- [ ] A level written by a reaction drained in frame N presents in frame N (pin P12).
- [ ] Reflection capture shades a member face at its volume's max whatever its scripts do. Changing a volume's `emissive_max`, or a face's membership, misses the reflection-capture cache.
- [ ] PRL round-trip of the emissive-volumes section. A level with no volumes emits no section.
- [ ] The Scripting surface example runs as a `content/dev` script. In the resulting level ramps:
  - `flicker` moves the level within [0.3, 1].
  - `setEmissiveLevel` to 0, fired by a dev-map trigger's `on_fire "powerCut"`, holds 0.
  - A finite `fade` settles on its last sample.
  - `setEmissiveAnimation` with args `null` clears the curve, and the level holds its last presented sample (pin P5).
  - A level write clears a running animation and holds (pin P1). In one body, animation then level: the level wins; level then animation: the animation wins (pins P2, P3). A level written mid-`fade` holds, and the fade's settle never lands (pin P4).
  - A non-finite level reads 0, and every animation sample clamps as a set level does. `periodMs` 0, `flicker` rate 0 and an empty curve are refused with a warning, leaving the prior animation; `playCount` 0 plays once; a `fade` whose `from` equals `to` settles on that value and counts no flash (pins P7–P10).
  - In one body, a level write then the same running animation again restarts it from its first sample, on host and client (pin P30).
  - A tag matching no volume is a no-op; a non-emissive target warns and is skipped (pin P11). A paused script clock holds the level (pin P13).
  
  TS and Luau emit identical wire.
- [ ] With the flash limiter on, a level step from 0 to 1 is rate-limited by the shared clamp rules. With it off, the step lands in one frame.
  - With it on, the Scripting surface `flicker` (rate 6) presents at most 3 flashes in any 1 s; scripts and the wire still see the unclamped level (pin P14).
  - The clamp judges strength, not level: a level step of 0.05 on a volume with max 20 (a strength step of 1.0) is limited as a strength step of 1.0 is.
  - A reaction writing level 0 and 1 alternately six times a second, with no animation, also presents at most 3 flashes in any 1 s (pin P34).
  - Clamp history: a volume is limited every frame, in view or not. A frame whose surface acquire fails leaves every clamp where it began. Loading another map starts every volume fresh; a level write does not reset the clamp. Turning the limiter on presents that frame unchanged (pins P15–P18).
- [ ] Co-op:
  - A host-only level write reaches a client. A late joiner receives the current level.
  - A client-local write stays local. A client-local write in the frame a host record arrives holds until the host's record changes (pin P35).
  - A host level write while a client runs a flicker reaches the client as that level with no animation: the client's curve stops and the level holds (pin P31).
  - A joiner arriving after the host's `powerCut` holds 0, although its own `levelLoad` installs the example's flicker (pin P32).
  - A client whose volume index or origin mismatches drops the update and warns once. A fog record never binds an emissive volume at the same index (pin P23).
  - A joiner arriving mid-`fade` sees it settle when the host's does; one arriving after it ended presents the last sample from its first frame (pins P21, P22).
  - A `levelLoad` wait-tail write lands on the host and replicates (pin P20). Demotion keeps the client's last level; re-promotion brings the host's (pin P24).
  - Host overrides on every volume of a 256-volume level reach a late joiner (pin P25). An animation past the wire's key cap warns once on the host, and the client keeps its last valid override (pin P26).
- [ ] After a script set level 0, `restartLevel` brings every volume back to max, with no animation, on host and client, before `levelLoad` fires (pin P19).
- [ ] The migration script:
  - Dry run lists renames and touched maps, writing nothing.
  - A real run renames every slot sibling and rewrites references.
  - A collision refuses with no partial write. So does `neon_metal_x`, whose result would start with a known prefix; `neon_panel` renames.
  - A former `neon_` texture on a mover or fluid face is named separately.
- [ ] combat-demo and reflection-probes-test compile with every former `neon_` face in a volume of max 4.0. At that max the bright panel's peak passes the bloom threshold and the dim panel's stays under it, restating the bloom pin. This replaces `neon_emissive_strength_clears_the_bloom_threshold`, which reads `Material::Neon`.
- [ ] Only `setEmissiveLevel` and `setEmissiveAnimation` write an emissive volume, and neither carries min or max; the replicated record carries level and animation only (grep gate: the emissive primitive list and payload fields).

### Manual
- [ ] combat-demo: the bright panel blooms and the dim one does not, as before. A script pulsing the bright panel's volume visibly breathes; at level 0 the panel shows its unlit diffuse.
- [ ] reflection-probes-test matches its pre-change look with its volumes in place, reflections of the panels included.
- [ ] Lava glows at face value; owner judges whether content wants a different `_e`.
- [ ] Two-player co-op: a host trigger dims a sign on both screens; a joiner arriving after sees it dimmed.

## Path

- Prefix removal touches every consumer in `research.md` §Neon consumers. Order: retirement and the 1.0 default, then the volume. Each slice leaves the tree green. The dev maps take their volumes in the same commit that drops the 4.0, so no commit regresses the bloom A/B.
- Per-face index to the shader:
  - Chosen: the compiler writes per-volume face-index lists into the new section. At load the renderer builds a per-vertex volume id stream beside the world vertex buffer, as `spec_owner_buffer` rides slot 1, and the fragment indexes the strength table.
  - Rival: a per-vertex field in Geometry (id 17), like `animated_block`. It needs unshared vertices (`animated_block_ids.rs` guard) and changes the Geometry ABI.
  - First slice: one volume around one face, one vertex shared with a non-member face, checking whether the shared vertex breaks the per-vertex id.
- Ownership rival: glow keys on the brush entity that owns the faces. Not chosen; argued in `research.md` §Ownership rival.
- Strength reaches the forward pass through a per-frame table packed CPU-side, one entry per volume; the forward pass keeps skipping the emissive sample when no `_e` exists. Movers' per-instance record is the door for mover glow.
- Volume parse and peel follow `fog_volume`. Brush entities `continue` before the world set in `parse.rs`. `fog_volume` (non-axis-aligned) and `lightmap_scale_region` each fail the compile on more than one brush, and lightmap selection never reads the region's kept planes (`lightmap_bake/charts.rs` tests the chart's first vertex against the AABB). Several hulls per entity follow the environment volumes, which keep one convex leaf per brush.
- Scripting and replication: copy `sdk/lib/entities/fog_volumes.{ts,luau}`, the `setFog*` reaction primitives and the fog presentation binding. Reuse `AnimatableScalar<"level">`; the level is sampled on the CPU, as fog's is. Level writes clear the animation as `setSpinRate` does, not as `setFogDensity` does.
- Flash clamp: `ValueClamp` is public and per value. Its keyed container is private (`flash_clamp` `mod keyed`; `KeyClamp for ValueClamp` exists only in tests), and the ghost look clamp keys `LookValueClamp`. Exposing a keyed `ValueClamp` is the executor's call.
- Docs: FGD entry; `docs/level_design.md` material table and an Emissive volumes section; `docs/scripting-reference.md` member table and primitives, saying beside the member table that emissive methods take level units.

## Open questions

- A face split by the BSP or the atlas cut into fragments that straddle a hull: does each fragment take its own centroid, or does membership follow the source brush side? — **delegated**: the executor picks whichever keeps one authored face whole and reports it.
- Strength table as a uniform or a storage buffer, within the forward pass's per-stage binding budgets — **delegated**.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Entity class | parsed classname | n/a | n/a | n/a | `emissive_volume` |
| Range min | record field | f32 | `component.min` | `component.min` | `emissive_min` |
| Range max | record field | f32 | `component.max` | `component.max` | `emissive_max` |
| Level | runtime component field | f32 (replicated) | `component.level` | `component.level` | n/a |
| Member kind | `getMapEntities` kind | `"emissive"` | `"emissive"` | `"emissive"` | n/a |
| Set primitive | reaction primitive | `"setEmissiveLevel"` | `"setEmissiveLevel"` | `"setEmissiveLevel"` | n/a |
| Animation primitive | reaction primitive | `"setEmissiveAnimation"` | `"setEmissiveAnimation"` | `"setEmissiveAnimation"` | n/a |
| Tags | record tags | length-prefixed strings | `tags` | `tags` | `_tags` |

## Wire format

New PRL section `EmissiveVolumes`, id 60 (next free after `ProbeDepthTiles` 59). Little-endian; mirrors `FogVolumes` (id 30) record and tag encoding:
- **Header:** `u16 version` (= 1, any other rejected), `u32 volume_count` (≤ 256).
- **Per volume, in map order:**
  - `f32 min`, `f32 max` (stored after any swap)
  - `f32[3] origin`: the AABB centre, the co-op binding check
  - `u32 face_count`, then `face_count × u32` Geometry face indices, ascending, each in range and owned by exactly one volume
  - `u32 tag_count`, then each tag as `u32 length` + UTF-8 bytes
- A level with no volumes emits no section; the loader reads absence as no volumes.
- Co-op presentation state gains the level kind and bumps `WIRE_VERSION` and `SNAPSHOT_VERSION` per `networking.md` §Version gates.
