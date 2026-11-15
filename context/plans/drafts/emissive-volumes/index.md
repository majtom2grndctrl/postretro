# emissive-volumes

Brief · resumable · reads: `context/lib/resource_management.md` §3, §4.5, §5 · `context/lib/rendering_pipeline.md` §7.3, §7.8 · `context/lib/scripting.md` §10.2, §12 · `context/lib/networking.md` §Presentation state · `context/lib/build_pipeline.md` §Compiler pipeline step 1, §PRL section IDs · read at eb44499

## Problem

Owner request: mappers set how strongly a surface glows, per placement, and scripts animate it. Today glow strength comes only from the texture-name prefix: `neon_` gives 4.0, every other prefix 0 (`Material::emissive_strength`). So only `neon_` textures glow at all, every face sharing a texture glows alike, and nothing at runtime can change it. A prefix named for one game's look sets glow for every game on the engine. When done:
- An `_e` map glows at face value on any texture, with no prefix.
- A brush volume around a surface sets the range of its glow strength: 0 up to an authored HDR maximum.
- Scripts pulse, fade, flicker or set that glow, and co-op peers see host-driven changes.

## Decisions

- **Retire the `neon` prefix and `Material::Neon`.** Neon's shininess matches Default and its ricochet flag has no reader. Its flat surface depth goes too: a former `neon_` texture with an `_h` map carves as Default does.
  - Glow strength becomes 1.0 wherever a material's `_e` slot is loaded, 0 otherwise. Runtime and bake read the one predicate.
  - `tools/gen_emissive.py` uses one default cutoff whatever the prefix; `--cutoff` stays the lever.
  - Diverges from `context/lib` wherever it describes prefix-derived glow, neon flatness or unlimited emissives: `resource_management.md` §3, §4.5, §5.2 and `rendering_pipeline.md` §7.3, §7.8. Each is rewritten at promotion.
- **`_e` stays an RGB colour map in linear light; black is zero glow.** `glow = _e × strength`. The diffuse map is the unlit surface and `_e` the lit one, so fading glow to 0 shows a real off state. Matches glTF `emissiveTexture` × `KHR_materials_emissive_strength` (`research.md`).
- **`emissive_volume` brush entity.** Keys:
  - `emissive_min` (default 0), `emissive_max` (default 1), `_tags`.
  - A bad value never stops the compile (`index.md` §2, "Designers over structure"). A negative value clamps to 0 and a non-finite one takes its key's default, each with a warning naming the key. A max below min swaps, with a warning.
  - A scripted **brightness** in [0, 1] sets strength = min + brightness × (max − min). With no script, brightness is 1.
  - Its brushes never render, collide or bake.
- **The map owns the range; scripts own only brightness.** Lights and fog take absolute values through the same `pulse`/`fade`/`flicker`. Here those methods take brightness in [0, 1], so a script cannot push a surface past the mapper's maximum (`index.md` §2, "Defaults for novices").
- **Brightness writes and animations.** A brightness write clears any running animation on its targets, as `setSpinRate` does, not as fog's static setters do. Clearing an animation leaves brightness at its last sampled value: the script's value, before any limiter.
- **Membership is compile-time and per static world face.**
  - A face belongs to a volume when its centroid lies inside one of the volume's brush hulls.
  - Overlaps resolve last-in-map-order wins, as `lightmap_scale_region` does.
  - Warnings:
    - once per pair of volumes that both contain a face centroid (volumes that only touch, or overlap in empty space, stay silent)
    - per volume left with no `_e` face, including one emptied by a later volume
  - At most 256 volumes per map; past that the compile fails, naming the count.
- **Static world faces only.**
  - Kinematic movers and fluids keep strength 1.0 with no lever. Accepted regression: a `neon_` face on a mover or fluid drops from 4.0 to 1.0. No dev map has one.
  - Model glow stays 0: models load only their diffuse slot.
- **Runtime owns brightness; the bake reads max.** Reflection-probe capture bakes each member face at its volume's max, its reference state (`research.md` §Capture reference state). A scripted pulse does not move a reflection. Bounce means exclude glow texels on any texture with an `_e` map.
- **Photosensitivity: one HDR rule in `flash_clamp`.**
  - The per-value clamp takes values in [0, 1]. A new shared rule lets it judge an HDR value s ≥ 0: s enters as s ÷ (s + 1), with 1 the bloom threshold, and the clamped result maps back exactly. Every HDR step counts, compressed as the tonemap compresses it.
  - Emissive volumes are the rule's first source; `E23--photosensitivity-source-floor` reuses it for lights.
  - While `accessibility.flashLimiter` is on, each volume's presented strength passes the clamp. Scripts and the wire carry the scripted brightness.
  - The red-flash rule and any shared budget across sources stay with E23.
- **Co-op: fog volumes' presentation-state behaviour, with three deliberate differences.** A host-only write replicates bound by emissive record index with the 1 cm origin check (`networking.md` §Presentation state); a machine-local write stays local. The differences:
  - the brightness-write clearing rule above
  - brightness units in place of absolute values
  - a per-machine limiter that never reaches the wire
- **Migration.** `tools/migrate_neon_prefix.py` renames `neon_x` to `x` for every slot sibling and rewrites `.map` texture references.
  - Dry-run by default.
  - Refuses on a name collision, or when `x`'s derived prefix (its first `_` token, case-folded, as the engine parses it) is a known material prefix.
  - Lists the maps it touched: only a mapper can decide which faces need a volume to keep HDR glow.
  - Names separately any former `neon_` texture on a mover or fluid face, which no volume reaches.
- **Dev content keeps its look.** Every dev map face that used a `neon_` texture sits in an `emissive_volume` with max 4.0, so the combat-demo bloom A/B holds.
- **Non-goals.**
  - Glow colour scripting (a tint channel): add only when content asks.
  - Area lights from glowing faces: a separate brief, not yet drafted, which may read a volume's brightness.

### Scripting surface

```ts
// Proposed design
const signs = getMapEntities("emissive", { tag: "bar_sign" });
for (const sign of signs) {
  // sign.component: { min: number; max: number; brightness: number }
  reactions.push(defineReaction("levelLoad", { sequence: sign.flicker({ min: 0.3, max: 1, rate: 6 }) }));
}
reactions.push(defineReaction("powerCut", { primitive: "setEmissiveBrightness", tag: "bar_sign", args: { brightness: 0 } }));
// Members also carry pulse({ min, max, periodMs }) and fade({ from, to, periodMs }).
// Every value is a brightness in [0, 1]; out-of-range samples clamp.
// The animation primitive is setEmissiveAnimation, args { periodMs, playCount?, brightness: number[] }
// with a non-empty curve, or args null to clear it, as setFogAnimation does.
// A finite playCount settles on its last sample. setEmissiveBrightness clears any running animation.
```

TypeScript and Luau twins both ship, wire byte-identical as the fog twins are.

## Acceptance

### Automated
- [ ] Textures:
  - A texture with an `_e` map glows at strength 1.0, with no prefix and with a known prefix (`metal_`). The predicate ignores the material.
  - One without `_e` packs 0 and skips the emissive sample. This is a regression guard on forward-pass cost.
  - A former `neon_` name is an unknown prefix, warned once (grep gate: no `neon` in the prefix tables).
  - A former `neon_` texture with an `_h` map carves as Default.
  - A skinned model with an `_e` sibling still packs 0.
- [ ] Bake and runtime agree on the `_e`-loaded predicate for one texture: bounce-mean glow exclusion, reflection-capture term and material uniform. That texture's bounce mean excludes its glow texels.
- [ ] `gen_emissive.py` without `--cutoff` uses one default whatever the file's prefix: a `neon_` file and any other give the same `_e`.
- [ ] Strength = min + brightness × (max − min) at brightness 0, 0.5 and 1. No script means max. Brightness outside [0, 1] clamps on both sides.
- [ ] Bad keys compile, each with one warning naming the key: max < min swaps, a negative value reads 0, and a non-finite value reads its default. Valid keys, 0 included, compile silently.
- [ ] Membership:
  - A face whose centroid is inside a hull joins; one just outside does not. A face in the plane of a volume brush's side (the volume flush on a wall) joins (pin P27).
  - A member face the atlas cuts into sub-faces glows as one: every sub-face carries its membership, and the section indexes the shipped Geometry faces (pin P29).
  - A volume enclosing a mover brush and a fluid lists neither. Both glow at 1.0 with an `_e` map, at any brightness.
  - Overlap warnings:
    - Two volumes that both contain a face centroid: the later wins, and one warning names the pair.
    - Volumes that only touch, or overlap where no centroid lies, stay silent.
    - A volume with no `_e` face warns, including one a later volume empties (pin P28).
  - Volume 256 compiles; volume 257 fails, naming the count.
- [ ] Adding a default volume (min 0, max 1) around `_e` faces changes only the emissive-volumes section: every other section is byte-identical to the same map without the volume.
- [ ] PRL round-trip of the emissive-volumes section. A map with no volumes emits no section. The loader refuses:
  - a version other than 1
  - a count over 256
  - a face index out of range or descending
  - a face owned twice
- [ ] At runtime:
  - A member face presents its volume's strength, and a non-member face sharing a vertex with it presents 1.0, at brightness 0 and 1.
  - In a 256-volume map, a face in the last volume presents that volume's strength (pin P36).
  - Brightness written by a reaction drained in frame N presents in frame N (pin P12).
- [ ] Reflection capture shades a member face at its volume's max whatever its scripts do. Changing a volume's `emissive_max`, or a face's membership, misses the reflection-capture cache.
- [ ] The Scripting surface example runs as a `content/dev` script, with `powerCut` fired by a dev-map trigger's `on_fire`. TS and Luau emit identical wire.
  - `flicker` moves brightness within [0.3, 1].
  - `powerCut` holds 0.
  - A finite `fade` settles on its last sample.
- [ ] Fog parity: the fog volume animation and presentation-state tests pass when run against the emissive kind (pins P6–P11, P13, P19–P26, P35). That covers:
  - install validation, settle and re-install
  - pause and tag dispatch
  - host-only replication and machine-local writes
  - late joins during and after an animation
  - wait tails, demotion and re-promotion, restart
  - the wire's key cap, mismatched bindings, and a fog record never binding an emissive volume
- [ ] Deliberate differences from fog:
  - A brightness write clears a running animation and holds, in any order within one body (pins P1–P4). A later identical animation restarts from its first sample (pin P30).
  - The same holds across co-op. A host write reaches a client mid-flicker as that brightness with no animation (pin P31). A joiner after `powerCut` holds 0, although its own `levelLoad` installs the flicker (pin P32).
  - Clearing with args `null` leaves the last sampled brightness, before the limiter, and the wire carries it (pins P5, P33).
  - Only `setEmissiveBrightness` and `setEmissiveAnimation` write a volume, and neither carries min or max (grep gate: the emissive primitive list and payload fields).
- [ ] Flash limiter:
  - The HDR rule maps s to s ÷ (s + 1) and back exactly.
  - With the limiter on, a strobe between strength 4 and 20 is limited (pin P37). So are the example's rate-6 `flicker` and static writes alternating 0 and 1 six times a second: each presents at most 3 flashes in any 1 s (pins P14, P34).
  - Scripts and the wire see the unclamped brightness. With the limiter off, a step lands in one frame.
  - A volume is limited every frame, in view or not. A frame whose surface acquire fails leaves every clamp where it began. Loading another map starts every clamp fresh; a brightness write does not. Turning the limiter on presents that frame unchanged (pins P15–P18).
- [ ] The migration script:
  - Dry run lists renames and touched maps, writing nothing. A real run renames every slot sibling and rewrites references.
  - Refuses a collision with no partial write. Also refuses `neon_metal_x` and `neon_Metal_x`; `neon_metalwork_x` and `neon_panel` rename.
  - Names a former `neon_` texture on a mover or fluid face separately.
- [ ] combat-demo and reflection-probes-test compile with every former `neon_` face in a volume of max 4.0. At that max the bright panel's peak passes the bloom threshold and the dim panel's stays under it. This replaces `neon_emissive_strength_clears_the_bloom_threshold`, which reads `Material::Neon`.

### Manual
- [ ] combat-demo: the bright panel blooms and the dim one does not, as before. A script pulsing the bright panel's volume visibly breathes; at brightness 0 the panel shows its unlit diffuse.
- [ ] reflection-probes-test matches its pre-change look with its volumes in place, reflections of the panels included.
- [ ] Two-player co-op: a host trigger dims a sign on both screens; a joiner arriving after sees it dimmed.

## Path

- Prefix removal touches every consumer in `research.md` §Neon consumers. Order: retirement and the 1.0 default, then the volume. Each slice leaves the tree green. The dev maps take their volumes in the same commit that drops the 4.0, so no commit regresses the bloom A/B.
- Per-face index to the shader:
  - Chosen: the compiler writes per-volume face-index lists into the new section. At load the renderer builds a per-vertex volume id stream beside the world vertex buffer, as `spec_owner_buffer` rides slot 1, and the fragment indexes a per-frame strength table packed CPU-side.
  - Rival: a per-vertex field in Geometry (id 17), like `animated_block`. It needs unshared vertices (`animated_block_ids.rs` guard) and changes the Geometry ABI.
  - First slice: one volume around one face, one vertex shared with a non-member face, checking whether the shared vertex breaks the per-vertex id.
- Ownership rival: glow keys on the brush entity that owns the faces. Not chosen; argued in `research.md` §Ownership rival.
- Volume parse and peel follow `fog_volume`; several hulls per entity follow the environment volumes (`research.md` §Precedents).
- Scripting and replication: copy `sdk/lib/entities/fog_volumes.{ts,luau}`, the `setFog*` reaction primitives and the fog presentation binding. Reuse `AnimatableScalar<"brightness">`. Brightness is sampled on the CPU, as fog density is. Make the fog suites generic over kind rather than copying them, so parity stays one set of tests.
- Flash clamp: the HDR rule sits beside `ValueClamp` in `render-cpu` `flash_clamp`, where E23 finds it (`research.md` §Precedents for the keyed container).
- Docs: FGD entry; `docs/level_design.md` material table and an Emissive volumes section; `docs/scripting-reference.md` member table and primitives, saying beside the member table that emissive methods take brightness in [0, 1].

## Open questions

- A face split by the BSP into fragments that straddle a hull: does each fragment take its own centroid, or does membership follow the source brush side? — **delegated**: the executor picks whichever keeps one authored face whole and reports it.
- Strength table as a uniform or a storage buffer, within the forward pass's per-stage binding budgets — **delegated**.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Entity class | parsed classname | n/a | n/a | n/a | `emissive_volume` |
| Range min | record field | f32 | `component.min` | `component.min` | `emissive_min` |
| Range max | record field | f32 | `component.max` | `component.max` | `emissive_max` |
| Brightness | runtime component field | f32 (replicated) | `component.brightness` | `component.brightness` | n/a |
| Member kind | `getMapEntities` kind | `"emissive"` | `"emissive"` | `"emissive"` | n/a |
| Set primitive | reaction primitive | `"setEmissiveBrightness"` | `"setEmissiveBrightness"` | `"setEmissiveBrightness"` | n/a |
| Animation primitive | reaction primitive | `"setEmissiveAnimation"` | `"setEmissiveAnimation"` | `"setEmissiveAnimation"` | n/a |
| Tags | record tags | length-prefixed strings | `tags` | `tags` | `_tags` |

## Wire format

New PRL section `EmissiveVolumes`, id 60 (next free after `ProbeDepthTiles` 59). Little-endian; mirrors `FogVolumes` (id 30) record and tag encoding:
- **Header:** `u16 version` (= 1, any other rejected), `u32 volume_count` (≤ 256).
- **Per volume, in map order:**
  - `f32 min`, `f32 max` (stored after the compiler's swap and clamps)
  - `f32[3] origin`: the AABB centre, the co-op binding check
  - `u32 face_count`, then `face_count × u32` Geometry face indices, ascending, each in range and owned by exactly one volume
  - `u32 tag_count`, then each tag as `u32 length` + UTF-8 bytes
- A map with no volumes emits no section; the loader reads absence as no volumes.
- Co-op presentation state gains the emissive brightness kind and bumps `WIRE_VERSION` and `SNAPSHOT_VERSION` per `networking.md` §Version gates.
