# emissive-volumes

Brief · resumable · reads: `context/lib/resource_management.md` §3, §4.5, §5 · `context/lib/rendering_pipeline.md` §7.3, §7.8 · `context/lib/scripting.md` §10.2, §12 · `context/lib/networking.md` §Presentation state · `context/lib/build_pipeline.md` §Compiler pipeline step 1, step 9, §Baked texture mips, §PRL section IDs · read at eb44499

## Problem

Owner request: mappers set how strongly and in what colour a surface glows, per placement, and scripts animate it. Today glow strength comes only from the texture-name prefix: `neon_` gives 4.0, every other prefix 0 (`Material::emissive_strength`). So only `neon_` textures glow at all, every face sharing a texture glows alike, and nothing at runtime can change it. A prefix named for one game's look sets glow for every game on the engine. Strength and volumes ship together: bake, flash limiter and runtime must read one strength number, and retiring the prefix needs the strength file to keep existing looks. When done:
- Each texture's glow strength is content: 1.0 by default, or a value the game sets in its mod's `materials.toml`.
- A brush volume around a surface scales and tints that glow, between an authored minimum and maximum multiplier.
- Scripts pulse, fade, flicker, set or recolour that glow; co-op peers see host-driven changes.

## Decisions

- **Retire the `neon` prefix and `Material::Neon`.** Neon's shininess matches Default and its ricochet flag has no reader. Its flat surface depth goes too: a former `neon_` texture with an `_h` map carves as Default does.
  - `tools/gen_emissive.py` uses one default cutoff whatever the prefix; `--cutoff` stays the lever.
  - Diverges from `context/lib` wherever it describes prefix-derived glow, neon flatness or unlimited emissives: `resource_management.md` §3, §4.5, §4.6, §5.2, `rendering_pipeline.md` §7.3, §7.8 and `build_pipeline.md` §Compiler pipeline step 9 (material stage). Each is rewritten at promotion.
- **Per-texture strength lives in `<mod root>/materials.toml`, and only the bake reads it.** One `emissive` value per texture; a texture with an `_e` map and no entry is 1.0, one without `_e` is 0.
  - Keys are texture names as TrenchBroom shows them (`collection/name`), normalised as the material stage normalises map texture names. Two keys that normalise alike warn once, naming both lines, and the later wins.
  - prl-build bakes each texture's strength, and the mean colour of its glow texels (non-black `_e` texels, linear), into the level; runtime reads only those baked values. An edit re-runs only what reads strength (reflection capture and the emissive section); lightmap and SH irradiance caches stay warm.
  - Content-owned and per asset (`index.md` §2, "with the map or its assets"), as glTF's material-level `KHR_materials_emissive_strength` is (`research.md` §Strength source). Plain data: the compiler evaluates no mod script.
  - One file per mod root; layering across mods waits on multi-mod discovery (`boot_sequence.md`). It is the home for content-owned material properties, `emissive` first (`research.md` §Strength source).
  - A broken file never stops the compile: a syntax error warns, naming file and line, and every texture takes its default; an unknown key warns; a non-number value warns and reads its default. An entry for a texture with no `_e` map warns and is ignored.
- **`_e` stays an RGB colour map in linear light; black is zero glow.** `glow = _e × strength`. The diffuse map is the unlit surface and `_e` the lit one, so fading glow to 0 shows a real off state. `_e` alpha is reserved for a future per-texel emissive channel: author it opaque. Faces without an `_e` map pay no new per-fragment cost; the forward pass is the Mac target's usual bottleneck.
- **`emissive_volume` brush entity scales and tints its faces.** Keys:
  - `emissive_min` (default 0), `emissive_max` (default 1), `emissive_color` (default white, read as each FGD colour key is: channel ÷ 255, used as linear), `_tags`. A malformed colour warns and reads white.
  - A face's glow is its texture strength × (min + brightness × (max − min)) × colour. Brightness is a scripted value in [0, 1], 1 with no script.
  - Bad values never stop the compile (`index.md` §2, "Designers over structure"). Each key is fixed first: a negative value reads 0, a non-finite one its default, and a value past a ceiling the ceiling, each warning by key name. Then a max below min swaps, with a warning. The ceiling sits far above any look and below the scene target's half-float maximum. The same rules hold for `materials.toml` values.
  - Its brushes never render, collide or bake.
- **The map owns range and default colour; scripts own brightness and colour.** Lights and fog take absolute values through the same `pulse`/`fade`/`flicker`. Here those methods take brightness in [0, 1] and colour channels clamp to [0, 1], so a script cannot push a surface past the mapper's multiplier (`index.md` §2, "Defaults for novices").
- **Brightness writes and animations.** A brightness write clears any running brightness animation on its targets, as `setSpinRate` does, not as fog's static setters do. Clearing an animation leaves brightness at its last sampled value: the script's value, before any limiter. A colour write leaves brightness and its animation alone.
- **Membership is compile-time and per static world face.**
  - A face belongs to a volume when its centroid lies inside one of the volume's brush hulls.
  - Overlaps resolve smallest containing volume wins (summed brush volume, as `reflection_volume` and the environment volumes resolve), a tie going to the later in map order. Nesting works in any order, silently.
  - A volume left with no `_e` face warns, including one a smaller volume empties.
  - At most 256 volumes per map; past that the compile fails, naming the count.
- **Movers and fluids take texture strength only.** They bind the per-texture material, so `materials.toml` reaches them; no volume does. Scripted glow on a mover waits for content that asks. Model glow stays 0: models load only their diffuse slot.
- **Runtime owns brightness and colour; the bake reads the no-script state.** Reflection capture bakes each member face at texture strength × max × colour, its reference state (`research.md` §Capture reference state). Bounce means exclude glow texels on any texture with an `_e` map.
- **Photosensitivity: the existing clamp, fed what the surface shows.** While `accessibility.flashLimiter` is on, each volume passes the `flash_clamp` per-value rules. Its brightest member texture (strength × the luminance of its baked glow colour) sets the input: strength min(factor × that texture's strength × glow luminance, 1), colour that glow colour × the volume's colour, so the existing red-flash rule sees a red sign (`research.md` §Clamp input). Scripts and the wire carry the scripted values. E23 owns colour-only brightness strobes (white ↔ blue at high strength), the bloom halo and any budget shared across sources; at promotion its draft records emissive volumes as a clamp source already built.
- **Co-op: fog volumes' presentation-state behaviour** (`networking.md` §Presentation state), bound by emissive record index. It differs only where the Decisions above do: brightness writes clear animations, and values are brightness.
- **Migration.** `tools/migrate_neon_prefix.py` renames `neon_x` to `x` for every slot sibling, rewrites `.map` texture references, and writes `emissive = 4.0` for each renamed texture into the mod's `materials.toml`, so every look survives with no volume.
  - Dry-run by default.
  - Refuses on a name collision, or when `x`'s derived prefix (its first `_` token, case-folded, as the engine parses it) is a known material prefix.
- **Non-goals.**
  - Colour animation (cross-fades between colours): add when content asks; status lights set a colour.
  - Per-texel animation (phase-map sweeps, world-surface flipbooks): material authoring, a separate brief.
  - Area lights from glowing faces: a separate brief (`research.md` §Strength source).

### Scripting surface

```ts
// Proposed design
const signs = getMapEntities("emissive", { tag: "bar_sign" });
for (const sign of signs) {
  // sign.component: { min: number; max: number; brightness: number; color: [number, number, number] }
  reactions.push(defineReaction("levelLoad", { sequence: sign.flicker({ min: 0.3, max: 1, rate: 6 }) }));
}
reactions.push(defineReaction("powerCut", { primitive: "setEmissiveBrightness", tag: "bar_sign", args: { brightness: 0 } }));
reactions.push(defineReaction("doorLocked", { primitive: "setEmissiveColor", tag: "status_lamp", args: { color: [1, 0.05, 0.02] } }));
// Members also carry pulse({ min, max, periodMs }) and fade({ from, to, periodMs }).
// Brightness values and linear colour channels are in [0, 1]; out-of-range values clamp.
// The animation primitive is setEmissiveAnimation, args { periodMs, playCount?, brightness: number[] }
// with a non-empty curve, or args null to clear it, as setFogAnimation does.
// A finite playCount settles on its last sample. setEmissiveBrightness clears any running animation.
```

TypeScript and Luau twins both ship, wire byte-identical as the fog twins are.

## Acceptance

### Automated
- [ ] Texture strength:
  - A texture with an `_e` map and no `materials.toml` entry glows at 1.0, with no prefix and with a known prefix (`metal_`). One without `_e` packs 0 and samples no emissive texture; the forward shader reads the volume table only inside the `emissive_strength > 0` branch (shader-source test). A skinned model with an `_e` sibling still packs 0.
  - A `materials.toml` entry of 4.0 reaches world faces, a mover, and a fluid whose texture no world face uses; that entry warns nothing, and editing it leaves the reflection-capture cache warm (pin P47). Bad values follow the key rules. An entry for a texture with no `_e` map warns and packs 0 (pin P51). A syntax error, an unknown key and a non-number value each warn, naming file and line, and the compile finishes: a syntax error leaves every texture at its default, a non-number value leaves its texture at its default, and an unknown key is ignored while its table's `emissive` still applies. A mod with no `materials.toml` compiles silently at defaults. After a good build, a syntax error rebuilds at defaults, and no earlier strength survives the warm cache (pin P53). Keys match as the material stage normalises texture names (pin P46). Two keys that normalise alike warn once and the later wins (pin P54).
  - Bake and runtime agree for one texture: reflection-capture term, bounce-mean glow exclusion and material uniform. The runtime never opens `materials.toml` (grep gate: the file name appears in no crate but `level-compiler`).
  - Editing the entry of a texture a static world face uses misses the reflection-capture cache and rewrites the emissive section; lightmap, SH irradiance and `.prm` caches stay warm. A texture with an `_e` map and `emissive = 0` still excludes its glow texels from its bounce mean.
  - A former `neon_` name is an unknown prefix, warned once (grep gate: no `neon` in the prefix tables). With an `_h` map it carves as Default.
- [ ] `gen_emissive.py` without `--cutoff` uses one default whatever the file's prefix.
- [ ] Glow = texture strength × (min + brightness × (max − min)) × colour, at brightness 0, 0.5 and 1 and a non-white colour. No script means max and the authored colour. Brightness outside [0, 1] clamps, written or sampled from a curve (pin P10); a negative or non-finite colour channel reads 0, with one warning; a script colour channel above 1 clamps to 1 (pin P49). A malformed `emissive_color` warns and reads white; `255 128 0` reads (1, 0.502, 0).
- [ ] Bad keys compile, each with one warning naming the key: max < min swaps, a negative value reads 0, a non-finite one its default, one past the ceiling the ceiling. Each key is fixed before the pair swaps (pin P38). Valid keys, 0 included, compile silently. A texture at the ceiling inside a volume whose max is at the ceiling presents finite glow and finite bloom (pin P50).
- [ ] Membership:
  - A face whose centroid is inside a hull joins; one just outside does not. A face in the plane of a volume brush's side joins (pin P27).
  - A member face the atlas cuts into sub-faces glows as one (pin P29). A volume enclosing a mover brush and a fluid lists neither.
  - Two volumes that both contain a face centroid: the smaller wins, a tie going to the later, silently. A volume nested inside another keeps its faces whichever comes first in the map (pin P57). A volume with no `_e` face warns, including one a smaller volume empties (pin P28).
  - An emptied volume feeds the limiter 0 (pin P56).
  - Volume 256 compiles; volume 257 fails, naming the count.
- [ ] Adding a default volume (min 0, max 1, white) around `_e` faces changes only the emissive section: every other section is byte-identical to the same map without the volume.
- [ ] PRL round-trip of the emissive section. Every map emits it, defaults included; a PRL without it fails load as stale. Each texture's baked glow colour is the linear mean of its non-black `_e` texels. The loader refuses a version other than 1, counts over their caps, a face index out of range, descending or owned twice, and values the compiler never writes (min above max, negative or non-finite, past the ceiling, a colour channel above 1).
- [ ] At runtime a member face presents its glow and a non-member face sharing a vertex with it presents its texture strength, at brightness 0 and 1; in a 256-volume map the last volume's face does too (pin P36). Brightness written by a reaction drained in frame N presents in frame N (pin P12).
- [ ] Reflection capture shades a member face at texture strength × max × colour whatever its scripts do. Changing a volume's `emissive_max`, `emissive_color` or membership, or the strength of a texture a static world face uses, misses the reflection-capture cache.
- [ ] The Scripting surface example runs as a `content/dev` script, with `powerCut` and `doorLocked` fired by dev-map triggers' `on_fire`. `flicker` moves brightness within [0.3, 1]; `powerCut` holds 0; `doorLocked` turns the lamp red while its brightness and animation run on; a finite `fade` settles on its last sample. TS and Luau emit identical wire.
- [ ] Fog parity: the fog volume animation and presentation-state tests pass against the emissive kind, colour writes following fog's tint (pins P6–P11, P13, P19–P26, P35, P45, P55). The exception is a scalar write meeting a running animation: the differences row governs those (pin P30). Each cited pin has a case in the shared suite; where no fog test covers one, that case is added and runs for both kinds.
- [ ] Deliberate differences from fog:
  - A brightness write clears a running animation and holds (pins P1, P4). In one body the later write wins (pins P2, P3). A later identical animation restarts from its first sample (pin P30).
  - Across co-op: a host write reaches a client mid-flicker as that brightness with no animation (pin P31); a joiner after `powerCut` holds 0, although its own `levelLoad` installs the flicker (pin P32).
  - Clearing with args `null` leaves the last sampled brightness, before the limiter, and the wire carries it (pins P5, P33).
  - A colour write on a volume running a `flicker` keeps the flicker's start time and samples, on host and client (pin P44).
  - Only `setEmissiveBrightness`, `setEmissiveAnimation` and `setEmissiveColor` write a volume, and none carries min or max (grep gate: the three primitives' Rust argument types and TS and Luau arg shapes have no `min` or `max` field; `emissive_min` and `emissive_max` are read only in the compiler).
- [ ] Flash limiter:
  - With the limiter on, the example's rate-6 `flicker` and static writes alternating 0 and 1 six times a second each present at most 3 flashes in any 1 s (pins P14, P34).
  - The clamp reads peak strength, not brightness: on a volume with max 20, writes alternating brightness 0.025 and 0.05 are limited (pin P40). A strobe whose shown glow is at or above 1 both ways reads white both ways and is not limited (pin P37). A volume whose member texture strength is 4 and max 1, with brightness alternating 0.15 and 0.2, is limited: its peak swings 0.6 to 0.8 (pin P48). On a volume of max 20, a write from brightness 0.025 to 1 rises on screen no faster than the rate cap: min(presented peak, 1) moves at most 4.0/s, then presents 20 (pin P41).
  - The clamp reads the shown glow, not a white texel: a migrated strength-4 panel whose mean glow luminance is 0.43, flickering brightness 0.3 to 1, is limited (pin P58). A red `_e` sign under a white volume, strobing, meets the red-flash rule (pin P59).
  - Colour writes alternating red and green six times a second at brightness 1 reach the clamp as its colour, and the red-flash rule limits them (pin P42). A white ↔ blue strobe at max 20 is not limited here; E23 owns it (pin P43). A held red presents desaturated at unchanged luminance, while `component.color` and the wire carry the written red; a write away from red presents that frame (pin P52).
  - Scripts and the wire see the unclamped values; with the limiter off, a step lands in one frame. While the script clock is paused the clamp keeps aging on presented-frame time, and a flat `fade` counts no flash (pins P9, P13).
  - A volume is limited every frame, in view or not. A frame whose surface acquire fails leaves every clamp where it began. Loading another map starts every clamp fresh; a brightness write does not. Turning the limiter on presents that frame unchanged (pins P15–P18).
- [ ] The migration script: dry run lists renames and the `materials.toml` entries it would write, writing nothing; a real run renames every slot sibling, rewrites references and writes the entries. With an existing `materials.toml`, a real run keeps every other entry, and an existing entry for `x` refuses as a collision. It refuses a collision with no partial write, and refuses `neon_metal_x` and `neon_Metal_x`; `neon_metalwork_x` and `neon_panel` rename.
- [ ] At promotion, E23's draft names `emissive_volume` as a built clamp source (grep gate on the class name). No shader reads `_e` alpha (grep gate: every emissive texture sample in the renderer's shaders ends `.rgb`).
- [ ] combat-demo and reflection-probes-test compile after migration with no `emissive_volume`. The bright panel's peak passes the bloom threshold and the dim panel's stays under it.

### Measured (report, not gate)
Each run pins the fixture, Mac target machine class, cache mode, baseline (the same map before this change) and metric. A worse-than-expected result is the owner's cue to weigh another implementation, not a failed build.
- [ ] Forward-pass GPU time (`POSTRETRO_GPU_TIMING=1`, 120-readback window) on a fixture view filled with glowing faces: 0 volumes, 1, 256, and 256 with 16 animating, against the pre-change baseline. Repeat on a view with no `_e` faces, isolating the per-vertex stream's cost.
- [ ] CPU per-frame cost of the volume update (curve sampling, clamp, packing) at 256 volumes with 16 animating, limiter on and off.
- [ ] Bytes uploaded per frame: zero on a frame where no volume changed; one entry per changed volume otherwise.
- [ ] prl-build stage time for membership and `materials.toml` on the hallway map, warm and cold, against the pre-change baseline.

### Manual
- [ ] combat-demo: the bright panel blooms and the dim one does not, as before. A volume pulsing the bright panel visibly breathes; at brightness 0 it shows its unlit diffuse. A grey-`_e` status lamp switches red and green.
- [ ] reflection-probes-test matches its pre-change look, reflections of the panels included.
- [ ] Two-player co-op: a host trigger dims a sign and recolours a lamp on both screens; a joiner arriving after sees both.

## Path

- Prefix removal touches every consumer in `research.md` §Neon consumers. Order: `materials.toml` and the per-texture strength, then retirement with the dev migration in the same commit, then the volume. Each slice leaves the tree green and the bloom A/B intact.
- `materials.toml` is read by the material stage (`build_pipeline.md` step 9), which already resolves textures before the lighting bakes and owns the shared material table; alias normalization follows that stage's rule. Strength enters only the reflection-capture key, as a value: `.prm` keys hash image bytes and are shared across names, and bounce means read only the `_e`-loaded predicate. Fluid-only textures join TextureNames after the material stage, so the strength table and any entry checks run after that append.
- Per-face index to the shader:
  - Chosen: the compiler writes per-volume face-index lists into the emissive section. At load the renderer builds a per-vertex volume id stream beside the world vertex buffer, as `spec_owner_buffer` rides slot 1, and the fragment indexes a per-frame table of volume factor × colour packed CPU-side. Texture strength stays in the per-material uniform.
  - Rival: a per-vertex field in Geometry (id 17), like `animated_block`. It needs unshared vertices (`animated_block_ids.rs` guard) and changes the Geometry ABI.
  - First slice: one volume around one face, one vertex shared with a non-member face.
- Ownership rival: glow keys on the brush entity that owns the faces. Not chosen; argued in `research.md` §Ownership rival.
- Upload the volume table only when a value changed; a static frame uploads nothing. Before adding a vertex stream, check whether the 36-byte world vertex has padding that can carry the volume id, which would leave untouched faces with no extra vertex fetch.
- Volume parse and peel follow `fog_volume`; several hulls per entity follow the environment volumes (`research.md` §Precedents).
- Scripting and replication: copy `sdk/lib/entities/fog_volumes.{ts,luau}`, the `setFog*` reaction primitives and the fog presentation binding. Reuse `AnimatableScalar<"brightness">`; colour follows fog's `tint`. Fog's presentation cases share test functions with lights, emitters and spin (`presentation_state.rs`, `presentation_conditioned_tests.rs`); parameterise only the cases the parity row cites.
- Flash clamp: no change to `flash_clamp`; each volume feeds min(peak strength, 1) to a public `ValueClamp` (`research.md` §Precedents for the keyed container).
- Docs: FGD entry; `docs/level_design.md` material table, `materials.toml` and an Emissive volumes section; `docs/scripting-reference.md` member table and primitives, saying beside the member table that emissive methods take brightness in [0, 1].

## Open questions

- A face split by the BSP into fragments that straddle a hull: does each fragment take its own centroid, or does membership follow the source brush side? — **delegated**: the executor picks whichever keeps one authored face whole and reports it.
- Volume table as a uniform or a storage buffer, within the forward pass's per-stage binding budgets — **delegated**.
- The value ceiling — **delegated** within the Decision's bounds.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP / file |
|---|---|---|---|---|---|
| Texture strength, glow colour | material-stage values | f32 + f32 × 3 linear per texture | n/a | n/a | `materials.toml`: `["<collection>/<name>"]` table (normalised as the material stage normalises map texture names), key `emissive` |
| Entity class | parsed classname | n/a | n/a | n/a | `emissive_volume` |
| Range min | record field | f32 | `component.min` | `component.min` | `emissive_min` |
| Range max | record field | f32 | `component.max` | `component.max` | `emissive_max` |
| Colour | record + runtime field | f32 × 3 linear (replicated) | `component.color` | `component.color` | `emissive_color` (color255, channel ÷ 255 as linear) |
| Brightness | runtime component field | f32 (replicated) | `component.brightness` | `component.brightness` | n/a |
| Member kind | `getMapEntities` kind | `"emissive"` | `"emissive"` | `"emissive"` | n/a |
| Set brightness | reaction primitive | `"setEmissiveBrightness"` | `"setEmissiveBrightness"` | `"setEmissiveBrightness"` | n/a |
| Set colour | reaction primitive | `"setEmissiveColor"` | `"setEmissiveColor"` | `"setEmissiveColor"` | n/a |
| Animation | reaction primitive | `"setEmissiveAnimation"` | `"setEmissiveAnimation"` | `"setEmissiveAnimation"` | n/a |
| Tags | record tags | length-prefixed strings | `tags` | `tags` | `_tags` |

## Wire format

New PRL section `Emissive`, id 60 (next free after `ProbeDepthTiles` 59). Little-endian; volume records mirror `FogVolumes` (id 30) record and tag encoding:
- **Header:** `u16 version` (= 1, any other rejected).
- **Textures:** `u32 texture_count` (= the level's TextureNames count), then per texture in TextureNames order: `f32` strength, `f32[3]` mean glow colour (linear).
- **Volumes:** `u32 volume_count` (≤ 256), then per volume, in map order:
  - `f32 min`, `f32 max` (after the compiler's fixes and swap), `f32[3] color` (linear)
  - `f32[3] origin`: the AABB centre, the co-op binding check
  - `u32 face_count`, then `face_count × u32` Geometry face indices, ascending, each in range and owned by exactly one volume
  - `u32 tag_count`, then each tag as `u32 length` + UTF-8 bytes
- Every map emits the section; a PRL without it fails load as stale.
- Co-op presentation state gains the emissive kind (brightness, animation, colour) and bumps `WIRE_VERSION` and `SNAPSHOT_VERSION` per `networking.md` §Version gates.
