# emissive-volumes

Brief · resumable · reads: `context/lib/resource_management.md` §4.5 · `context/lib/rendering_pipeline.md` §7.3, §7.8 · `context/lib/scripting.md` §10.2, §12 · `context/lib/networking.md` §Presentation state · `context/lib/build_pipeline.md` §Compiler pipeline step 1, §PRL section IDs · read at eb44499

## Problem

Owner request: mappers set how strongly a surface glows, per placement, and scripts animate it. Today glow strength comes only from the texture-name prefix: `neon_` gives 4.0, every other prefix 0 (`Material::emissive_strength`). So only `neon_` textures glow at all, every face sharing a texture glows alike, and nothing at runtime can change it. A prefix named for one game's look sets glow for every game on the engine. When done:
- An `_e` map glows at face value on any texture, with no prefix.
- A brush volume around a surface sets the range of its glow strength: 0 up to an authored HDR maximum.
- Scripts pulse, fade, flicker or set that glow, and co-op peers see host-driven changes.

## Decisions

- **Retire the `neon` prefix and `Material::Neon`.** Neon carries nothing else: shininess and surface depth match Default, and ricochet has no reader.
  - Glow strength becomes 1.0 wherever a material's `_e` slot is loaded, 0 otherwise. Runtime and bake read the one predicate, so the forward pass still skips the emissive sample when no `_e` exists.
  - `tools/gen_emissive.py` takes its brightness cutoff as a flag, not from the prefix.
  - Diverges from `resource_management.md` §4.5 "prefix-derived strength"; that section is rewritten at promotion.
- **`_e` stays an RGB colour map in linear light; black is zero glow.** Strength multiplies the texel: `glow = _e × strength`. The diffuse map is the unlit surface and `_e` is the lit one, so fading glow to 0 shows a real off state. This matches glTF `emissiveTexture` × `KHR_materials_emissive_strength` (`research.md`).
- **`emissive_volume` brush entity.** Keys:
  - `emissive_min` (default 0) and `emissive_max` (default 1). Both are finite and ≥ 0; a max below min swaps, with a warning.
  - A scripted **level** in [0, 1] sets strength = min + level × (max − min). With no script the level is 1, so the volume sits at max.
  - `_tags`.
  
  Its brushes never render, collide or bake (peeled at parse like `fog_volume`).
- **The map owns the range; scripts own only the level.** This diverges on purpose from lights and fog, whose `pulse`/`fade`/`flicker` take absolute values. A script cannot push a surface past the mapper's maximum, so look stays with the map (`index.md` §2, "Defaults for novices"). The same methods therefore mean level units on an emissive member, and the docs say so beside the member table.
- **Membership is compile-time and per static world face.**
  - A face belongs to a volume when its centroid lies inside one of the volume's brush hulls.
  - Overlapping volumes resolve last-in-map-order wins, as `lightmap_scale_region` does, and the compiler warns once per overlapping pair. Rival: smallest containing volume wins (`reflection_volume`, environment volumes), so nesting needs no warning. Last-wins was kept because the two selectors share a rule: both override a surface property, not a region's physics.
  - A volume containing no face with an `_e` map warns. A face with no `_e` inside a volume still glows nothing.
  - At most 256 volumes per level; past that the compile fails, naming the count.
- **Static world faces only.**
  - Kinematic movers and fluids keep strength 1.0 with no lever. Movers leave any static volume, and the per-instance mover record is the door when content asks.
  - Accepted regression: a `neon_` face on a mover or fluid drops from 4.0 to 1.0 and cannot bloom from glow alone. No dev map has one.
  - Lava's `_e` therefore starts glowing at 1.0; that is content, not engine policy.
  - Imported models stay on `Material::Default` (unchanged).
- **Runtime owns the level; the bake reads max.**
  - Per frame, the renderer packs each volume's current strength into one table that world faces index.
  - Reflection-probe capture bakes each member face at its volume's max, the no-script level. Captures shade a reference state: an animated light enters at the cycle mean of its authored KVP curve (`resource_management.md` §5), and script curves are never baked (`scripting.md` §2). A volume has no authored curve, so its reference state is its no-script level. A scripted pulse does not move a reflection.
  - Bounce means exclude glow texels on any texture with an `_e` map.
- **Photosensitivity.** The level is a CPU-packed source, so it passes through the `flash_clamp` per-value clamp (`ValueClamp`), one clamp per volume, while `accessibility.flashLimiter` is on. This is the first emissive source the `E23--photosensitivity-source-floor` draft names; that draft keeps the shared budget across sources.
- **Co-op: presentation state, not gameplay.**
  - A host-only level write replicates bound by emissive record index with the 1 cm origin check, exactly as fog volumes do (`networking.md` §Presentation state).
  - A machine-local write stays local.
- **Migration.** `tools/migrate_neon_prefix.py` renames `neon_*` textures and every sibling slot, and rewrites `.map` texture references.
  - Dry-run by default; refuses on a name collision.
  - It lists the maps it touched, because only a mapper can decide which faces need a volume to keep their HDR glow.
  - It names separately any former `neon_` texture on a mover or fluid face, which no volume reaches.
  - It ships with `tools/` in the SDK.
- **Dev content keeps its look.** Every dev map face that used a `neon_` texture sits in an `emissive_volume` with max 4.0. The combat-demo bloom A/B (bright blooms, dim stays under threshold) holds.
- **Retire `drafts/per-surface-bloom-opt-in`.** The owner considers per-surface bloom solved by per-face `_e` assignment. Its prefix-based approach is gone with the prefix.
- **Non-goals.**
  - Glow colour scripting (a tint channel): add only when content asks.
  - Area lights from glowing faces: a separate brief, not yet drafted, which should read this volume's level for its brightness.
  - Glow floors above black: owner ruling, 0 is the only floor.

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
// The animation primitive is setEmissiveAnimation { periodMs, playCount?, level: number[] | null }:
// null clears it, and a finite playCount settles on its last sample, as setFogAnimation does.
```

TypeScript and Luau twins both ship, wire byte-identical as the fog twins are.

## Acceptance

### Automated
- [ ] A texture with an `_e` map and no prefix glows at strength 1.0. One without `_e` packs 0 and skips the sample. A former `neon_` name is an unknown prefix, warned once.
- [ ] Bake and runtime agree on the `_e`-loaded predicate for one texture: bounce-mean glow exclusion, reflection-capture term and material uniform.
- [ ] Strength = min + level × (max − min) at level 0, 0.5 and 1. No script means max. A level outside [0, 1] clamps on both sides. Max < min swaps and warns.
- [ ] Membership:
  - A face whose centroid is inside a hull joins; one just outside does not.
  - Two overlapping volumes: the later wins and one warning names the pair.
  - A volume with no `_e` face warns.
  - Volume 256 compiles; volume 257 fails, naming the count.
- [ ] PRL round-trip of the emissive-volumes section. A level with no volumes emits no section, and its other sections are byte-identical to the same map compiled without the section.
- [ ] The Scripting surface example runs as a `content/dev` script. In the resulting level ramps:
  - `flicker` moves the level within [0.3, 1].
  - `setEmissiveLevel` to 0 holds 0.
  - A finite `fade` settles on its last sample.
  - `setEmissiveAnimation` with `null` clears the curve.
  
  TS and Luau emit identical wire.
- [ ] With the flash limiter on, a level step from 0 to 1 is rate-limited by the shared clamp rules. With it off, the step lands in one frame.
- [ ] Co-op:
  - A host-only level write reaches a client. A late joiner receives the current level.
  - A client-local write stays local.
  - A client whose volume index or origin mismatches drops the update and warns once.
- [ ] The migration script:
  - Dry run lists renames and touched maps, writing nothing.
  - A real run renames every slot sibling and rewrites references.
  - A collision refuses with no partial write.

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
- Ownership rival: glow keys on the brush entity that owns the faces (`func_group` already carries `_reflection_capture` for its own faces; `switch` compiles as world). Membership would be exact, with no centroid, overlap or split-face rule. Not chosen:
  - TrenchBroom groups and layers drop custom keys on save.
  - Ownership forces moving existing worldspawn brushes into an entity.
  - A volume retrofits glow over worldspawn and groups untouched, as `lightmap_scale_region` and `reflection_volume` already select faces.
- Volume parse and peel: `fog_volume` and `lightmap_scale_region` in the Quake adapter (`build_pipeline.md` §Compiler pipeline step 1). The hull test reuses the peeled source-hull planes `lightmap_scale_region` keeps.
- Scripting and replication: copy `sdk/lib/entities/fog_volumes.{ts,luau}`, the `setFog*` reaction primitives and the fog presentation binding. Reuse `AnimatableScalar<"level">`. Fog's 17-sample pulse with a wrap sample fits a CPU linear sampler; this level is sampled on the CPU too.
- Flash clamp: the keyed per-value container in `render-cpu` `flash_clamp`, as the ghost look clamp uses it.
- Docs: FGD entry; `docs/level_design.md` material table and an Emissive volumes section; `docs/scripting-reference.md` member table and primitives.

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
