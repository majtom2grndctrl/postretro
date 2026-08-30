# Build Pipeline

> **Read this when:** setting up the map authoring toolchain, modifying the asset pipeline, adding custom entities, or debugging map compilation issues.
> **Key invariant:** maps are authored in TrenchBroom today; PRL is the sole runtime map format, and source-format vocabulary stops at the compiler's `format/` adapter (§Source-format neutrality). Engine canonical unit: 1 unit = 1 meter.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md)

---

## Pipeline Overview

Maps are authored in TrenchBroom, compiled to PRL with prl-build:

```
TrenchBroom (.map) ──► prl-build (postretro-level-compiler) ──► PRL file (.prl) + .prm sidecars

Engine loads PRL + .prm sidecars at runtime (PNGs for UI only)
```

prl-build builds a BSP tree as a compiler intermediate, generates portal geometry, builds a global BVH over all static triangles, and packs runtime data into a custom binary format. BSP drives spatial partitioning and portal generation at compile time only. Runtime consumes cells, a cell locator, portals, and BVH arrays; it does not load or walk `BspNodes` / `BspLeaves`. Engine loads via the `postretro-level-format` crate.

---

## Supported Map Formats

prl-build accepts idTech2 `.map` files (Quake 1/2 dialect, parsed via shambler/shalrath). Unit scale: 1 unit = 0.0254 m (one inch, exact).

Both Standard (axis-aligned) and Valve 220 (explicit UV axes) texture projections are supported. Shalrath auto-detects per face; they can coexist in one `.map` file.

### Source-format neutrality

> **Key invariant:** PRL and every shared compiler stage are source-format agnostic. Quake and TrenchBroom vocabulary stops at the `format/` adapter.

`.map` is the only input format today. That is a content decision, not an architectural one. PRL is the engine's contract; the Quake dialect is one front end that targets it. A second front end — a different editor, a mesh-based authoring format, a procedural generator — must be able to reach PRL without touching a shared stage.

`crates/level-compiler/src/format/` is that boundary. One module per source format. Each translates its own vocabulary into the canonical map representation (`crates/level-compiler/src/map_data.rs`) before shared logic runs. Downstream stages — BSP, portals, geometry, BVH, bakes, pack — never branch on which format produced their input and never see source vocabulary.

The rule is not "avoid Quake artifacts." Quake-shaped authoring is fine and expected. The rule is that those artifacts are translated, not propagated.

| Source-format concern | Canonical form |
|---|---|
| Coordinate axes and handedness (Quake Z-up) | Engine convention (Y-up) |
| Units (1 unit = 0.0254 m) | Meters |
| Angle encoding, spotlight direction convention | Canonical light orientation |
| Radiosity intensity reference (mapper-authored `light 300`) | Linear intensity |
| Texture projection dialects (Standard, Valve 220) | Resolved UV axes |
| Classnames and FGD property names | Canonical archetypes and typed fields |
| Editor-only containers and keys (`func_group`, `_tb_*`) | Flattened into the world or dropped, before shared logic |
| Text-format quoting and escaping quirks | Decoded values |

**The line test:** would a different input format need a different answer? Yes — adapter. No — shared stage.

**The FGD is a front-end schema, not the engine's entity model.** It expresses canonical archetypes for one editor. A second front end brings its own schema and reaches the same canonical vocabulary. Entity semantics live in the canonical layer; the FGD projects them.

Two failure modes this invariant exists to prevent:

- **Vocabulary leak.** Source-format naming or convention reaching a PRL section or a shared stage. Invisible until a second format arrives with no answer for it.
- **Canonical layer shaped by the source.** Defining a canonical field by what Quake happens to do rather than by what the engine needs. A Quake-shaped canonical layer is a leak with extra steps — the adapter still exists, but it has nothing left to translate.

---

## PNG Texture Pipeline

No WAD files. Textures are authored as PNGs.

| Stage | What happens |
|-------|-------------|
| Author | Create PNGs in `content/<mod>/textures/<collection>/<name>.png` (where `<mod>` is `base` for first-party content or `tests` for fixtures). TrenchBroom requires one subdirectory level. |
| TrenchBroom | Browses the textures directory via the Postretro game config. |
| prl-build | Reads PNGs, decodes them, runs Mitchell-Netravali downsampling in linear color space, and writes per-texture `.prm` mip sidecars to `<workspace>/baked/materials/<blake3-hex>.prm`. Stores a content-addressed blake3 key per texture in the `TextureCacheKeys` PRL section. Authored PNGs are not shipped or read at runtime for world materials. |
| PRL output | `TextureNames` section stores a deduplicated texture name list (verbatim from the `.map`, possibly collection-qualified). `TextureCacheKeys` section stores one 32-byte blake3 per name entry. No pixel data. |
| Engine | Loads `.prm` sidecars at level load via the blake3 keys in `TextureCacheKeys`. Never opens a PNG for world materials. UI textures (splash, HUD) still load directly from PNGs. |

### Texture name resolution (compile time)

TrenchBroom identifies materials by their path **relative to the textures root**, so a `.map` may carry a **collection-qualified** name (e.g. `50-free-textures/concrete_pavement_036`) rather than the bare stem. Hand-authored maps may also use bare stems. `prl-build` (`crates/level-compiler/src/texture_mips.rs`) handles both:

- The name→PNG index (`build_name_to_path_map`) keys each PNG under its path **relative to the texture root** — forward-slashed, lowercased, extension stripped (e.g. `50-free-textures/concrete_pavement_036`). It also inserts a **bare-stem alias** (`concrete_pavement_036`) for back-compat, but only when that stem is unique across all collections. On a stem collision the alias is dropped and a `warn!` names both paths, so a bare name never silently resolves to the wrong collection.
- The incoming map name is normalized (lowercase, `\`→`/`, leading `textures/` stripped) so both `collection/stem` and root-inclusive `textures/collection/stem` map to the relative key. Lookup tries the normalized relative name, then falls back to the bare last path segment.
- A qualified base remains selected when any bundle slot exists there, including sibling-only materials. Only an entirely missing qualified bundle falls back to the unique bare alias. All four slots resolve from the selected base, so siblings never cross collections.

A material name with a space (e.g. a collection dir `Level Eleven Games Sci-Fi Texture Pack v1`) is double-quoted in the `.map` by TrenchBroom. shalrath has no quote handling, so the parser (`crates/level-compiler/src/parse.rs`) runs a pre-parse pass that strips the quotes and swaps interior spaces for a path-illegal sentinel byte, keeping the material field one token. The sentinel is decoded back to a real space at the single texture-read boundary, so every downstream stage sees the human-readable name.

---

## Custom FGD

Project deliverable alongside the engine. Defines Postretro-specific entities for TrenchBroom.

| Entity | Type | Purpose | Key Properties |
|--------|------|---------|----------------|
| `light` | point | Omnidirectional baked light | `light` (intensity), `_color` (RGB), `_falloff_range` (falloff distance, required), `_light_size` (bake-only emitter radius for soft shadows; absent → 0.25, authored 0 → hard shadow), `delay` (falloff model), `style` (animation), `_phase` (style cycle offset), `_bake_only` (bakes but no runtime presence; default 0), `_shadow_type` (static-geo shadow technique: `static_light_map` default, or `sdf`). `_cast_entity_shadows` does NOT apply to baked lights (it lives on the dynamic tier; the compiler warn-clears it here). `_dynamic` retired in Task 1b of `sdf-static-occluder-shadows`; `is_dynamic` is classname-derived. |
| `light_spot` | point | Baked spotlight with cone | + `_cone`, `_cone2` (inner/outer angles), `angles` (direction). Shares `_light_size`. |
| `light_sun` | point | Baked directional sun light | + `angles` (direction vector), `_angular_diameter` (bake-only soft-shadow source angle in degrees; absent → 0.5, authored 0 → hard shadow) |
| `light_dynamic` | point | Dynamic (runtime, unbaked) omnidirectional light | `light` (intensity), `_color` (RGB), `_falloff_range`, `delay` (falloff model), `_cast_entity_shadows` (whether the light casts shadows from moving ENTITIES; dynamic-tier only, default ON), `_tags`. Bakes into nothing; evaluated at runtime via the shadow-map pool. `is_dynamic` set from the classname. |
| `light_dynamic_spot` | point | Dynamic (runtime, unbaked) spotlight with cone | + `_cone`, `_cone2` (inner/outer angles), `angles` (direction). Same dynamic-tier semantics as `light_dynamic`. |
| `fog_volume` | brush | Per-region fog; geometry behaviour auto-detected — axis-aligned brushes (every face normal ±X/±Y/±Z) become an ellipsoid inscribed in the AABB; non-axis-aligned brushes become a plane-bounded convex hull | `density`, `glow`, `edge_softness` (plane-bounded only), `falloff` (axis-aligned only), `tint`, `saturation`, `min_brightness`, `light_range`, `scatter_bias`, `ambient_scatter`, `_tags` (ambient color is SH-derived; no `color` KVP) |
| `sh_protect_volume` | brush | Keeps intersecting SH delta-probe bricks dense during coarsening; excluded from static world geometry, collision, and runtime entities | `dilation` (world/engine-meter AABB expansion; default 0; negative rejected) |
| `fog_lamp` | point | Spherical halo fog emitter; default warm amber | `density`, `glow`, `radius` (sphere radius; sizes AABB), `radial_falloff`, `tint`, `saturation`, `min_brightness`, `light_range`, `scatter_bias`, `ambient_scatter`, `_tags` (ambient color is SH-derived; no `color` KVP) |
| `fog_tube` | point | Capsule-strip fog emitter; default cool blue-white | `density`, `glow`, `radius` (capsule radius), `height` (capsule length), `pitch` / `yaw` (capsule axis), `radial_falloff`, `tint`, `saturation`, `min_brightness`, `light_range`, `scatter_bias`, `ambient_scatter`, `_tags` (ambient color is SH-derived; no `color` KVP) |
| `billboard_emitter` | point | Billboard particle emitter | `rate` (particles/sec; default 6), `lifetime` (seconds; default 3), `spread` (cone half-angle radians; default 0.4), `buoyancy` (-1=falls, 0=floats, >0=rises; default 0.2), `drag` (velocity damping/sec; default 0.8), `sprite` (collection name; default "smoke"), `initial_velocity_x/y/z` (default 0/0.8/0), `color_r/g/b` (linear; default 1/1/1), `spin_rate` (radians/sec; default 0) |
| `prop_mesh` | point | Map-placed skinned-model entity | `model` (content-relative glTF path; required — absent or unresolvable logs a warning, load continues) |
| `env_cubemap` | point | Reflection probe position | `size` (resolution per face; default 256) |
| `env_reverb_zone` | brush | Acoustic zone | `reverb_type`, `decay_time`, `occlusion_factor` |
| `kinematic_mover` | brush | Runtime-moving brush payload, excluded from static BSP/BVH/lightmap/collision | `name`, `path` (first waypoint name), `move_mode` (`once`/`ping_pong`), `speed`, `wait_ms`, `start_on_spawn`, spin/carry fields, `block_policy` (`displace`/`reverse`/`stop`/`crush`), `crush_damage`, `crush_interval_ms`, `auto_close_ms` (blank inherits mod default; 0 disables), `open_event`, `close_event`, `blocked_event`, `crush_event`, `_tags` |
| `kinematic_waypoint` | point | Waypoint in a mover path | `name`, `next` (optional next waypoint name), `origin` |
| `trigger_volume` | brush | Invisible AABB; touch/use activation runs a direct mover command and named enter/paired-exit reactions; excluded from static BSP/BVH/collision/lightmap/SDF/portals/navmesh | `activation` (`touch`/`use`), `target_tag`, `on_fire`, `on_exit`, `command` (`start`/`stop`/`reverse`/`go_to_path_node`), `command_arg` (required node name for go-to), `fire_mode` (`once`/`multiple`), `rearm_ms`, `enabled_on_spawn`, `_tags`. Blank optional KVPs (`activation`, `command`, `fire_mode`, `rearm_ms`, `enabled_on_spawn`) fall back to their declared defaults — a field cleared in TrenchBroom is unset, not a compile error |
| `switch` | brush | Visible, solid, pressable brush; compiles as static world geometry **and** a `use`-activation trigger volume whose faces grow toward `use_reach`, each clamped to the free space the compiler can prove is in front of it; the geometry never moves. Exactly one brush per switch — zero brushes or more than one is a compile error | `name`, `use_reach` (per-face press margin, map units; default 24 ≈ 0.61 m; must exceed the compiler's flush-tolerance floor, a few hundredths of a map unit, and be ≤ 128 — outside that range is a compile error), `target_tag`, `on_fire`, `on_exit`, `command` (`start`/`stop`/`reverse`/`go_to_path_node`), `command_arg`, `fire_mode` (`once`/`multiple`), `rearm_ms`, `enabled_on_spawn`, `_tags`; blanks fall back to defaults as for `trigger_volume`, `use_reach` included. No `activation` — always `use`; an authored one warns and is discarded |
| `worldspawn` | special | Scene-wide load-time settings | `data_script` (path to entry `.ts`, `.js`, or `.luau` data script, relative to `.map`; TypeScript and JavaScript compile via `scripts-build`, Luau passes through; its compile-time evaluation emits the compiler-only light-membership manifest; absent = no data script), `ambient_color` (RGB ambient floor), `fog_pixel_scale` (volumetric pass resolution divisor; default 4, range 1–8), `_lightmap_density` (lightmap bake density, meters per texel; default 0.04; finer = higher resolution; `--lightmap-density` CLI overrides; non-finite/≤0 warns and falls back to default), `_sh_coarsen` (`"0"` selects the uniform-L0 id-41 fallback; absent or any other value keeps direct-delta coarsening enabled), `entity_shadow_min_intensity_ratio` (static-light entity-shadow selection intensity floor as ratio of map max; default 0.5), `entity_shadow_min_range` (static-light entity-shadow selection falloff floor in meters; default 4.0), `initialGravity` (world gravity in m/s²; negative = downward; default standard Earth = -9.81; malformed/non-finite supplied values are compile errors) |

**Load-time gameplay declarations.** Maps may seed level-wide gameplay values at load for accessible non-script authoring. They cannot mutate gameplay values after load or override per-archetype descriptor tuning. `initialGravity` is a level declaration; player movement tuning remains descriptor-owned.

**Mover authoring split.** KVPs define basic deterministic kinematic motion. Brush tags bind movers to triggers and reactions for richer event-driven behavior.

### Entity resolution

- **`light`, `light_spot`, `light_sun`** (baked tier) and **`light_dynamic`, `light_dynamic_spot`** (dynamic tier) — all validated at compile time (falloff distance required, spotlight direction verified, intensity bounds checked). Baked-tier lights feed the SH irradiance volume baker and the directional lightmap baker; dynamic-tier lights bake into nothing and feed the runtime direct lighting buffer + shadow-map pool. The tier is set by classname, not a KVP. Compilation fails on validation errors.
- **`fog_volume`** — resolved at load time to world-space AABBs, shape, and fog parameters. Uploaded as a compact storage buffer (up to 16 entries). Per-sample test: shape membership (AABB as conservative bound), then optional half-space clip plane (normal points into the removed region). No BSP traversal at runtime.
- **`sh_protect_volume`** — compiler resolves the brush hull to a world/engine-space AABB, expands all faces by `dilation` meters, then keeps every intersecting 4×4×4 SH delta-probe brick at L0 before seam smoothing. It emits no runtime entity or PRL section.
- **`kinematic_mover`** — compiler normalizes a finite local `spin_axis`, validates finite `spin_speed` and finite non-negative `spin_accel`, then packs the static rotation authoring with the mover. Authored non-zero speed and positive acceleration must remain non-zero after conversion from degrees to runtime radians. A non-zero spin requires a non-zero axis and may use one waypoint as a pure rotator; zero-spin movers still require at least two waypoints.
- **`trigger_volume`** — compiler extracts each brush as a world-space AABB with activation, direct-command, and named enter/exit event data. Runtime preserves direct mover commands and dispatches bound reactions; the brush has no static geometry or collision role. Optional KVPs resolve through one shared path with `switch`: a blank value is unset and takes the key's declared default, never a parse or unknown-value error.
- **`switch`** — compile-time sugar over two shipped mechanisms; no runtime type, no PRL addition. The brush folds into the static world brush set (the switch renders, takes baked light, and collides like worldspawn brushwork) *and* resolves to a trigger volume with `activation` forced to `use`, whose AABB then grows toward `use_reach` on every face. Growth is part of the mechanism: use-activation is a capsule/AABB intersection test and the brush is solid, so the volume must reach past the switch face into the space the player occupies. The press test measures from the capsule axis, not its surface, so a face's effective reach is its clamped margin plus the player capsule's radius. That radius is an authored descriptor field with no engine default, so effective reach moves with the content: at the repo's only authored capsule (0.2 m ≈ 7.9 map units) and the default `use_reach`, about 31.9 map units. Growing blindly reaches *through* a wall, so each face's allowance is clamped to the occluders standing past it. The invariant: **no grown face extends past the near side of any occluder the compiler could not rule out of the corridor that face grows into.** A hull qualifies as an occluder for one face on three conditions — its AABB overlaps that face's cross-section by positive area; its AABB stands past the face along the growth axis by more than the flush tolerance; and no single one of its own brush planes separates the *growth prism*, the cross-section extended along the growth axis to the full margin. Edge-on contact is deliberately not overlap: a flush mount zeroes the face it backs and leaves the four faces it touches edge-on fronting open room, so flush mounting is a zero gap rather than a special case, and any gap width clamps the same way. An occluder that covers the cross-section and extends past both faces on an axis embeds the brush; both those faces get zero. The plane test earns its cost — on the AABB alone, a diagonal partition, wedge, ramp, or chamfer far from the switch could straddle it on every axis and zero all six faces against geometry that was never in front of them. It is a *separating-plane* test: a plane putting the prism's interior-nearest corner outside proves no intersection, but surviving every plane does not prove intersection. The result is therefore conservative, not exact — it can keep an occluder a finer test would drop, which costs reach rather than leaking it. Partial cross-section overlap clamps as readily as full overlap, and that is correct: the trigger is an AABB, so growing *part* of a face is not representable, and a console sunk one unit into its mount cannot grow horizontally at all. Occluders are the static world hulls plus `kinematic_mover` hulls, minus the switch's own brush, movers taken at their **authored (compile-time)** position. When no *horizontal* face grew (engine space is Y-up, so a standing player reaches along X and Z), the compiler warns with the switch's name and position and names which horizontal faces were clamped and against what. The wording stays at what the compiler concluded — a conservative test cannot claim the switch is walled in on every side. A flush wall mount zeroes exactly one horizontal face and stays silent. Known gaps in the invariant: a mover authored *clear* of the switch that later moves *into* the corridor — a blast door authored open and closed by this very switch — is no occluder at compile time, so the face grows fully and at runtime the volume sits behind a closed solid (the opposite direction, authored across the corridor and later moving away, only costs reach); clamping uses the *ungrown* cross-section one axis at a time, leaving diagonally adjacent geometry that only intersects the grown box's corner unaccounted for; and a brush with no usable vertices has an empty AABB and clamps nothing. Exactly one brush per switch — a brushless switch and a multi-brush switch are both compile errors, the latter because the press volume is the union AABB and spans the room between two consoles. Because a switch *is* a trigger volume at the component layer, `trigger_volume` component queries also return switches.
- **`billboard_emitter`** — resolved at level load via the built-in classname dispatch table. The engine spawns an ECS entity with a `BillboardEmitterComponent` configured from the map's KVPs. See §Built-in classname routing below.
- **`prop_mesh`** — resolved at level load via the built-in classname dispatch table. The engine spawns a `Transform` + `MeshComponent { model }` entity at `entity.origin`; the renderer loads and uploads the model into its handle→model cache once per distinct path. See §Built-in classname routing below.
- **`env_cubemap`** — marks a position for offline cubemap baking. Bake tool is out of initial scope.
- **`env_reverb_zone`** — future audio input. Resolve through runtime cell IDs, not BSP leaves. Each matched cell gets spatial reverb parameters for the audio subsystem.

---

## Built-in Classname Routing

The level loader resolves FGD `classname` values against an engine-side handler table (`ClassnameDispatch`) at level load. This table is populated once at engine init by `register_builtins()` and is never cleared on level unload — built-in handlers describe engine types, not per-level state.

For each map entity:
1. Look up `entity.classname` in the built-in handler table.
2. If found: instantiate the configured components, apply the KVP map, spawn the ECS entity at `entity.origin`, copy `_tags`.
3. If not found: `log::debug!` and skip — unregistered classnames are valid in maps that don't use them.

Invalid KVP values log a warning naming the key and entity origin, fall back to the documented default, and load continues.

**Current built-in types:** `billboard_emitter`, `prop_mesh`. `kinematic_mover` and `kinematic_waypoint` are compiler/runtime special entities consumed by PRL `KinematicGeometry`, not generic classname dispatch.

**Two-sweep dispatch.** After the built-in pass, the loader runs a second sweep against script-registered entity types declared on `ModManifest.entities`. The built-in pass returns the set of classnames it attempted to handle; the second sweep skips any classname in that set. Built-ins win on collision even when the built-in handler failed to spawn (e.g. registry exhausted) — a classname is owned by exactly one of the two paths for the lifetime of the level. Collisions log a `warn!` once per classname. The second sweep matches placements against each descriptor's `canonicalName`; descriptors with no `canonicalName` are skipped (marker-only archetypes — see `scripting.md §2`). Any placement whose classname is not matched by either sweep and is not in the engine-special exclusion set (`worldspawn`, `player_spawn`) logs a `warn!` once per classname per sweep, naming the placement origin. See `context/lib/scripting.md §2` for the data context lifecycle that populates the descriptor table consumed by the second sweep.

---

## Surface Material Derivation

Texture name prefix maps to a material enum. Drives footstep sounds, bullet impacts, and decals. The engine provides the prefix-to-material lookup mechanism; which prefixes exist is a game content concern. The table grows as content requires it.

Example: `metal_floor_01` → Metal, `concrete_wall_03` → Concrete. See `resource_management.md` §3 for the full mechanism and behavior hooks.

Unknown prefix falls back to a default material with a warning at load time.

---

## PRL Compilation

### Compiler pipeline

```
parse .map → scripts-build emits light-membership manifest → prl-build applies membership → extract kinematic and trigger brush entities → BSP construction → brush-side projection → portal generation → exterior leaf culling → geometry → BVH → lightmap bake → octahedral irradiance volume bake → pack .prl
```

1. **Parse.** Extracts brush volumes, brush sides, and entities. Applies coordinate transform (Quake Z-up → engine Y-up) and unit scale. Light entities route to FGD translation and validation; they don't participate in BSP construction. TrenchBroom `func_group` editor groups are flattened into the static world brush set at the Quake adapter boundary so grouped brushes compile like worldspawn brushes. `kinematic_mover` brush entities are peeled off before static world construction and packed as origin-relative geometry plus waypoint records. `trigger_volume` brushes are peeled off as invisible AABBs and packed with activation, direct-command, and named enter/exit event data. `sh_protect_volume` brushes are peeled off as world/engine-space AABBs for the optional SH coarsening classifier; `dilation` expands them in meters and they produce no runtime payload. `switch` brushes go both ways: they join the static world brush set *and* are peeled off as a `use` trigger AABB at the raw brush hull. Their `use_reach` margins land in a later pass, after the static world brush set and the mover brush set are both complete — how far a face may grow depends on the hulls the compiler cannot rule out of the corridor in front of it, tested AABB-first and then against each candidate brush's own planes, and that question cannot be answered while the entity sweep is still collecting them.
2. **Script-derived light membership.** `scripts-build` evaluates the map data script against the parsed map-light table and emits a validated sidecar. `prl-build` reads it before bake namespaces form. A static light targeted by `setLightAnimation` reserves the existing animated-light bake structures — animated chunks, weight maps, and indirect delta data — even without `_animated 1`. The sidecar is compiler-only and is not a PRL section. Dynamic lights remain runtime-only and reserve no baked structures.
3. **BSP construction.** Partitions world space into solid and empty leaves using brush-derived planes. Leaf solidity is established during construction from the brush half-space intersection — not inferred from face positions afterward. Classification is exact for arbitrary convex brushes: leaf regions are carried as convex polytopes (not AABB approximations), and a leaf is solid when any single brush fully contains it — so abutting non-cuboid brushes cull shared interior faces identically to cuboids.
4. **Brush-side projection.** Derives visible world faces from brush sides. Produces triangulated geometry per empty leaf; faces in solid space are discarded.
5. **Portal generation.** Clips splitting-plane polygons against ancestor planes to produce convex portals connecting adjacent empty leaves. Always runs; portals are stored in every PRL for runtime traversal.
6. **Exterior leaf culling.** Flood-fills through the portal graph from outside the map boundary. Exterior-reachable leaves produce no geometry. A map with a leak has interior leaves incorrectly classified as exterior.
7. **Geometry.** Fan-triangulates faces into a global vertex/index buffer. Associates each face with a material bucket and cell ID.
8. **BVH.** Builds a global SAH BVH over all static geometry organized by `(face, material_bucket)` pair. Flattens to dense arrays; leaves sorted by material bucket for contiguous per-bucket indirect draw slots.
9. **Lightmap bake.** UV-unwraps world geometry into a lightmap atlas. Ray-casts per-texel irradiance and dominant incoming light direction from all static lights against the global BVH. Static `static_light_map` shadows are baked as **soft area-light visibility** (stratified shadow-ray sampling of each emitter, multiplied into irradiance), not a hard 1-texel gate; an authored `_light_size`/`_angular_diameter` of `0` short-circuits back to a single hard ray. Per-layer atlas dimensions are bounded (cap 8192² per layer, which requires matching device `max_texture_dimension_2d` support — checked at renderer init); default density is 0.04 m/texel, and a per-map `_lightmap_density` worldspawn KVP opts a map into finer density. Charts that overflow one layer's area spill into additional array layers (up to a fixed layer cap); a single chart too large for one layer hard-fails the build. A per-light warning fires when an emitter is too small to soften at the atlas density (sub-texel penumbra). Skipped when the map has no static lights.
   - **Soft-shadow bake cost.** Soft visibility multiplies each stage's per-(hit × light) shadow-ray cost by the area-sample count, so penumbra-heavy maps pay a multi-fold bake-time increase over the hard-gate path. Adaptive escalation (a 4-ray probe set, escalating to the full count only in penumbras) keeps fully-lit/fully-shadowed texels cheap and bounds that cost. Lightmap layers and ShadowmaskAtlas memos are cached, so unchanged inputs pay that cost once. The SH indirect-bounce delta path is cache-less but low-frequency. `--soft-shadow-samples` (default 32) sets the escalated full-sample count. Raising it invalidates cached lightmap layers and shadowmask memos keyed through selected layer hashes. The uncached animated weight-map stage recomputes from scratch every build. Adaptive-escalation thresholds stay fixed constants regardless, so the bake stays deterministic.
10. **Octahedral irradiance volume bake.** Bakes static-light indirect irradiance into octahedral atlas tiles and isotropic Chebyshev depth moments. When animated lights are present, also bakes the sparse indirect-only delta tile companion for runtime composition.
11. **Pack.** Writes all sections to the `.prl` binary format.

### Progress reporting, controls, and logging

The stage sequence above is orchestrated by `pipeline.rs`, which is decoupled from how progress is
presented. Presentation goes through a `Reporter` trait (`reporter.rs`) with two frontends: a
`PlainReporter` for non-TTY runs (CI, pipes, `xtask`, redirected streams) that emits timestamped stage
lines plus discrete percent/ETA lines, and a `TuiReporter` (`tui.rs` and its submodules) that drives a
ratatui terminal UI when stdin, stdout, and stderr are all TTYs. The orchestrator calls the reporter
per stage (begin → optionally declare a progress handle → finish or skip) and never assumes which
frontend is active.

- **Governor** (`governor.rs`) is the single cooperative gate for pause/resume and core-throttle. Serial
  loops call `checkpoint()` (parks while paused); parallel work items call `enter()` exactly once at
  their outermost boundary (parks while paused or at the permit cap). A permitted item must never wait
  on another permitted item — that would deadlock at one permit. `BakeControl` (`bake_control.rs`)
  carries the governor plus a stage's display-only progress counter to those work-item boundaries.
- **Progress is display-only.** Counters (`StageProgress` in `reporter.rs`) feed percent/ETA and never
  flow back into bake output, so pausing/throttling/the TUI cannot perturb the `.prl` — output stays
  byte-identical to a straight-through build (the Determinism invariant under Build Cache).
- **Logging** goes through a `CollectingLogger` (`logger.rs`) installed via `log::set_boxed_logger`,
  preserving `RUST_LOG`/verbose filtering while tallying warn-and-above records into a shared sink the
  active reporter drains. Every build ends with a warning tally (count plus the formatted records),
  printed on the normal screen even for warnings that scrolled out of the live TUI region.

### PRL section IDs

Version fields are exact-match format epochs unless a format explicitly adopts
full semver. Use semver only when the bytes encode major/minor/patch and the
loader has compatibility or migration behavior. PRL and section-internal
versions reject mismatches today, so integer epochs are the honest contract.
Section-internal epochs advance independently when their section payload
changes.

PRL header `version` is 4. Loading a file with any other version fails.

| Section | ID | When present |
|---------|-----|-------------|
| Portals | 15 | Compiler emits every build; runtime treats missing, empty, decode/schema-failed, or polygon-unusable data as no usable portal graph; endpoint/adjacency mismatches are fatal when graph is otherwise usable |
| TextureNames | 16 | Always |
| Geometry | 17 | Always |
| AlphaLights | 18 | Always |
| Bvh | 19 | Always |
| ShVolume | 20 | Retired legacy L2 SH irradiance payload; stale files are rejected by section-internal version |
| LightInfluence | 21 | When compiled with lighting |
| Lightmap | 22 | Always (placeholder atlas when a map has no static lights) |
| ChunkLightList | 23 | Always; per-chunk static-light index lists for specular culling and runtime sdf-light selection |
| AnimatedLightChunks | 24 | When compiled with animated lights |
| AnimatedLightWeightMaps | 25 | When compiled with animated lights; per-texel weight maps for the compose pass |
| LightTags | 26 | When at least one light carries a tag; one space-delimited tag-list string per AlphaLight record (empty string = untagged) |
| DeltaShVolumes | 27 | When the map has at least one animated light; per-light sparse octahedral irradiance delta tiles, with a per-affinity-cell coarsening level |
| DataScript | 28 | When `data_script` KVP present on `worldspawn`; compiled script bytes + original source path |
| MapEntity | 29 | When the map has at least one non-light, non-worldspawn entity; per-entity classname, origin, angles, tags, and KVP bag for runtime classname dispatch |
| FogVolumes | 30 | Always (12-byte overhead when no fog_volume brushes present; carries fog_pixel_scale and initial_gravity) |
| FogCellMasks | 31 | When at least one fog volume entity is present (fog_volume brush, fog_lamp, or fog_tube) |
| TextureCacheKeys | 32 | Always; one 32-byte blake3 per TextureNames entry pointing at a `.prm` sidecar under `baked/materials/` |
| SdfAtlas | 33 | When the map has SDF static occluder data |
| OctahedralShVolume | 34 | When compiled with lighting; base indirect irradiance as layer-aware octahedral atlas tiles |
| DirectShVolume | 35 | When the map has static baked lights; dense baked static-direct layer-aware octahedral irradiance for movers/skinned meshes and legacy billboard fallback; BC6H at rest; no depth moments (read from id 34); same tile geometry and layer assignment as OctahedralShVolume; section-internal `DIRECT_SH_VOLUME_VERSION` |
| NavMesh | 36 | When the map has walkable navigation; baked regions/portals for runtime pathfinding |
| CellDrawIndex | 37 | When the BVH has non-empty leaves (omitted for zero-leaf maps); independent of portal presence. Per-cell CSR of owned BVH-leaf spans driving the runtime candidate cull (`rendering_pipeline.md` §7.1) |
| Cells | 38 | Always; runtime visibility units, preserving compiler spatial ids for cells, portal endpoints, fog masks, BVH leaf `cell_id`, and diagnostics |
| CellLocator | 39 | Always; point-to-cell decision tree used by runtime visibility and object placement diagnostics |
| EntityShadowLights | 40 | When `DirectShVolume` and usable `DirectShDeltaVolumes` are emitted and the compiler selects at least one baked-tier static light for runtime entity-shadow promotion; ascending indices into `AlphaLights` |
| DirectShDeltaVolumes | 41 | Required companion for `EntityShadowLights`; sparse per-selected-light direct octahedral irradiance deltas covering every selection index, with a per-affinity-cell coarsening level |
| ShadowmaskAtlas | 42 | When usable `EntityShadowLights` are emitted; per-selected-light baked world-visibility masks packed into RGBA channels, with `0xFF` channel entries for globally dropped masks |
| KinematicGeometry | 43 | Version 6 when the map has at least one `kinematic_mover`; origin-relative mover vertices/indices/face metadata, static spin/blocking authoring, closed-mover portal associations, carried dynamic-light links, and `kinematic_waypoint` records |
| TriggerVolumes | 44 | When the map has at least one `trigger_volume` or `switch`; trigger AABBs, direct mover commands, and named enter/exit events |
| AnimatedDirectShDeltaVolumes | 45 | When the map has animated baked lights; sparse per-animated-light direct-SH delta tiles composed into the dynamic-receiver atlas, with a per-affinity-cell coarsening level |
| CellVisibility | 46 | Optional, versioned, strictly parsed baked Cell→Cell coupling relation: per-cell reachability component IDs plus canonically ordered coupled-pair distance/aperture graded records. Missing → conservative all-perceivable, no-graded-detail fallback. Id 14 (`LeafPvs`) is a retired hole; do not reuse |
| BillboardDirectScatterVolume | 47 | Optional dense normal-free direct-scatter base for billboards: `Rgba16Float` RGB plus binary validity alpha in the x-fastest id-34 probe order. Static-only maps omit it when no `static_light_map` source contributes. A map with animated-only `static_light_map` scatter entries may emit an all-zero RGB base solely as the required grid/validity anchor for a valid id-48 companion. Invalid or missing data selects legacy billboard direct lighting |
| AnimatedBillboardDirectScatterDeltaVolumes | 48 | Optional dense animated billboard direct-scatter deltas: reuses id-45 descriptor mapping and CSR affinity layout, but stores fixed dense 4×4×4 `Rgba16Float` RGB deltas per CSR entry (reserved zero alpha). Required with id 47 whenever id 45 is present; a missing or invalid pair selects legacy billboard direct lighting |

**Coarsened delta sections (ids 27, 41, 45):** The wire representation supports an independent L0/L1/L2 level for every affinity cell in each section. L0 stores every valid probe tile. L1 stores valid brick-corner tiles. L2 stores one synthesized mean tile over the brick's valid probes. Payload size and order follow kept probes and kept rank, not dense probe index. Production adaptively classifies id 41 only. Ids 27 and 45 intentionally emit uniform L0 until animation and script amplitudes have bounded runtime contracts. Protection AABBs force intersecting bricks to L0 in an adaptively classified section before one-level seam smoothing.

**Delta-section loader floor:** Each id 27, 41, and 45 raw payload must fit the 128 MiB storage-buffer binding floor. Loader checks the on-wire byte length before decoder allocation. An over-floor id 27 or id 45 is independently disabled, so compose falls back to its base contribution. An over-floor id 41 clears id 41 and its paired `EntityShadowLights` selection (id 40), preserving the all-or-nothing promotion contract. This loader safety floor is separate from the compiler's post-compaction 256 MiB aggregate cap across ids 27/41/45. The compiler also reports, but does not reject, payload above the 64 MiB authoring target.

**Lightmap (id 22):** versioned section carrying the baked irradiance and dominant-direction atlases. Irradiance is BC6H (`Bc6hRgbUfloat`) at rest by default — ~8× smaller than the uncompressed `Rgba16Float` debug path, hardware-decoded and -filterable; direction stays `Rgba8Unorm` octahedral on a nearest sampler (octahedral lerp ≠ slerp, so it is never compressed or linearly filtered). Both atlases are `texture_2d_array`: charts that overflow one 8192²-capped layer spill into additional layers rather than failing the bake. BC6H is lossy, so the lightmap stage is exempt from the byte-identical determinism invariant (correctness is round-trip within tolerance; the cache keys on inputs regardless).

Wire layout (format version 2, all little-endian; source of truth `crates/level-format/src/lightmap.rs`):

- **Header (48 bytes):** `u32 version` (= 2; pre-v2 sections — whose first u32 was `width` — are rejected at parse as `InvalidData`), `u32 layer_count` (shared by both atlases, ≥ 1), then the decoupled irradiance and direction descriptors. Irradiance: `u32 irr_width`, `u32 irr_height` (per-layer, pow2 ≥ 4), `f32 irr_texel_density` (m/texel, informational), `u32 irr_format` (0 = Rgba16Float, 1 = Bc6hRgbUfloat), `u32 irr_total_bytes` (all irradiance layers combined). Direction: `u32 dir_width`, `u32 dir_height` (per-layer, pow2 ≥ 4), `f32 dir_texel_density` (informational; may differ from irradiance), `u32 dir_format` (= 0, Rgba8Unorm octahedral — only defined value), `u32 dir_total_bytes` (all direction layers combined). Irradiance and direction dimensions are independent; the two atlases need not match in size (the wire format allows it; the current bake always sets `dir_width = irr_width` and `dir_height = irr_height` — a coarser-direction optimisation can land later without a format-version bump).
- **Irradiance blob (`irr_total_bytes`):** layer-major (layer 0, then layer 1, … `layer_count`−1). Each layer is `irr_width × irr_height` texels — Rgba16Float is `u16×4` per texel row-major (`y·irr_width + x`); Bc6hRgbUfloat is `ceil(w/4)·ceil(h/4)·16` bytes of row-major 4×4 blocks.
- **Direction blob (`dir_total_bytes`):** layer-major. Each layer is `dir_width × dir_height × 4` bytes (Rgba8Unorm octahedral, row-major).
- **Optional LMOD trailer (8 bytes):** at offset `48 + irr_total_bytes + dir_total_bytes`; omitted when `mode = Shadowed`. `u32 magic` (ASCII `"LMOD"`) + `u32 mode` (0 = Shadowed, 1 = Unshadowed). A shadowed bake writes no trailer (byte-identical to `main`); a missing trailer reads as Shadowed. Bytes past the trailer are ignored as forward-compat slack. Single-layer bakes write `layer_count = 1`; the v2 layout is always used.

**OctahedralShVolume (id 34):** sibling replacement for legacy `ShVolume` (id 20), carrying base indirect irradiance. Its dense grid and tile geometry describe the composed atlas sampled at runtime. Separate compact geometry describes the at-rest payload: valid probes only, tagged BC6H or `Rgba16Float`. `SH_VOLUME_VERSION` is section-internal; version 9 is the compact format, and stale pre-migration `.prl` files are rejected rather than silently accepted.

**DirectShVolume (id 35):** baked static-direct octahedral irradiance for entities and billboards. Same tile geometry, probe ordering, shared per-layer atlas dimensions, `atlas_tiles_per_row`, `layer_count`, `tiles_per_layer`, and layer assignment as `OctahedralShVolume` (id 34); carries no depth moments (read from id 34). The irradiance blob is layer-major, matching id 34: all BC6H block bytes or uncompressed `Rgba16Float` texels for layer 0, then layer 1, etc. Stored BC6H at rest. Emitted only when the map has static baked lights. Section-internal `DIRECT_SH_VOLUME_VERSION` is 2 for the layer-aware header. Runtime: sampled by skinned-mesh and billboard shaders, gated by `has_direct`; forward and fog pipelines bind but do not sample it.

**DirectShDeltaVolumes (id 41):** required sparse CSR companion for `EntityShadowLights`. Runtime promotion is all-or-nothing: every selection index in `EntityShadowLights` must appear in `affinity_lights` at least once. Missing, malformed, or partial direct deltas clear both sections at load so a selected light never runs as both direct SH and runtime light. Layout mirrors `DeltaShVolumes` affinity cells (`affinity_factor = 4`, `affinity_dims = ceil(DirectShVolume.grid_dimensions / 4)`, CSR `affinity_offsets`, and flat `affinity_lights`) but carries no animation descriptor mapping. Each affinity cell has its own coarsening level; each CSR entry stores tiles only for that cell's kept probes, by kept rank. L2 stores one synthesized valid-brick mean tile. Production bakes coarsen id 41 by default under the runtime-safe direct envelope; worldspawn `_sh_coarsen "0"` short-circuits classification and preserves uniform L0. Ids 27 and 45 remain uniform L0 until their animation/script amplitudes have a bounded runtime contract. `affinity_lights` entries are selection indices: zero-based positions in the `EntityShadowLights` list, not `AlphaLights` indices. Deltas use the same direct bake math as `DirectShVolume` before BC6H compression and are clipped to each selected light's reach. Compiler-side conservative dropping may omit a bounded-negligible entry through the existing CSR absence encoding, but always retains coverage for every selection index.

**ShadowmaskAtlas (id 42):** optional sibling for `EntityShadowLights`, used by entity→world shadows from promoted static lights. Header is little-endian `u32 width`, `height`, `layer_count`, `selected_light_count`, followed by one channel byte per selected light (`0..3` = RGBA channel, `0xFF` = no mask/dropped). Channel bytes pad to 4-byte alignment. Payload is `layer_count × width × height × 4` bytes of layer-major `Rgba8Unorm` visibility data (`255` = fully visible). Dimensions and layer count match the Lightmap irradiance atlas. Compiler consumes `EntityShadowLights` order; it never reselects from `AlphaLights`. `_bake_only 1` lights have no entries because they are absent from `AlphaLights` and the selected set. Channel assignment graph-colors selected lights that affect the same texel. If the overlap graph is not 4-colorable, masks drop globally from lowest intensity upward until it is colorable.

**KinematicGeometry (id 43):** optional section for deterministic brush movers. Version 5 carries mover identity/path, local collision/render geometry, static spin authoring, block/crush policy, optional auto-close override, named transition events, and per-mover portal associations for movers closed at their docked pose. Version 6 appends per-mover carried dynamic-light links (`AlphaLights` index plus local offset). Both additions are presentation-only: portal associations drive camera portal occlusion, while carried links resolve to local runtime entities; neither enters `CellVisibility`, non-camera coupling, or multiplayer content parity. Versions 1–5 remain loadable; v1–v5 decode with no carried-light links. The auto-close override has an explicit presence marker: absence inherits the mod default, including through a blank KVP; present zero disables it. Version 3 has its historical scalar rule: zero means absent/inherit and non-zero means authored. Version 2 defaults blocking fields and carries spin authoring; version 1 also defaults spin to zero and `carry_yaw = false`. Loader rejects non-zero degree-domain spin values that become zero in runtime radians, so pure-rotator admission and ramp semantics cannot change across the conversion boundary. Waypoint records carry `name`, `next`, and origin. Runtime hashes only deterministic prediction and collision inputs into the multiplayer static-content gate; host-only block policy and timers remain outside it. Mover brushes are not present in static `Geometry`, `Bvh`, static collision, lightmaps, SDF, portals, or navmesh.

**TriggerVolumes (id 44):** optional section for brush triggers. Each record carries a world-space AABB, activation (`touch` or `use`), target tag, closed mover-command vocabulary, fire mode, rearm delay, enabled-on-spawn state, tags, and named `on_fire`/`on_exit` events. Version 2 appends the event names; version 1 decodes both as empty. Runtime preserves direct mover commands and dispatches bound event reactions. `trigger_volume` brushes are not present in static `Geometry`, `Bvh`, static collision, lightmaps, SDF, portals, or navmesh; `switch` brushes are, and their records are otherwise indistinguishable.

**CellDrawIndex (id 37):** per-cell CSR of each cell's owned BVH-leaf spans, baked from the already-sorted global leaf array joined to the compiler cell records (a BVH leaf is drawable when `index_count > 0` and its cell is `!is_solid && face_count > 0`). Layout: `version`, `cell_count` (= `Cells.cell_count`), `span_count`, reserved, `cell_span_offset[cell_count + 1]` prefix sums, then `Span { leaf_start, leaf_count }[span_count]`. A cell touching K material buckets owns K disjoint spans (BVH leaves are bucket-sorted). **Presence rule:** emitted whenever the BVH has non-empty leaves; omitted for zero-leaf maps; independent of portal presence. The runtime cross-validates it at load and drives the candidate cull from it (`rendering_pipeline.md` §7.1); absence when required or any validation failure is a load error. Derived from the BVH stage and baked uncached, like the BVH itself.

**Cells (id 38):** runtime visibility cell records copied from the compiler's final empty/solid/exterior partition. Cells preserve compiler spatial ids one-to-one so portal endpoints, BVH leaf `cell_id`, fog masks, and diagnostics share a stable id space without a runtime remap. Each cell stores bounds, solid/exterior/drawable flags, face range summary, and a range into the flat portal-reference list. BSP split planes are not serialized here.

**CellLocator (id 39):** runtime point-to-cell locator. Version 1 encodes a validated decision tree derived from the compiler BSP, but the runtime contract is locator-to-cell, not BSP traversal. On-plane positions choose the front child. Runtime camera visibility, mesh/particle placement culling, and diagnostics use this locator through `LevelWorld::locate_cell`.

**DeltaShVolumes (id 27):** sparse CSR companion for animated-light indirect deltas. Its affinity-cell structure uses `affinity_factor = 4`, `affinity_dims = ceil(base_dims / 4)`, CSR `affinity_offsets`, and flat `affinity_lights`. Each affinity cell has its own coarsening level; each CSR entry stores octahedral delta tiles only for that cell's kept probes, by kept rank. L2 stores one synthesized valid-brick mean tile. The delta bake stores indirect-only unit-radiance transport; animated direct lighting lives in `lm_anim`, and runtime descriptors apply authored color and intensity exactly once to both compose paths. `DELTA_SH_VOLUMES_VERSION` is section-internal and stale sections are rejected. Delta bakes are invoked directly from the compiler rather than through the build cache. Compiler-side conservative dropping may omit an entry through the existing CSR absence encoding only when its composed output is within the fixed error budget; script-mutable animated entries remain present.

**AnimatedDirectShDeltaVolumes (id 45):** sparse CSR companion for animated direct SH. Each affinity cell has its own coarsening level; each CSR entry stores tiles only for that cell's kept probes, by kept rank. L2 stores one synthesized valid-brick mean tile. Conservative compiler-side dropping may omit bounded-negligible immutable entries, while script-mutable animated entries remain present. An all-empty result is omitted rather than emitted as an empty section.

### Runtime visibility

Portal traversal normally computes visibility: per-frame flood-fill from the camera cell with frustum narrowing at each portal. Solid-cell, exterior-camera, and no-portals cases fall back to per-cell AABB frustum culling. `CollisionWorld` remains the physics source of truth; cells and portals do not answer collision contacts. See `rendering_pipeline.md` §2.

---

## Navigation bake

Walkable navigation is baked offline into a `NavMesh` PRL section (SectionId 36). Baked after the BVH stage and before the lightmap bake; the runtime loader decodes it. Runtime pathfinding traverses regions with A* and follows the resulting portal sequence with funnel string-pulling.

**Query contract.** Runtime consumes convex walkable **regions joined by portals** — the pathfinding query surface (A* over regions, funnel over portal segments). The shape is the durable contract; the bake *algorithm* swaps behind it (rectangular decomposition first, a contour tracer later). Off-mesh links and hints (jump links, cover) extend it as future portal kinds / region attachments — additive, no format break. Seed of a broader baked spatial-AI layer.

**Navmesh ↔ collision.** The bake reads the same triangles the collision trimesh uses, so it never marks walkable what collision rejects. The navmesh **routes** (which regions connect); `CollisionWorld` owns ground height and final movement (agents sweep real collision along a region path).

**Scope.** One graph per map, one canonical agent. The section records the agent parameters it was baked with (radius, height, step, slope), so multi-agent support is an explicit migration, not a silent reinterpretation.

**Agent defaults (worldspawn KVPs).**

| KVP | Default |
|-----|---------|
| `nav_agent_radius` | 0.4 m |
| `nav_agent_height` | 1.8 m |
| `nav_step_height` | 0.5 m (matches the player's `stepHeight`) |
| `nav_max_slope` | 45.0° |
| `nav_cell_size` | 0.25 m |

**Region decomposition.** Each region covers a single floor level. Every climbable step (floor delta ≤ `nav_step_height`) becomes a region boundary joined by a portal rather than being absorbed into one multi-level region. This increases region count (logged at bake time) and is a defensible reading of the "merge ≤ step\_height" rule — the rule bounds what may merge; it does not mandate merging.

## Build Cache

Disk-backed content-hash cache that lets `prl-build` skip cached bake work when inputs are unchanged.

**Disposable stage cache vs. runtime-required compiled output.** Two distinct trees sit at the workspace root, and the distinction matters:

- **`.build-caches/prl-cache/` — disposable stage cache.** Pure bake-time acceleration. Safe to delete at any time; the next build recreates it and nothing at runtime depends on it.
- **`baked/materials/<hash>.prm` — runtime-required compiled output.** The texture mip sidecars the *engine reads at level load* (see §Baked texture mips). Deleting them does not break the build, but it does break the runtime until the next bake repopulates `baked/materials/`. They live in the top-level `baked/` tree — **not** in `.build-caches/` — precisely so that "delete the disposable cache" (`rm -rf .build-caches/`) never strips runtime-required output. Both trees are dev-local and regenerable (gitignored); only the disposable one is described in the rest of this section.

**Location.** `.build-caches/prl-cache/` at the workspace root (the parent directory containing `Cargo.toml`). Created automatically on first build. Safe to delete at any time — the next build recreates it.

**Participating stages.** Lightmap bake and SH volume bake, plus the ShadowmaskAtlas memo, static and animated billboard direct-scatter, animated-light weight-map, SDF-atlas, and navmesh stages. Parse, BSP, portals, geometry, and BVH run uncached — they are fast enough that caching yields no measurable speedup.

**Cache grain (lightmap + SH).** These two channels are cached *per element*, not per whole stage, so editing one light refreshes only the affected entries:

- **Lightmap — per-light layers.** Each static light's contribution (linear irradiance + unnormalized weighted direction + coverage + raw soft visibility, full-precision) is a separate `"lightmap_layer"` entry, keyed on that light's params, its influence-bounded geometry slice, density/sample-count, and the atlas layout. The raw soft-visibility payload is also the ShadowmaskAtlas raw-mask source. `LAYER_FORMAT_VERSION` covers the full payload. The compositor sums the layers (in global light order) and normalizes once, reproducing the monolithic `bake_face_chart` byte-for-byte (pre-BC6H). Exact in both warm and cold builds.
- **Lightmap — composited section (second level).** A `"lightmap_section"` entry memoizes the encoded `Lightmap` (id 22) section itself, keyed on the ordered per-light layer fingerprints plus the encode parameters (texel density, irradiance format). A no-edit rebuild hits it and skips reading the layers, compositing, and BC6H-encoding entirely — the per-light layers are the recompose fallback when any light, geometry, or atlas input changes. Pure memoization of an already-exact pipeline output, so it cannot perturb byte-identity; warm-only, like the layers.
- **ShadowmaskAtlas — selected-light section (second level).** A `"shadowmask_atlas"` entry memoizes the encoded `ShadowmaskAtlas` (id 42) section. Key includes selected `EntityShadowLights` order, selected `"lightmap_layer"` fingerprints, atlas dimensions, and layer count. A no-edit rebuild hits it and skips selected layer reads, channel assignment, quantization, and section encoding. On a miss, compiler reuses existing `"lightmap_layer"` entries as the raw-visibility source and bakes only missing/corrupt selected layers. Warm-only, like the lightmap section memo. Cold `--no-cache`/`--release` recomputes through the uncached shadowmask path.
- **SH — per-probe-group entries.** The probe grid is partitioned into 4³-probe groups; each is a `"sh_group"` entry baked over its probe subset with a *bounded reaching-light set* (`falloff_range` dilated by a finite reach cutoff), then assembled (byte-copy placement) into the volume. Bounding the light set is what localizes a light edit; it also makes warm SH a benign approximation (out-of-reach lights drop — dimmer-or-equal, never miscolored). The soft-visibility sample-lattice seed mixes each light's **global** `static_lights` index (not its position in the bounded slice), so a kept light gets the same rotation whether the bake sees the full set (cold) or the bounded set (warm) — that is what makes "dimmer-or-equal, never brighter" hold strictly. The cold `--no-cache` path runs the exact whole-volume bake instead. SH rays trace full geometry, so any geometry edit re-bakes every group.

**Warm vs cold builds (dev-default / release-on-purpose).** The interactive default is a *warm* (cached) build: fast iteration, exact direct lightmap, approximate indirect SH. The `--release` flag selects the *cold* exact build — every stage baked exact — and is the only artifact a final map should ship from. `--release` is the intent-named ship mode; mechanically it bypasses the cache exactly like `--no-cache` (it implies `--no-cache`; passing both is fine and identical). A warm build trades exactness for speed and is not shippable. The split is per channel. The direct lightmap is exact in both modes: a cached lightmap is byte-identical to a full bake (pre-compression). Indirect SH is exact only in a release/cold build. A warm build bakes SH at a finer-than-whole-volume grain, bounding each region's light set — a benign approximation, dimmer-or-equal in far-bounce regions, never miscolored. A warm build emits a one-line warning naming `--release` as the ship flag. Judge final indirect lighting on a release build. Run production and release bakes with `--release` (or `--no-cache`).

**Key composition.** `blake3(stage_id || stage_version_le_bytes || input_hash)`.

| Component | Form |
|-----------|------|
| `stage_id` | string literal — `"lightmap_layer"` (per-light), `"lightmap_section"` (composited-section memo), `"shadowmask_atlas"` (selected-light section memo), `"sh_group"` (per-probe-group), `"billboard_direct_scatter"` (static scatter), `"animated_billboard_direct_scatter"` (animated scatter deltas), `"animated_lm_weight_maps"`, `"sdf_atlas"`, or `"navmesh"` |
| `stage_version` | `u32` cache epoch in each stage's module, bumped manually when that stage's algorithm or payload format changes. Each stage owns its own epoch and version-bumps independently — the per-light-layer and per-group-SH formats version separately from each other and from the legacy whole-stage bakes |
| `input_hash` | Stage-defined blake3 over serialized inputs/config — covers the data the stage reads. Postcard serialization is the common pattern; stages may use a fixed-order byte stream when that is the stage contract. |

**Stage version bump rule.** Bump a stage's epoch when its output computation changes (algorithm, sampling, formula, or atlas packing). The substrate invalidates every entry for that stage on the next build. Do not bump for unrelated changes. Each stage's current epoch lives as a `u32` constant in its own module — the source is authoritative; this doc does not pin the number.

**Determinism invariant.** Byte-identical output for identical inputs — with two scoped carve-outs. The guarantee holds for the direct lightmap before compression and for the cold whole-volume SH bake (the ship path). New code in `lightmap_bake.rs` or `sh_bake.rs` must preserve it. Avoid common non-determinism sources: `HashMap` iteration feeding output ordering, non-order-preserving parallel reductions. **Exempt:** (1) lossy compressed output (BC6H irradiance) — correctness is round-trip within tolerance, not byte-equality; (2) indirect SH baked finer than the whole volume (warm incremental builds) — a deliberate bounded approximation; the cold whole-volume bake stays exact. Either way the cache stays correct: it keys on inputs, not outputs. Every bake is self-consistent — same inputs, same bytes.

**CLI flags.**

| Flag | Effect |
|------|--------|
| `--cache-dir <PATH>` | Use a custom cache directory instead of `.build-caches/prl-cache/` at the workspace root |
| `--cache-max-size <SIZE>` | LRU size budget for the cache, swept at build start (default 2 GiB). Accepts a byte count or a binary-unit suffix (`2GiB`, `512MiB`, `1.5GiB`) |
| `--sh-delta-max-size <SIZE>` | Post-drop raw-payload cap for sparse SH delta sections ids 27, 41, and 45 (default 256 MiB). It accepts the same byte syntax as `--cache-max-size`; an overage is a named compiler error before packing, never an automatic quality reduction. **64 MiB is an authoring diagnostic target: the compiler emits a non-failing post-compaction warning above it, with per-section and aggregate bytes; it is not the default cap or a rejection threshold.** Compiler-only: no map KVP or runtime representation. |
| `--no-cache` | Disable the cache entirely — neither read nor write, no directory created (no prune either) |
| `--release` | Produce a shippable map: the exact ship path (exact monolithic lightmap + exact whole-volume SH). Intent-named ship mode; implies `--no-cache` (passing both is fine and identical). The interactive default is a warm build — ship only `--release` artifacts. |
| `--soft-shadow-samples <N>` | Soft-shadow penumbra escalated full-sample count (default 32). Folds into the lightmap-layer cache key and, for selected lights, the ShadowmaskAtlas memo key. Raising it invalidates those cache entries. The uncached animated weight-map stage recomputes from scratch. Run `prl-build --help` for the full flag list. |

**Entry format.** One file per entry, named by the hex key. `get()` validates integrity before returning payload; mismatch is a soft failure (warning, cache miss).

**Eviction.** LRU size cap, enforced by a sweep at the start of every cached build (before the bake writes a fresh generation). When the directory exceeds the budget (`--cache-max-size`, default 2 GiB), the least-recently-used entries are deleted oldest-first until the total fits. Recency is the entry's mtime: `get` bumps it on every hit and `put` sets it on write, so a long-stable entry (hit every build, never rewritten) stays warm while orphaned generations — the tail content addressing leaves behind whenever an input changes — age out and get reclaimed. The sweep is off the bake path (one directory listing plus a few unlinks) and best-effort: any I/O error is logged and the build proceeds. In-flight `*.tmp` stage files are never touched. `--no-cache`/`--release` skip the cache (and the sweep) entirely. A corrupted entry is still discarded as a cache miss without touching other entries. The cache remains safe to delete manually at any time.

---

## Baked texture mips

Per-texture mip-chain sidecars are **runtime-required compiled output** living under the top-level `baked/materials/` tree — not in the disposable `.build-caches/` stage cache (see §Build Cache). prl-build writes them; the engine reads them at level load.

**`.prm` files.** Each sidecar bundles up to four material slots — diffuse,
specular, normal, and emissive — each optional. Multi-slot filenames use the
canonical bundle hash over slot mask and every present slot's raw PNG bytes.
A diffuse-only bundle uses `blake3(diffuse PNG content)` so world and model
diffuse-only materials share one sidecar. Other single-slot bundles use
`blake3(tag_byte || PNG content)`; the tag distinguishes specular, normal, and
emissive. Two bundles with the same diffuse but different siblings therefore
have distinct runtime-loadable filenames. Stored at
`<workspace>/baked/materials/<hex>.prm`. Cross-mod dedupe is intended:
identical complete bundles produce the same `.prm` regardless of which mod
authored them.

**Wire format.** `.prm` v3 (`PRM\x02`) has a fixed 45-byte header, followed by
present slot blocks in diffuse → specular → normal → emissive order. The header
contains a file-level `layer_count: u16`; it is a bundle property shared by every
present slot, never a per-slot count. Each slot payload is layer-major: the complete
mip chain for layer 0, then the complete chain for layer 1, and so on. The wire
layout lives in `postretro-level-format::prm`. `.prm` uses a `u8` exact-match format
epoch (not the stage-cache `u32` convention) — the header owns its own version
semantics.

World-material and model sidecars remain single-layer (`layer_count = 1`) and use
their existing 2D runtime upload path. Writers of array-backed sidecars must cap
`layer_count` at 256, the portable WebGPU/wgpu `max_texture_array_layers` baseline;
the shared writer rejects larger values. The reader accepts structurally valid larger
counts so tools and future device-specific consumers can inspect them. Any upload path
must still validate against its active adapter before allocation. The current renderer
does this for decoded-PNG sprite collection frame counts before creating its `D2Array`;
parsed array-backed `.prm` upload is downstream work and has no renderer path yet.

**Filtering.** Mitchell-Netravali separable filter (B = C = 1/3) in linear
space throughout. sRGB diffuse and emissive color decode via a 256-entry LUT
before filtering and re-encode via IEC 61966-2-1. Specular filters as linear
R8. Normal filters linearly then renormalises per output texel; `(0, 0, 1)`
substitutes when magnitude < 1e-4. Output is then BC5-encoded (RG channels
only; the shader reconstructs Z).

**Cache invalidation.** Multi-slot filenames cover the complete bundle; the header repeats that bundle hash for cache validation. A world-material cache hit requires a matching bundle hash and structurally valid payload for every declared slot. Truncated or corrupt declared slots trigger a full rebake and atomic overwrite (tempfile `<hex>.prm.tmp.<pid>` → `std::fs::rename`). Diffuse-only model and world bundles retain their shared diffuse-content filename without colliding with richer world bundles. A version mismatch in the header triggers rebake. To force a full retexture rebuild, delete `baked/materials/` (the next bake repopulates it; doing so leaves the runtime without world-material mips until then).

**Runtime.** Level load resolves each `TextureNamesSection` entry's blake3 key from `TextureCacheKeysSection`, opens the corresponding `.prm`, and uploads each slot's mip chain directly. A zero key (`[0u8; 32]`) substitutes per-slot placeholders silently. A corrupt or missing `.prm` substitutes per-slot placeholders and logs a `warn!`; load continues. Sampler `lod_max_clamp` is set to `mip_count - 1` per texture.

**Model textures.** `prop_mesh` model base-color textures bake the same way, content-driven from the model placements in the map — no CLI flag, mirroring how world materials follow from `TextureNames`. prl-build resolves each placed model's glTF base-color PNG(s) and bakes a diffuse-only `.prm`, content-addressed by `blake3(base-color PNG)` — byte-identical to a diffuse-only world sidecar. Richer world bundles use complete-bundle filenames and cannot be replaced by a model bake. Model rendering still consumes only the diffuse slot and substitutes neutral specular and normal placeholders. Unlike world materials, no PRL section carries model keys: the runtime content-hashes the same PNG when it loads the glTF and opens `<key>.prm` directly, so the compiler only has to make the sidecar exist. The glTF base-color path resolver is shared by runtime and compiler through the `gltf-resolve` feature of `postretro-level-format`. Missing or malformed glTF fails the whole model load, so the model is skipped. Only an unresolved, missing, or unreadable base-color PNG or material degrades to the texture placeholder. Compiler resolution and bake failures warn; compilation continues. For standalone model prep, `cargo run -p xtask -- bake-model-textures <scene.gltf>` runs the same model-texture sidecar bake without compiling a map. Output stays under `<workspace>/baked/materials/`: gitignored, regenerable, runtime-required.

---

## Non-Goals

- Runtime level compilation
- Format plugin registry — adding an input format is a code change in `format/`, not a registration surface
- WAD file support
- Runtime lightmap baking
