# Build Pipeline

> **Read this when:** setting up the map authoring toolchain, modifying the asset pipeline, adding custom entities, debugging map compilation issues, or packaging a build for someone else's machine (§Distribution packaging).
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

Source layout is free-form within a line's grammar. shalrath separates items by exactly one line ending, so before parsing, `crates/level-compiler/src/parse/map_source.rs` turns CRLF into LF and, outside quoted values, drops blank, whitespace-only and `//` comment lines and trims indentation. The parse must then consume the whole file: an entity shalrath cannot parse is an error naming the source line where parsing stopped, never a silently shortened entity list.

**Entity angles.** One converter, `quake_to_engine_angles` in the Quake adapter, turns a point entity's `angles` or `mangle` (pitch yaw roll, degrees; either wins over `angle`) or legacy `angle` (yaw only; `-1` up, `-2` down) into engine Euler radians. No component changes sign: the axis map (x, y, z) → (−y, z, −x) is a proper rotation, so map yaw stays the TrenchBroom arrow (`entity_model.md` §4, Facing convention). A malformed value warns and reads as zero, except on a `teleport_destination`, where it fails the build. Light direction keeps its own spotlight convention.

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
| Author | Create PNGs in `<mod root>/textures/<collection>/<name>.png` — `content/dev` in this workspace, and `content/<mod>` for the mod named by `postretro.toml` in a packaged distribution. TrenchBroom requires one subdirectory level. |
| TrenchBroom | Browses the textures directory via the Postretro game config. |
| prl-build | Reads PNGs, decodes them, runs Mitchell-Netravali downsampling in linear color space, and writes per-texture `.prm` mip sidecars to `<baked root>/materials/<blake3-hex>.prm`. Stores a content-addressed blake3 key per texture in the `TextureCacheKeys` PRL section. Authored PNGs are not shipped or read at runtime for world materials. |
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
| `lightmap_scale_region` | brush | Compiler-only per-surface lightmap density override; excluded from world geometry, collision, and runtime entities. A chart whose first face vertex lies in the region AABB uses `global_density / _lightmap_scale`; overlapping regions resolve last-defined wins in map entity order. | `_lightmap_scale` (finite positive float, default 1.0; below 1 is coarser, above 1 is finer) |
| `sh_protect_volume` | brush | Forces intersecting SH base-storage bricks and coarsened delta-section bricks to L0; excluded from static world geometry, collision, and runtime entities | `dilation` (world/engine-meter AABB expansion; default 0; negative rejected) |
| `streaming_seam_volume` | brush | Compiler-only portal cut and preferred far-side SH warm-up; never blocks a door or changes visibility | Exactly one convex brush spanning the intended portal plane; must match at least one portal with positive-area intersection and interior on both sides |
| `stream_resident_volume` | brush | Compiler-only pin of SH clusters intersecting its region, including their baked owner closure | Exactly one convex brush; must overlap at least one runtime cell by positive AABB volume |
| `stream_priority_region` | brush | Compiler-only rank for optional SH prefetch and seam warm-up under pressure | Exactly one convex brush; `_stream_priority` integer 0–3 (blank defaults to 0); must overlap at least one runtime cell |
| `fog_lamp` | point | Spherical halo fog emitter; default warm amber | `density`, `glow`, `radius` (sphere radius; sizes AABB), `radial_falloff`, `tint`, `saturation`, `min_brightness`, `light_range`, `scatter_bias`, `ambient_scatter`, `_tags` (ambient color is SH-derived; no `color` KVP) |
| `fog_tube` | point | Capsule-strip fog emitter; default cool blue-white | `density`, `glow`, `radius` (capsule radius), `height` (capsule length), `pitch` / `yaw` (capsule axis), `radial_falloff`, `tint`, `saturation`, `min_brightness`, `light_range`, `scatter_bias`, `ambient_scatter`, `_tags` (ambient color is SH-derived; no `color` KVP) |
| `billboard_emitter` | point | Billboard particle emitter | `rate` (particles/sec; default 6), `lifetime` (seconds; default 3), `spread` (cone half-angle radians; default 0.4), `buoyancy` (-1=falls, 0=floats, >0=rises; default 0.2), `drag` (velocity damping/sec; default 0.8), `sprite` (collection name; default "smoke"), `initial_velocity_x/y/z` (default 0/0.8/0), `color_r/g/b` (linear; default 1/1/1), `spin_rate` (radians/sec; default 0) |
| `prop_mesh` | point | Map-placed skinned-model entity | `model` (content-relative glTF path; required — absent or unresolvable logs a warning, load continues) |
| `env_reverb_zone` | brush | Acoustic zone | `reverb_type`, `decay_time`, `occlusion_factor` |
| `kinematic_mover` | brush | Runtime-moving brush payload, excluded from static BSP/BVH/lightmap/collision | `name`, `path` (first waypoint name), `move_mode` (`once`/`ping_pong`), `speed`, `wait_ms`, `start_on_spawn`, spin/carry fields, `block_policy` (`displace`/`reverse`/`stop`/`crush`), `crush_damage`, `crush_interval_ms`, `auto_close_ms` (blank inherits mod default; 0 disables), `open_event`, `close_event`, `blocked_event`, `crush_event`, `open_sound`, `close_sound`, `blocked_sound`, `crush_sound` (blank reads as absent), `_tags` |
| `kinematic_waypoint` | point | Waypoint in a mover path | `name`, `next` (optional next waypoint name), `origin` |
| `trigger_volume` | brush | Invisible AABB; touch/use activation runs a direct mover command and named enter/paired-exit reactions; excluded from static BSP/BVH/collision/lightmap/SDF/portals/navmesh | `activation` (`touch`/`use`), `target_tag`, `on_fire`, `on_exit`, `command` (`start`/`stop`/`reverse`/`go_to_path_node`), `command_arg` (required node name for go-to), `fire_mode` (`once`/`multiple`), `rearm_ms`, `enabled_on_spawn`, `_tags`. Blank optional KVPs (`activation`, `command`, `fire_mode`, `rearm_ms`, `enabled_on_spawn`) fall back to their declared defaults — a field cleared in TrenchBroom is unset, not a compile error |
| `switch` | brush | Visible, solid, pressable brush; compiles as static world geometry **and** a `use`-activation trigger volume whose faces grow toward `use_reach`, each clamped to the free space the compiler can prove is in front of it; the geometry never moves. Exactly one brush per switch — zero brushes or more than one is a compile error | `name`, `use_reach` (per-face press margin, map units; default 24 ≈ 0.61 m; must exceed the compiler's flush-tolerance floor, a few hundredths of a map unit, and be ≤ 128 — outside that range is a compile error), `target_tag`, `on_fire`, `on_exit`, `command` (`start`/`stop`/`reverse`/`go_to_path_node`), `command_arg`, `fire_mode` (`once`/`multiple`), `rearm_ms`, `enabled_on_spawn`, `_tags`, `_reflection_capture` (as on `func_group`; not built); blanks fall back to defaults as for `trigger_volume`, `use_reach` included. No `activation` — always `use`; an authored one warns and is discarded |
| `func_group` | brush | Authored brush entity, no `_tb_type`: static world geometry like worldspawn brushwork. TrenchBroom groups and layers write the same classname with `_tb_type` and keep only their name, id, link id and transform | `_reflection_capture` (`auto` default / `always` / `never`; reflection-probe capture override, `resource_management.md` §5.2; not built) |
| `fluid_volume` | brush | Swimmable fluid region; non-solid, excluded from static BSP/BVH/collision/lightmap/SDF/portals/navmesh. Its exposed faces draw as the fluid surface. Make a pool one entity with several brushes: two `fluid_volume` entities draw a surface between them | `fluid` (required; names a fluid the mod manifest declares, `scripting.md` §2), `priority` (integer, default 0) |
| `gravity_volume` | brush | Regional gravity; non-solid, excluded as above | `gravity` (required; one signed vertical number in m/s², like `initialGravity`: `"-3"` low, `"0"` weightless, positive pulls up), `priority` |
| `push_volume` | brush | Regional push — updraft, wind or current; non-solid, excluded as above. A current is a `fluid_volume` with a coincident `push_volume` | `push` (required; three numbers, an acceleration in map axes in m/s²: `"0 0 25"` updraft, `"8 0 0"` wind; the nominal current, which particles and the fluid surface show), `force` (optional, default 1; any finite number ≥ 0, no upper cap; bodies feel `push × force`, so `"0"` is a decorative current), `priority` |
| `trigger_teleport` | brush | Teleporter region: a player whose body point crosses in is relocated (`movement.md` §6, Teleport); non-solid, excluded as above. Several convex brushes per entity read as their union | `name`, `mode` (`classic` default, or `seamless`), `target` (a `teleport_destination` `name`), `landmark` (seamless only; a `teleport_destination` in the source space), `enabled_on_spawn` (0/1, default 1), `arrival_sound` (key under `sounds/`, no extension; default none), `on_teleport`, `_tags` |
| `teleport_destination` | point | Compiler-only arrival or landmark point, baked into the teleporter records that name it; no runtime entity | `name` (unique), `angles` (or `angle`/`mangle`; yaw only counts, pitch or roll warns) |
| `reflection_volume` | brush | Authored reflection-probe projection box and influence region; compiler-only, excluded from static world geometry, collision and runtime entities. Up-facing fluid faces bind the smallest such box containing their area centroid instead of their compiler-derived probe (`resource_management.md` §5) | `_capture_offset` (optional; two numbers on the map's horizontal axes moving the capture point; a third (vertical) number, any other malformed value, or an offset leaving the box is a build error naming the entity) |
| `worldspawn` | special | Scene-wide load-time settings | `data_script` (path to entry `.ts`, `.js`, or `.luau` data script, relative to `.map`; TypeScript and JavaScript compile via `scripts-build`, Luau passes through; its compile-time evaluation emits the compiler-only light-membership manifest; absent = no data script), `ambient_color` (RGB ambient floor), `fog_pixel_scale` (volumetric pass resolution divisor; default 4, range 1–8), `_lightmap_density` (lightmap bake density, meters per texel; default 0.04; finer = higher resolution; `--lightmap-density` CLI overrides; non-finite/≤0 warns and falls back to default), `_sh_coarsen` (`"0"` selects the uniform-L0 id-41 fallback; absent or any other value keeps direct-delta coarsening enabled), `entity_shadow_min_intensity_ratio` (static-light entity-shadow selection intensity floor as ratio of map max; default 0.5), `entity_shadow_min_range` (static-light entity-shadow selection falloff floor in meters; default 4.0), `initialGravity` (level default gravity in m/s², the gravity outside every gravity volume; negative = downward; default -20.32, Quake's 800 u/s² at the map scale; malformed/non-finite supplied values are compile errors; stored in the EnvironmentVolumes header) |

**Load-time gameplay declarations.** Maps may seed level-wide gameplay values at load for accessible non-script authoring. They cannot mutate gameplay values after load or override per-archetype descriptor tuning. `initialGravity` is a level declaration; player movement tuning remains descriptor-owned. Static regional gravity, push and fluid volumes are level declarations too, baked at compile time; a further regional field needs its own amendment. `force` on `push_volume` is one such amendment: a modifier that rides its volume's push and wins with it, never a field resolved on its own. `trigger_teleport` is another: its region is baked, but `enabled_on_spawn` seeds only the initial armed state, which scripts change after load. No FGD key names a `movement.swim` field: swim tuning stays descriptor-owned.

**Mover authoring split.** KVPs define basic deterministic kinematic motion. Brush tags bind movers to triggers and reactions for richer event-driven behavior.

### Entity resolution

- **`light`, `light_spot`, `light_sun`** (baked tier) and **`light_dynamic`, `light_dynamic_spot`** (dynamic tier) — all validated at compile time (falloff distance required, spotlight direction verified, intensity bounds checked). Baked-tier lights feed the SH irradiance volume baker and the directional lightmap baker; dynamic-tier lights bake into nothing and feed the runtime direct lighting buffer + shadow-map pool. The tier is set by classname, not a KVP. Compilation fails on validation errors.
- **`fog_volume`** — resolved at load time to world-space AABBs, shape, and fog parameters. Uploaded as a compact storage buffer (up to 16 entries). Per-sample test: shape membership (AABB as conservative bound), then optional half-space clip plane (normal points into the removed region). No BSP traversal at runtime.
- **`lightmap_scale_region`** — compiler resolves the brush hull to an ordered world-space AABB plus bounding planes and retains it only through lightmap chart planning. The first chart vertex (`p0`) decides AABB membership; source entity order provides deterministic last-defined-wins precedence. It emits no PRL payload and never enters the static world brush set.
- **`sh_protect_volume`** — compiler resolves the brush hull to a world/engine-space AABB, expands all faces by `dilation` meters, then forces every intersecting 4×4×4 SH base-storage brick and coarsened delta-section brick to L0 before seam smoothing. It emits no runtime entity or PRL section.
- **SH streaming hint brushes** — `streaming_seam_volume`, `stream_resident_volume`, and `stream_priority_region` are peeled from world geometry before BSP. After portal/cell construction, seams resolve against convex portal polygons and split the canonical cluster partition; resident and priority regions resolve against runtime cell AABBs. Resident and priority edits change only id-49 hint records. A seam edit also changes the partition, so it reorders id-50 cluster payloads and the cluster-major lightmap blocks (ids 17, 22, 25, 42; see §Build Cache). No hint changes id 51. They emit no runtime `MapEntity`.
- **`kinematic_mover`** — compiler normalizes a finite local `spin_axis`, validates finite `spin_speed` and finite non-negative `spin_accel`, then packs the static rotation authoring with the mover. Authored non-zero speed and positive acceleration must remain non-zero after conversion from degrees to runtime radians. A non-zero spin requires a non-zero axis and may use one waypoint as a pure rotator; zero-spin movers still require at least two waypoints.
- **`trigger_volume`** — compiler extracts each brush as a world-space AABB with activation, direct-command, and named enter/exit event data. Runtime preserves direct mover commands and dispatches bound reactions; the brush has no static geometry or collision role. Optional KVPs resolve through one shared path with `switch`: a blank value is unset and takes the key's declared default, never a parse or unknown-value error.
- **`fluid_volume`, `gravity_volume`, `push_volume`** — the Quake adapter translates each into one canonical environment-volume record carrying only its field, so no later stage sees the authoring split. Overlapping volumes combine per field (`entity_model.md` §7, Environment Volumes): for each field the winner among volumes that set it is the highest `priority`, then the smallest summed brush volume, compared at 8 significant bits so near-equal sizes usually tie, then the earliest map entity. Each brush becomes one convex leaf (planes plus bounds), and the record keeps its source map entity index so every diagnostic names `entity N '<classname>'`. Push converts to engine axes by swizzle only, since it is an acceleration in m/s², not a length. `force` is read only on a push volume; absent or blank is 1, and `-0` reads as 0 so both spellings write the same bits. Build errors, each naming the entity: a blank or missing required field; a `gravity` that is not exactly one number; a `push` that is not exactly three numbers; a `force` that is negative, non-finite or not a number; a non-integer `priority`; no brushes; a brush with no usable vertices, zero volume, fewer than four bounding planes, or a hull past engine precision. The brushes are peeled before BSP like `trigger_volume`.
- **`switch`** — compile-time sugar over two shipped mechanisms; no runtime type, no PRL addition. The brush folds into the static world brush set (the switch renders, takes baked light, and collides like worldspawn brushwork) *and* resolves to a trigger volume with `activation` forced to `use`, whose AABB then grows toward `use_reach` on every face. Growth is part of the mechanism: use-activation is a capsule/AABB intersection test and the brush is solid, so the volume must reach past the switch face into the space the player occupies. The press test measures from the capsule axis, not its surface, so a face's effective reach is its clamped margin plus the player capsule's radius. That radius is an authored descriptor field with no engine default, so effective reach moves with the content: at the repo's only authored capsule (0.2 m ≈ 7.9 map units) and the default `use_reach`, about 31.9 map units. Growing blindly reaches *through* a wall, so each face's allowance is clamped to the occluders standing past it. The invariant: **no grown face extends past the near side of any occluder the compiler could not rule out of the corridor that face grows into.** A hull qualifies as an occluder for one face on three conditions — its AABB overlaps that face's cross-section by positive area; its AABB stands past the face along the growth axis by more than the flush tolerance; and no single one of its own brush planes separates the *growth prism*, the cross-section extended along the growth axis to the full margin. Edge-on contact is deliberately not overlap: a flush mount zeroes the face it backs and leaves the four faces it touches edge-on fronting open room, so flush mounting is a zero gap rather than a special case, and any gap width clamps the same way. An occluder that covers the cross-section and extends past both faces on an axis embeds the brush; both those faces get zero. The plane test earns its cost — on the AABB alone, a diagonal partition, wedge, ramp, or chamfer far from the switch could straddle it on every axis and zero all six faces against geometry that was never in front of them. It is a *separating-plane* test: a plane putting the prism's interior-nearest corner outside proves no intersection, but surviving every plane does not prove intersection. The result is therefore conservative, not exact — it can keep an occluder a finer test would drop, which costs reach rather than leaking it. Partial cross-section overlap clamps as readily as full overlap, and that is correct: the trigger is an AABB, so growing *part* of a face is not representable, and a console sunk one unit into its mount cannot grow horizontally at all. Occluders are the static world hulls plus `kinematic_mover` hulls, minus the switch's own brush, movers taken at their **authored (compile-time)** position. When no *horizontal* face grew (engine space is Y-up, so a standing player reaches along X and Z), the compiler warns with the switch's name and position and names which horizontal faces were clamped and against what. The wording stays at what the compiler concluded — a conservative test cannot claim the switch is walled in on every side. A flush wall mount zeroes exactly one horizontal face and stays silent. Known gaps in the invariant: a mover authored *clear* of the switch that later moves *into* the corridor — a blast door authored open and closed by this very switch — is no occluder at compile time, so the face grows fully and at runtime the volume sits behind a closed solid (the opposite direction, authored across the corridor and later moving away, only costs reach); clamping uses the *ungrown* cross-section one axis at a time, leaving diagonally adjacent geometry that only intersects the grown box's corner unaccounted for; and a brush with no usable vertices has an empty AABB and clamps nothing. Exactly one brush per switch — a brushless switch and a multi-brush switch are both compile errors, the latter because the press volume is the union AABB and spans the room between two consoles. Because a switch *is* a trigger volume at the component layer, `trigger_volume` component queries also return switches.
- **`billboard_emitter`** — resolved at level load via the built-in classname dispatch table. The engine spawns an ECS entity with a `BillboardEmitterComponent` configured from the map's KVPs. See §Built-in classname routing below.
- **`prop_mesh`** — resolved at level load via the built-in classname dispatch table. The engine spawns a `Transform` + `MeshComponent { model }` entity at `entity.origin`; the renderer loads and uploads the model into its handle→model cache once per distinct path. See §Built-in classname routing below.
- **`trigger_teleport`, `teleport_destination`** — the compiler resolves each teleporter's `target` and `landmark` to destination poses baked into its id-55 record; a destination emits nothing else. Each teleporter also writes an id-29 map-entity companion keyed by its source entity, carrying name and tags for runtime install and script queries. Build errors, each naming the entity: an unknown `target` or `landmark`, a duplicate destination `name`, seamless without `landmark`, classic with one, `landmark` equal to `target`, an unknown `mode`, `enabled_on_spawn` other than 0/1, malformed destination angles, a destination with brushes, a teleporter with no brushes, and more than 65,535 teleporters. Only destination yaw counts; a pitch or roll warns. Brushes are peeled before BSP like `trigger_volume`. Overlap ranking compares exact summed brush volumes, then map order (`entity_model.md` §7, Teleporter regions).
- **`func_group`** — flattened into the static world brush set. On an authored `func_group` or a `switch`, `_reflection_capture` resolves at parse to a canonical per-brush capture override, so shared stages never see the key; on a TrenchBroom group or layer it warns once per entity and is ignored, since TrenchBroom drops it on save. An authored `func_group` needs no `_tb_id`. Not built.
- **`reflection_volume`** — peeled off at parse, outside the environment-volume classnames so id 52 is unchanged; its AABB reaches only the reflection-probe capture stage, so adding one leaves geometry, BVH and SH sections byte-identical. Binding, drop and cap rules: `resource_management.md` §5.
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
parse .map → extract kinematic, trigger and environment-volume brush entities → BSP construction → brush-side projection → buried-light classification → scripts-build emits light-membership manifest → prl-build applies membership → portal generation → exterior leaf culling → geometry → BVH → SH and light-selection bakes → atlas preparation → cell residency set → fused lightmap/shadowmask bake → reflection-probe capture → pack .prl
```

1. **Parse.** Extracts brush volumes, brush sides, and entities. Applies coordinate transform (Quake Z-up → engine Y-up) and unit scale. Light entities route to FGD translation and validation; they don't participate in BSP construction. TrenchBroom `func_group` editor groups are flattened into the static world brush set at the Quake adapter boundary so grouped brushes compile like worldspawn brushes. `kinematic_mover` brush entities are peeled off before static world construction and packed as origin-relative geometry plus waypoint records. `trigger_volume` brushes are peeled off as invisible AABBs and packed with activation, direct-command, and named enter/exit event data. `fluid_volume`, `gravity_volume` and `push_volume` brushes are peeled off as convex leaves for the EnvironmentVolumes section; fluid brushes keep their textured sides for the surface stage. `trigger_teleport` brushes are peeled off as convex leaves for the Teleporters section. `reflection_volume` brushes are peeled off as AABBs for reflection-probe capture. `lightmap_scale_region` brushes are peeled off as ordered world AABBs plus source-hull planes so chart planning can apply their per-surface density override; they produce no runtime payload. `sh_protect_volume` brushes are peeled off as world/engine-space AABBs for SH base-storage and delta coarsening classifiers; `dilation` expands them in meters and they produce no runtime payload. The three SH streaming hint brushes are also peeled off here and retained only until post-portal cluster resolution. `switch` brushes go both ways: they join the static world brush set *and* are peeled off as a `use` trigger AABB at the raw brush hull. Their `use_reach` margins land in a later pass, after the static world brush set and the mover brush set are both complete — how far a face may grow depends on the hulls the compiler cannot rule out of the corridor in front of it, tested AABB-first and then against each candidate brush's own planes, and that question cannot be answered while the entity sweep is still collecting them.
2. **BSP construction.** Partitions world space into solid and empty leaves using brush-derived planes. Leaf solidity is established during construction from the brush half-space intersection — not inferred from face positions afterward. Classification is exact for arbitrary convex brushes: leaf regions are carried as convex polytopes (not AABB approximations), and a leaf is solid when any single brush fully contains it — so abutting non-cuboid brushes cull shared interior faces identically to cuboids.
3. **Brush-side projection.** Derives visible world faces from brush sides. Produces triangulated geometry per empty leaf; faces in solid space are discarded. Overlapping brushes are ordinary authoring, so coplanar same-facing sides that meet in one leaf are emitted once: a side the other covers is dropped, and a partial overlap is clipped from the later-arriving side (a texture mismatch is warned about). The face list holds only faces some leaf draws; the watertightness check reads that list. Buried-light classification then runs on the BSP tree (see Buried lights below).
4. **Script-derived light membership.** Runs after buried-light classification, so the map-light table already omits buried lights, which have no runtime entity. The `data_script` path is resolved and checked right after parse, so a missing or unsupported script fails before BSP construction. `scripts-build` evaluates the map data script against that table and emits a validated sidecar. The table also carries `mapMembers`, the movers, trigger volumes (switches included) and spawners runtime places, so the script's member queries answer as they do at runtime. A version-2 table omits the key when a map has none, and that absence is authoritative; a version-1 (lights-only) table supplies no members, so those queries answer empty and appear in the stub inventory. `prl-build` reads it before bake namespaces form. A static light targeted by `setLightAnimation` reserves the existing animated-light bake structures — animated chunks, weight maps, and indirect delta data — even without `_animated 1`. The sidecar is compiler-only and is not a PRL section. Dynamic lights remain runtime-only and reserve no baked structures.
5. **Portal generation.** Clips splitting-plane polygons against ancestor planes to produce convex portals connecting adjacent empty leaves. Always runs; portals are stored in every PRL for runtime traversal.
6. **Exterior leaf culling.** Flood-fills through the portal graph from outside the map boundary. Exterior-reachable leaves produce no geometry. A map with a leak has interior leaves incorrectly classified as exterior.
7. **Geometry.** Fan-triangulates faces into a global vertex/index buffer. Associates each face with a material bucket and cell ID.
8. **BVH.** Builds a global SAH BVH over all static geometry organized by `(face, material_bucket)` pair. Flattens to dense arrays; leaves sorted by material bucket for contiguous per-bucket indirect draw slots.
9. **Octahedral irradiance volume and light-selection bakes.** Bakes static-light indirect irradiance into octahedral atlas tiles and isotropic Chebyshev depth moments. When animated lights are present, also bakes the sparse indirect-only delta tile companion for runtime composition. It also completes direct-SH delta and entity-shadow selection before atlas preparation.
10. **Atlas preparation.** UV-unwraps world geometry into charts; default density is 0.04 m/texel, and a per-map `_lightmap_density` worldspawn KVP opts a map into finer density. A face whose padded chart exceeds one pool layer (2048 texels) is cut on its own texel grid into sub-faces of near-equal extent: two when two fit, else the fewest whose longest piece fits with overlap on both sides. Each sub-chart is a window onto its parent's grid (frame, pitch, resolved density). Neighbouring sub-charts overlap by at least the bilinear footprint, baked as real surface, so texels match across a cut. The cut then rebuilds the pre-atlas face-identity set: per-leaf face ranges, geometry, BVH and CellDrawIndex. No other stage before atlas preparation emits face identity. A new face-naming stage joins that set or runs after the cut. The SDF bakes from the pre-cut geometry, so density edits never re-key it. A map with no static lights cuts nothing and plans no lightmap placements: its id 22 has no blocks, so nothing would read them. Only then do the streaming hints and the canonical cell partition resolve, once (§Cluster residency metadata). Each cell's charts pack into one or more cell blocks whose edges are multiples of `lcm(4, direction texel scale)`. An oversized cell fills each block before opening the next (a chart moves to a later block only when it fits no earlier block's free space), and each block is trimmed to its content. Blocks are ordered cluster-major: partition cluster, then cell id, then sub-block. The block id is that position, so a cell's blocks are contiguous. More than 65,535 blocks, or more faces within an animated light's reach than the animated block table holds, fails the build by name at atlas preparation, before any bake buffer is allocated. The animated count is conservative: every lit face, before the bake culls unlit ones. Bake-layer count is not capped; the compile working-set budget bounds lightmap scale (step 12). Before any face is cut, a lower bound of the stage's working set is checked against that budget: irradiance bytes over a texel area no packing undercuts, an uncut chart's padded extent and a cut face's own polygon area. It fails a pathological scale region fast and never refuses what the step-12 gate admits. Soft-visibility noise seeds key on the chart's world-space frame and chart-grid texel (`chart_raster::chart_texel_seed`, shared by the static, shadowmask and animated bakes; a sub-chart keys on its parent's frame and grid), never on bake-layer coordinates or face indices, so noise survives repacking and unrelated edits. Blocks then pack into internal bake layers, compiler working planes that never ship, so the per-layer bake, shadowmask fill, and per-light cache partitions keep their layer loops. Encoding slices each block's rect out of its encoded bake layer; BC formats encode 4×4 texel blocks and block origins are aligned, so a slice equals encoding the block alone. Each vertex gets its block id + 1 and a block-local lightmap UV.
11. **Cell residency set.** Bakes id 51 from cells, portals, the cell locator and teleporters; see §Cell residency set stage.
12. **Fused lightmap and shadowmask bake.** Ray-casts per-texel irradiance and dominant incoming light direction from all static lights against the global BVH. Static `static_light_map` shadows are baked as **soft area-light visibility** (stratified shadow-ray sampling of each emitter, multiplied into irradiance), not a hard 1-texel gate; an authored `_light_size`/`_angular_diameter` of `0` short-circuits back to a single hard ray. A per-light warning fires when an emitter is too small to soften at the atlas density (sub-texel penumbra). Skipped when the map has no static lights.
   - **Working-set gate.** After the block pack and before the stage bakes, on cold and warm paths alike, the stage's predicted working set is checked against `--working-set-max-size`. An overage is a named error giving predicted bytes, budget and bake-layer count, never a coarsening. A stage that bakes nothing predicts zero. Admission prices stage metadata as well as texel payloads, including ownership estimates, key prefixes and cache-use records; its bound is independent of cache mode. Partition work and layer keys are traversed without whole layer-by-light tables. Cache-use tracking grows separately per stage so lightmap work cannot resize an earlier stage's records. On the cold path one copy of encoded ids 22 and 42 is live from compose through the cache write, the PRL write and its read-back; a warm memo hit may hold the cache payload beside the parsed section (`development_guide.md` §1.4).
   - **All-resident floor.** A level that cannot stream (no id 51: the loader's portal test failed) and whose all-resident pool needs more array layers than the runtime device floor fails by name; it would render placeholder lightmaps on every machine. A level that can stream gets a named warning. The compiler and renderer count pool layers with one shared placement.
   - **Soft-shadow bake cost.** Soft visibility multiplies each stage's per-(hit × light) shadow-ray cost by the area-sample count, so penumbra-heavy maps pay a multi-fold bake-time increase over the hard-gate path. Adaptive escalation (a 4-ray probe set, escalating to the full count only in penumbras) keeps fully-lit/fully-shadowed texels cheap and bounds that cost. The probe set is snapped against the full lattice once per light and count, not per receiver: the lightmap builds it per chart, and `SoftProbes::new` reuses one point/spot snap per thread, keyed by count alone — so the point/spot lattice must stay light-independent. The SH bounce skips a light's shadow ray when it is out of range or its Lambert term at the hit is exactly zero (past a spot's outer cone, or behind the surface); exact zero keeps the bake byte-identical, so a near-zero fringe is still traced. Lightmap layers and ShadowmaskAtlas memos are cached, so unchanged inputs pay that cost once. Only the indirect SH delta bake is cached per CSR entry, so its unchanged lights are not re-baked (see §Build Cache); the direct and animated-direct delta bakes recompute uncached. `--soft-shadow-samples` (default 32) sets the escalated full-sample count. Raising it invalidates cached lightmap layers and shadowmask memos keyed through eligible lights' layer hashes. The animated weight-map stage is cached as one whole-stage entry, so raising the sample count re-bakes it. Adaptive-escalation thresholds stay fixed constants regardless, so the bake stays deterministic.
   - **Fused walk.** Cold and warm builds run the same fused lightmap and shadowmask walk. It produces per-light partitions concurrently through a bounded window of lights, then folds them and feeds the shadowmask fill strictly in global light order, so output never depends on completion order. Cache reads and writes run outside the compiler's resource-governor permits, and the stage's peak-memory prediction charges the window's resident partitions. Each light skips charts beyond its falloff reach (directional lights reach every chart); a culled chart still advances progress, so progress completes at its published total.
13. **Reflection-probe capture.** Runs after texture mips, the last bake stage before the cluster directory, with the BVH still in scope, against the static world only; it reads ids 18, 22, 23, 25, 27, 34 and 52 and the albedo and emissive `.prm` sidecars, so it follows every stage that produces them. Placement (`resource_management.md` §5.1) runs on every build before the cache lookup, so drop and cap warnings reach warm builds. Each probe traces four rotated-grid rays per interior tile texel against the BVH, culling back faces as the world pass does; hits shade from the decoded lightmap, the animated weight maps, the SDF lights the runtime's selection picks there with shadow rays, the base and delta SH volumes (each stored slot clamped after its deltas compose, as the runtime clamps it), and the albedo and emissive mip the ray's footprint selects (`resource_management.md` §5.2). The same rays fill the distance tile. Per-light unit contributions accumulate in the same pass, so the composed capture and the share table come from one trace. Cached as one whole-section entry (§Build Cache) whose key hashes placement's output (each probe's source, box, point and bound faces) and the outputs it reads — the packed SH and delta volumes, not their inputs, since warm SH differs from cold — plus the constants that shape the capture; byte-identical across worker counts. After the section exists, cold or warm, the stage warns, naming the probe and its entity, when its capture point sees under half of its bound water by the runtime's own capture-shadow test on the stored distance tile (`rendering_pipeline.md` §7.3a); that water draws no reflection. Emits id 54 when a probe survives.
14. **Pack.** Writes all sections to a sibling staging file and validates them through the same open handle. Publication holds a persistent sibling advisory lock. Cooperating prl-build processes serialize the final identity check and rename; a compiler whose original output changed refuses publication. The lock does not constrain external writers that ignore it. Such a writer can modify an existing inode without changing its identity, or replace the output or staging name after the last check. Failed staging files remain for manual cleanup because pathname unlink cannot prove the checked file was not replaced before deletion. Publication does not fsync the file or parent directory.

**Buried lights.** A baked-tier Point or Spot light whose origin lies inside solid is excluded from the build and warned about, by its adapter-written source label (entity index, classname, authored origin). The test runs on the compiler BSP right after BSP construction and brush-side projection: every leaf within 1 mm of the origin must be solid. A bounding-box test would flag lights in open space beside brushes. A light whose origin is on or within 1 mm of a face is kept; `light_size` plays no part. Classification precedes the data script, so the script's light table omits buried lights. The light namespaces form after the membership manifest applies, and a buried light is in none of them. So it takes no lightmap, SH, delta, entity-shadow-selection, shadowmask-channel or chunk-list work, and it has no AlphaLights, LightInfluence or LightTags record, so it gets no runtime `spec_lights` entry and no script entity. The SDF-atlas need check and light-animation validation still read the full parsed light list. Source indices still name positions in that full list, so no other light's identity shifts. Dynamic lights can move and directional origins light nothing, so neither is classified. Excluding it matters for specular: otherwise the light is admitted to the chunk lists around its brush and, with no mask channel, adds specular through the wall. Chunk-list admission traces from the origin, so the rule tests the origin, not the emitter.

**Fluid surfaces.** A fluid volume's visible surface is every brush side not buried in solid world and not shared with another brush of the same entity. Each side is clipped by successive plane cuts against nearby solid world brushes (worldspawn, `func_group`, `switch`) and the entity's other brushes: the part inside either is removed, a side flush against a solid face is buried (it would z-fight), and a partly buried side keeps only its exposed part. Where two sibling brushes share a coplanar, same-facing side, the earlier brush keeps the overlap, so the union's boundary draws once. Each exposed part is then split along every push brush over it, of any push volume, so a current drifts exactly the surface its push covers. A face point counts as covered when its drift sample, 2 mm into the fluid, is inside the push: the runtime samples at that depth, and a test pins the two equal. So a current flush with or just under the waterline covers the face, a wind resting on it does not, and an oblique plane cuts where the sample crosses it. An edge or corner touch leaves a face whole, no part under 1 mm² is kept, and parts sum to the face's area. The surface is translucent, so a crack between two faces shows. Once every volume's faces are built, seams close level-wide, across volumes, fluids, planes and facings, judged on the f32 positions the section encodes, within 0.1 mm (the split epsilon, from which the renderer's collinear and grid-plane tolerances derive). First vertices of different faces weld to one of their own positions, never two vertices of one face and none further than 0.1 mm, so a chain of near points breaks rather than drifting. Then each face gains every other face's welded vertex within 0.1 mm of its boundary, on the edge nearest it and in order along that edge (a point equally near two edges takes the one whose sector about the area centroid holds it), repeated on every face an insert changed. Each face stays a simple polygon within about 0.1 mm of its convex piece, but need not be star-shaped about its centroid: on a long thin face a point just inside a long edge lies in a short edge's sector. A face the weld would flip, flatten or carry across itself keeps its own positions, and inserts restore its shared boundary. Two faces that meet therefore carry bit-identical vertices along their shared boundary (the renderer keeps every inserted vertex a triangle corner, `rendering_pipeline.md` §7.3a). Closure moves and adds vertices only: no face is added, dropped or reordered and no texture index changes, so id-54's (volume, face) bindings and the drift maps keep their meaning; each vertex takes its UV from its own side's projection. A weld may tilt a piece under a few millimetres off up-facing and cost it its probe binding. Lookup is output-sensitive (a cell hash for the weld, a point tree for inserts) and the result is identical warm and cold and across worker counts. A level whose fluid faces have no T-junction or near-coincident vertex keeps identical id-52 bytes; any other level's id-52 bytes change once, which misses the id-54 cache and recaptures its probes. Fluid faces never weld to world faces: the depth-written wall behind a fluid edge fills any crack there. Surfaces build after geometry and movers, just before TextureMips: each face indexes the shared level texture table, and a texture only fluid faces use appends to TextureNames and TextureCacheKeys after the world's, so no earlier stage's cache key sees it. UVs are in texels from the side's projection, as static faces derive them. These faces are not the drawn mesh: at install the renderer cuts each along the SH probe lattice (`rendering_pipeline.md` §7.3a), so compiled face density sets no lighting resolution. A dry volume has no faces. Beyond ids 52 and 53, and the reflection probes their up-facing faces derive (id 54), volumes add nothing to any section: cells, portals, static collision, navmesh, lightmaps, SH and SDF are byte-identical to the same map without them.

**Planned section-footprint diagnostics.** Pack reports every emitted section from the
descriptors it actually writes, using the known section name when available and retaining
unknown ids numerically. Per-section bytes sum to the payload portion of the file; header
and section-table bytes are separate container overhead. Reporting must not re-encode a
section or defeat the one-payload-at-a-time write lifetime.

**Atlas preparation and SH ordering.** Chart planning and packing are not ray work, and the lightmap bake, shadowmask fill, animated light chunks, and animated weight maps consume only the charts and placements they produce. The SH/delta, entity-shadow-selection, billboard direct-scatter, and `ChunkLightList` stages complete before atlas preparation. This ordering is load-bearing: direct-SH delta work can clear entity-shadow selection wholesale, and the final id-40 selection is a ranking tier for shadowmask ownership, which is decided inside the fused walk; cut faces fix their owners from the cut-face estimate just before it (§PRL, ShadowmaskAtlas ownership). These pre-atlas stages consume position-only geometry, so density and scale edits do not invalidate their cache keys. Of the cached stages after atlas preparation, two stay density-independent by key. The SDF atlas hashes vertex positions, indices and BSP solidity of the geometry as it stood before the oversize-face cut, not the cut geometry or the lightmap UVs and block ids atlas preparation writes, so even a density edit that moves a cut hits it. CellResidencySet leaves per-cell face ranges out.

**Cell residency set stage.** Right after atlas preparation, the CellResidencySet stage (id 51) reads only the encoded Cells, Portals, CellLocator and Teleporters (id 55), plus, when teleporters exist, the final BSP leaves' f64 half-spaces, which classify teleporter regions exactly where the encoded f32 bounds cannot. It builds the visibility world through the runtime loader's own section-to-world conversion, so it samples exactly the portal graph the runtime walks; a portal set the loader would reject emits no section. Per-camera-cell sampling runs in parallel under the governor. It reads no hint, chart, or light, and never feeds a bake.

**Cluster residency metadata and payload stage.** The canonical cell partition resolves
once, during atlas preparation, after the oversize-face cut: authored streaming hints resolve
against the finalized portal polygons and runtime cells, and seams split the partition. Its
inputs are final once the cut has rebuilt the BVH. It counts BVH leaves per cell, so a density
edit that moves a cut can change cluster membership; the stage is uncached, and no cached
pre-atlas stage keys on it. Lightmap blocks take their cluster-major order
from it, and the ClusterDirectory stage consumes the same partition rather than
recomputing it. After all bakes and final section-presence decisions, that uncached stage
addresses only the emitted SH sections. It emits id 49, then emits id 50 from borrowed
finalized SH sources when id 34 is present. It runs before serialization in warm and cold
builds. It never feeds a bake: global SH rays, coarsening, and legacy section bodies remain
unchanged. Construction and validation borrow finalized metadata without cloning or
re-encoding whole lighting payloads.

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
  on another permitted item — that would deadlock at one permit. A permit guards ray work and the
  lightmap window consumer's CPU work, not cache I/O: a cached work item runs its cache get and put
  outside its permit, and a hit checkpoints without one. The indirect delta SH bake and lightmap
  partitions do this; the direct and animated-direct delta bakes do not cache. Base SH groups still
  hold their permit across get and put. The lightmap window's consumer (fold, dilate, layer encode)
  holds exactly one permit, so that CPU work is bounded by `-j`, and it never waits on chart tasks.
  `BakeControl` (`bake_control.rs`)
  carries the governor plus a stage's display-only progress counter to those work-item boundaries.
- **Progress is display-only.** Counters (`StageProgress` in `reporter.rs`) feed percent/ETA and never
  flow back into bake output, so pausing/throttling/the TUI cannot perturb the `.prl` — output stays
  byte-identical to a straight-through build (the Determinism invariant under Build Cache).
- **Foreground stage.** Concurrent lifecycle stages can advance together, but the TUI highlights
  exactly one foreground stage. The fused lighting bake keeps ShadowmaskAtlas live in the background
  while Lightmap Bake owns foreground focus.
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
| Geometry | 17 | Always. Container-entry version 2; the loader rejects any other, since the payload has no version of its own. Each vertex's `lightmap_block` is its id-22 block id + 1 (0 = no lightmap), and its lightmap UV is block-local, spanning that block's extent. Load rejects a vertex naming a block past the id-22 table |
| AlphaLights | 18 | Always |
| Bvh | 19 | Always |
| ShVolume | 20 | Retired legacy L2 SH irradiance payload; stale files are rejected by section-internal version |
| LightInfluence | 21 | When compiled with lighting |
| Lightmap | 22 | Always; v3 per-cell blocks. Zero blocks when a map has no static lights, which the runtime renders with its neutral placeholder |
| ChunkLightList | 23 | Always; per-chunk static-light index lists for specular culling and runtime sdf-light selection |
| AnimatedLightChunks | 24 | When at least one animated-light chunk bakes a lit texel. Candidates are faces overlapping an animated light's influence sphere (no occlusion, facing, or cone test); after the weight-map bake, chunks with no lit texel are dropped from ids 24 and 25 and from the BVH leaf chunk ranges |
| AnimatedLightWeightMaps | 25 | Paired 1:1 with id 24; per-texel weight maps for the compose pass, v5 compact and paged. Each animated face is one block, its chart placement rect, whose chart padding is a zero gutter. After the unlit-chunk cull, blocks pack in cell order (MaxRects per page, never revisiting earlier pages) into square, power-of-two pages of one size: at least the largest block, at least 1024² unless the bake layer is smaller, at most the bake-layer size; the fewest-bytes size wins, ties keeping the smaller page. A block table keys each block by its id-22 cell block and block-local texel origin, and maps it to its page and offset; chunk rects are in compact coordinates and keep their offset inside their block. The weights stay in static-lightmap texel space. The compiler never writes a packed layout larger than the identity layout (every block at its bake-layer position, one page per bake layer still holding a chunk, at the bake-layer size); the weight-map stage caches that identity layout. The VRAM budget (`page² × pages × 12 B` ≤ 1 GiB) is checked on the chosen layout. The build fails on a vertex shared between a block-owning face and another face, a bilinear footprint leaving its placement, or a block count over `ANIMATED_BLOCK_CAP`; with a placeholder static lightmap (no static baked lights) the vertex guards and stamping are skipped because vertices carry no lightmap UVs. Geometry (id 17) vertices carry the block id (`n + 1`, 0 = none) in their former pad field, stamped after the animated atlas repack. Loaders accept v5 only. In every build they reject a page size that is not a power of two holding the largest block, and an animated block that names a block past the id-22 table or leaves its cell block's extent. With a zero-block or absent id 22 the block-frame checks are skipped and the map renders without animated light |
| LightTags | 26 | When at least one light carries a tag; one space-delimited tag-list string per AlphaLight record (empty string = untagged) |
| DeltaShVolumes | 27 | When the map has at least one animated light; per-light sparse octahedral irradiance delta tiles, with a per-affinity-cell coarsening level |
| DataScript | 28 | When `data_script` KVP present on `worldspawn`; compiled script bytes + original source path |
| MapEntity | 29 | When the map has at least one non-light, non-worldspawn entity; per-entity classname, origin, angles, tags, and KVP bag for runtime classname dispatch |
| FogVolumes | 30 | Always (leading `u16` version, then fog_pixel_scale and the volume list; 10-byte overhead when no fog_volume brushes present). Carries no gravity: level gravity lives in EnvironmentVolumes |
| FogCellMasks | 31 | When at least one fog volume entity is present (fog_volume brush, fog_lamp, or fog_tube) |
| TextureCacheKeys | 32 | Always; one 32-byte blake3 per TextureNames entry pointing at a `.prm` sidecar under `baked/materials/` |
| SdfAtlas | 33 | When the map has SDF static occluder data |
| OctahedralShVolume | 34 | When compiled with lighting; base indirect irradiance as layer-aware octahedral atlas tiles |
| DirectShVolume | 35 | When the map has static baked lights; metadata-derived stored static-direct layer-aware octahedral irradiance for movers/skinned meshes and legacy billboard fallback; BC6H at rest; no depth moments (read from id 34); same id 34 node set, slot order, geometry, format, and payload contract as OctahedralShVolume; section-internal `DIRECT_SH_VOLUME_VERSION` |
| NavMesh | 36 | When the map has walkable navigation; baked regions/portals for runtime pathfinding |
| CellDrawIndex | 37 | When the BVH has non-empty leaves (omitted for zero-leaf maps); independent of portal presence. Per-cell CSR of owned BVH-leaf spans driving the runtime candidate cull (`rendering_pipeline.md` §7.1) |
| Cells | 38 | Always; runtime visibility units, preserving compiler spatial ids for cells, portal endpoints, fog masks, BVH leaf `cell_id`, and diagnostics |
| CellLocator | 39 | Always; point-to-cell decision tree used by runtime visibility and object placement diagnostics |
| EntityShadowLights | 40 | When `DirectShVolume` and usable `DirectShDeltaVolumes` are emitted and the compiler selects at least one baked-tier static light for runtime entity-shadow promotion; ascending indices into `AlphaLights` |
| DirectShDeltaVolumes | 41 | Required companion for `EntityShadowLights`; sparse per-selected-light direct octahedral irradiance deltas covering every selection index, with a per-affinity-cell coarsening level |
| ShadowmaskAtlas | 42 | When any lightmapped face admits an owner, independent of ids 40 and 41; per-face owner lights (up to four compact static spec-light indices per id-17 face) and their baked world-visibility masks. Format tag `SMB7`: an owner table, then one BC5 `.rg` group A and group B plane per id-22 block, in id-22 block order. Validated against ids 17, 18 and 22; a malformed section, an invalid owner table, a block count that differs from id 22's, or id 42 without id 22 is a hard load error (§ShadowmaskAtlas ownership) |
| KinematicGeometry | 43 | Version 7 when the map has at least one `kinematic_mover`; origin-relative mover vertices/indices/face metadata, static spin/blocking authoring, closed-mover portal associations, carried dynamic-light links, mover sound keys, and `kinematic_waypoint` records |
| TriggerVolumes | 44 | When the map has at least one `trigger_volume` or `switch`; trigger AABBs, direct mover commands, and named enter/exit events |
| AnimatedDirectShDeltaVolumes | 45 | When the map has animated baked lights; sparse per-animated-light direct-SH delta tiles composed into the dynamic-receiver atlas, with a per-affinity-cell coarsening level |
| CellVisibility | 46 | Optional, versioned, strictly parsed baked Cell→Cell coupling relation: per-cell reachability component IDs plus canonically ordered coupled-pair distance/aperture graded records. Missing → conservative all-perceivable, no-graded-detail fallback. Id 14 (`LeafPvs`) is a retired hole; do not reuse |
| BillboardDirectScatterVolume | 47 | Optional dense normal-free direct-scatter base for billboards: `Rgba16Float` RGB plus binary validity alpha in the x-fastest id-34 probe order. Static-only maps omit it when no `static_light_map` source contributes. A map with animated-only `static_light_map` scatter entries may emit an all-zero RGB base solely as the required grid/validity anchor for a valid id-48 companion. Invalid or missing data selects legacy billboard direct lighting |
| AnimatedBillboardDirectScatterDeltaVolumes | 48 | Optional dense animated billboard direct-scatter deltas: reuses id-45 descriptor mapping and CSR affinity layout, but stores fixed dense 4×4×4 `Rgba16Float` RGB deltas per CSR entry (reserved zero alpha). Required with id 47 whenever id 45 is present; a missing, invalid, or oversized pair selects legacy billboard direct lighting |
| ClusterDirectory | 49 | Always in current compiler output; deterministic cell clusters, SH-family ownership/halo metadata, and authored seam/pin/priority hints; container-entry and internal version 2 |
| ClusterShPayloads | 50 | When id 49 and id 34 are present; independently readable per-cluster payloads for streamed ids 27/34/35/41/45; container-entry version 1, internal epoch 1 |
| CellResidencySet | 51 | When the level has usable portals; per-camera-cell lightmap residency relation: CSR over every camera cell id of `(cell, lead)` entries, `lead` the smallest movement lead (id-46 fixed point, max 32 m) at which the cell is mandatory. Pins are not baked (id 49 supplies them). Absent → lightmaps run all-resident. Malformed → load error; internal version 1. See the id-51 paragraph below |
| EnvironmentVolumes | 52 | Always; internal version 1. Header: level default gravity. Then every `fluid_volume`, `gravity_volume` and `push_volume` record in map entity order: source entity, priority, optional gravity, optional push, fluid name (empty = dry), convex brush leaves, and fluid surface faces. A level with no volumes still carries its gravity. Absent → load error naming the section (stale format; recompile). See the id-52 paragraph below |
| PushForces | 53 | When some `push_volume` authors a force other than 1; internal version 1. One `f32` force per id-52 record, in record order; a record without push carries 1. Absent → every force is 1. Malformed → load error. See the id-53 paragraph below |
| ReflectionProbes | 54 | Optional: when at least one probe survives; internal version 2. Header carries the engine tile dimension and border and the colour and distance format tags. Per probe: source entity, projection box, capture point, tile layer and animated-light shares; then (volume, face, probe) bindings, so id 52 is unchanged; then the colour tiles, then the distance tiles. Absent → no probes. See the id-54 paragraph below |
| Teleporters | 55 | When the map has at least one `trigger_teleport`; internal version 1, encoded as id 52 is (endianness, version header, record count, convex-leaf planes, length-prefixed strings). Per record, in map entity order: source entity, mode, enabled-on-spawn, convex leaves, destination origin and yaw, landmark origin and yaw (zeros for classic, ignored), `arrival_sound` and `on_teleport` (empty = none). At most 65,535 records; index `0xFFFF` is reserved for no teleporter. Absent → no teleporters. Malformed → load error: bad header, mode or flag, truncation or trailing bytes, invalid convex leaves, a non-finite pose, or a repeated source entity |

**Cluster directory (id 49).** Resource-agnostic clustering uses portal adjacency and
fixed primitive/cell limits of 64/32. Every runtime cell belongs to exactly one stable
cluster; multi-cell clusters are connected, and an indivisible over-budget cell remains an
explicitly flagged singleton. Resource coverage does not alter that partition. The
v2 directory preserves the old 40-byte header and existing tables; its final header
words count appended sorted seam portal IDs and 16-byte cluster hint records. Seam cuts
keep marked portal endpoints in different clusters, even across alternate portal paths.
Pins and max-on-overlap priority are per-cluster hints; they never reuse the existing
cluster-record flags, whose bit 0 marks an oversized singleton. With no hints, the
canonical partition and id-50 content remain unchanged. The loader and compiler
readback reject malformed hints or noncanonical membership, including a v1 directory.
Authoring stays compiler-only: each streaming-hint entity owns exactly one finite,
positive-volume convex brush. A seam must cut through a generated portal with positive
area and interior depth on both sides; resident and priority regions must overlap at
least one runtime-cell AABB by positive volume. `_stream_priority` is an integer from
0 through 3 (blank is 0); zero emits no priority record. These brushes never enter BSP,
geometry, collision, gameplay, visibility, lightmap, SDF, or navmesh output.
The uncached `ClusterDirectory` metadata stage runs after every bake and final SH-family
presence decision and before section serialization. The chosen limits and bounded dry-run
evidence live in `measurements/sh-probe-streaming/cluster-directory-thresholds.md`.

The directory addresses SH resources by grid-relative probe or affinity-cell ranges, never
payload byte offsets. Cross-cluster affinity cells carry one compile-time-fixed accumulation
owner; other references are reconstruction-only halo. One authored level remains one
logical PRL. A missing id 49 remains a whole-load level. The loader re-derives the whole
range table from cells, probes and the cell locator and rejects any difference; that
derivation runs on the level worker every load, so its cost must never grow as active bricks
× member cells.

**Cluster SH payloads (id 50).** Id 50 is a validated, independently readable,
cluster-major companion for ids 27/34/35/41/45. It is emitted after id 49 only when id 34
is present. Ids 47/48 remain whole-resident and do not appear in id 50. Id 50 duplicates
the streamable payload deliberately: global BC6H atlas blocks cannot be gathered into
independent cluster cells from legacy compressed bytes. Its source inventory, source
versions, index, and chunk hashes must match id 49 and the emitted legacy sections before
any runtime-mode decision. Invalid id 50 rejects even when streaming is disabled. Valid
id-49/id-50 levels default to bounded asynchronous residency; `off` selects the validated
legacy whole-load path and `sync-proof` is the deterministic no-eviction test path.
Existing section layouts, epochs, cache-stage epochs, sampler bindings, and sample taps
remain intact, including id 34 v11 and id 35 v4.

**Cell residency set (id 51).** A streaming-owned cell relation: for each camera cell, the
cells a lightmap-block residency controller must hold. It is keyed by cell and never names
a block, so a later streamed resource can map the same cells to its own units. It is not an
id-46 axis: id 46 stays conservative, and this set is sampled and carries a lead. The bake
samples the runtime portal walk, over the world the loader itself builds from ids 38/15/39,
from a 3×3×3 inset eye lattice in every camera cell (not solid, not exterior). It dilates
each sampled set by one portal hop, then records every cell within portal-path lead
`MAX_LEAD_METERS` (32 m) plus each such cell's dilated set. Each entry carries the smallest
lead that makes it mandatory; lead 0 is the camera cell's own dilated set. Teleporters
add zero-length routes from source region to arrival cells: the cell the locator puts the
classic destination in, or the cells the seamless source union maps onto. Source and image
cells come from exact convex intersection against the final BSP leaves after AABB
rejection, contacts included, solid and exterior leaves excluded. Every camera cell within
maximum lead of a source region carries its arrival cells and their dilated sets at its
distance to the region, lead 0 where the region touches it; arrival and ordinary origins
merge nearest-first before any cell is stamped. A route is one hop: arrival rooms are
mandatory only from camera cells within lead of a source, and arriving inside another
teleporter does not follow it. Armed state plays no part, so a set piece is warm before it
arms. The lightmap residency dry run checks a PRL's baked id 51 against direct evaluation:
equal without id 55; with it, a superset holding every directly evaluated cell at no greater
lead, the excess reported as arrival delta. Each camera
cell's range is sorted by lead, then cell id, one entry per cell. Pins are not in it; the
runtime reads them from id 49. The cache key hashes `postretro_visibility::PORTAL_WALK_EPOCH`
(§Build Cache): a portal-walk change that can alter any visible set must bump that epoch,
or warm builds keep serving sets from the old walk. Load rejects: another version; a camera cell count
that differs from the level's cell count; a length that disagrees with the counts; CSR
offsets that do not start at 0, decrease, or end anywhere but `entry_count`; an entry naming
a cell past the cell count; a lead past `max_lead`; and a range that is unsorted or repeats
a cell.

**Coarsened delta sections (ids 27, 41, 45):** The wire representation supports an independent L0/L1/L2 level for every affinity cell in each section. L0 stores every valid probe tile. L1 stores valid brick-corner tiles. L2 stores one synthesized mean tile over the brick's valid probes. Payload size and order follow kept probes and kept rank, not dense probe index. Production adaptively classifies id 41 only. Ids 27 and 45 intentionally emit uniform L0 until animation and script amplitudes have bounded runtime contracts. Protection AABBs force intersecting bricks to L0 in an adaptively classified section before one-level seam smoothing.

**Delta tile layout (ids 27, 41, 45):** Each octahedral texel stores three little-endian f16 RGB halves. Probe validity comes from the section mask and kept set; delta texels carry no alpha. Disk payload and renderer storage-buffer layout are identical. Base atlases ids 34/35 keep their independent format, including alpha where their sampler contract needs it. Billboard scatter id 48 also keeps its independent four-half sample format.

**Delta-section loader floor:** Each id 27, 41, and 45 raw payload must fit the 128 MiB storage-buffer binding floor. Loader checks the on-wire byte length before decoder allocation. An over-floor id 27 or id 45 is independently disabled, so compose falls back to its base contribution. An over-floor id 41 clears id 41 and its paired `EntityShadowLights` selection (id 40), preserving the all-or-nothing promotion contract. This loader safety floor is separate from the compiler's post-compaction 256 MiB aggregate cap across ids 27/41/45. The compiler also reports, but does not reject, payload above the 64 MiB authoring target.

**Animated billboard-scatter cap:** Id 48 has an independent 64 MiB maximum encoded-section size because every retained CSR entry expands to a dense 64-probe `Rgba16Float` block even when id 45's compact payload is empty. Compiler predicts this size from finalized id 45 before scatter cache lookup or bake and withholds ids 47/48 together on overage; pack repeats the guard. Loader validates container bounds, then soft-drops an over-cap id 48 before decode and preserves id 45. Renderer independently checks every id-48 storage buffer against the active device's storage-binding and single-buffer limits; failure binds the dummy scatter texture and selects legacy billboard lighting. The cap is policy only and adds no wire field or version change. Valid empty id-45/id-48 CSR pairs remain present so compose can base-seed.

**Lightmap (id 22):** versioned section carrying the baked irradiance and dominant direction as per-cell blocks, the residency unit a streaming loader reads one block at a time (`rendering_pipeline.md` §4). Irradiance is BC6H (`Bc6hRgbUfloat`) at rest by default — ~8× smaller than the uncompressed `Rgba16Float` debug path, hardware-decoded and -filterable. Direction is always `Rg8Unorm` octahedral on a nearest sampler (octahedral lerp ≠ slerp, so it is never compressed or linearly filtered). BC6H is lossy, so the lightmap stage is exempt from the byte-identical determinism invariant (correctness is round-trip within tolerance; the cache keys on inputs regardless).

Wire layout (version 3, all little-endian, offsets from the section payload start; source of truth `crates/level-format/src/lightmap.rs`):

- **Header (20 bytes):** `u32 version` (= 3; every other version is rejected), `u32 block_count` (≤ `MAX_LIGHTMAP_BLOCKS` = 65,535, so block id + 1 fits a vertex's `u16`), `u32 direction_texel_scale`, `u32 irradiance_format` (0 = Rgba16Float, 1 = Bc6hRgbUfloat), `u32 mode` (0 = Shadowed, 1 = Unshadowed). The direction scale is a power of two dividing `LIGHTMAP_POOL_LAYER_EDGE` (2048); the bake keeps full-resolution irradiance and reduces direction by `--direction-texel-scale` (default 2) per axis. The compiler writes only Shadowed. The runtime records Unshadowed but does not honour it, and warns at load. Zero blocks is a valid header with no records.
- **Block records (36 bytes each):** `u32 cell_id`, `u16 width`, `u16 height`, `u64 irradiance_offset`, `u32 irradiance_len`, `u64 direction_offset`, `u32 direction_len`, `u32 reserved` (= 0). Block id is the record index. Records are sorted cluster-major: canonical-partition cluster, then cell id, then sub-block. A cell may own several contiguous blocks, or none.
- **Blobs, in record order:** each block's irradiance (Rgba16Float `u16×4` per texel row-major, or BC6H 4×4 blocks of 16 bytes), then its direction (`(width/scale) × (height/scale) × 2` bytes). A block's two blobs are adjacent, so one read fetches both; blocks' blob pairs are disjoint and in record order.

Load rejects: another version; an unknown irradiance format or mode; a direction scale that is not a power of two dividing the pool layer edge; more than 65,535 blocks; a zero extent, an extent past 2048 on either axis, or one that is not a multiple of `lcm(4, direction scale)`; a blob length that disagrees with its block's extent and format; a blob range overlapping the index or leaving the section; a direction blob that does not start where its irradiance blob ends; a block whose blobs start before the previous block's end; a nonzero reserved field; a `cell_id` past the Cells table, or a cell whose blocks are not contiguous.

**ChunkLightList (id 23):** per-chunk index lists into the static (baked-tier) spec-light set — dynamic lights are excluded, riding the runtime shadow-map path. The per-chunk cap is 256. Contained point and spot lights have hard first-tier precedence. Other overflow candidates rank by model-aware closest-AABB influence, then compacted static-light slot. This prioritizes strong lights across chunk faces, but neighboring chunks may still retain different subsets at an influence threshold. Admission is **err lit**: the list must carry every static light that can shade a receiver surface inside the chunk. The renderer's forward specular pass iterates only a chunk's list with no fallback, so a dropped reachable light leaves a chunk-plane gap in the specular highlight — the bake's admission test is the renderer's correctness boundary here.

**OctahedralShVolume (id 34):** sibling replacement for legacy `ShVolume` (id 20), carrying base indirect irradiance. `SH_VOLUME_VERSION` is section-internal; current version 11 rejects stale sections. Probe metadata carries binary validity, density level, and hierarchy scale. Scale 0 is one 4×4×4 affinity brick; each higher wire scale doubles the aligned brick edge. Production selection currently stops at scale 1; compiler measurement overrides exercise scales 0–3. L0 stores valid probes in local x-fastest order and is always scale 0. L1 stores the node's eight corners in corner order; invalid corners are zero tiles. L2 stores one synthesized mean over valid node probes. Empty scale-0 bricks store no tiles. Partial edge bricks stay L0. Every scaled node retains at least one valid probe in its aligned origin brick, so the runtime's elected origin workgroup can write its stored slots; the loader rejects a section that violates this invariant. Node origins alone advance the stored-tile prefix; every member resolves that origin slot. Payload slots remain x-fastest and are tagged BC6H or `Rgba16Float`. Runtime derives a pointer-free indirection word from validated metadata and composes and samples the same stored atlas. See rendering_pipeline.md §4 "Adaptive base-probe spacing".

**DirectShVolume (id 35):** baked static-direct octahedral irradiance for entities and billboards. `DIRECT_SH_VOLUME_VERSION` is section-internal; current version 4 rejects stale sections. It carries no probe metadata or depth moments: id 34 supplies validity, density levels, and node scales. Its stored-atlas geometry and node-major payload slots must exactly match id 34's stored set; loader cross-checks geometry and payload length before use. The irradiance blob is layer-major, matching id 34: all BC6H block bytes or uncompressed `Rgba16Float` texels for layer 0, then layer 1, etc. Stored BC6H at rest. Emitted only when the map has static baked lights. Runtime: sampled by skinned-mesh and billboard shaders, gated by `has_direct`; forward and fog pipelines bind but do not sample it.

**DirectShDeltaVolumes (id 41):** required sparse CSR companion for `EntityShadowLights`. Runtime promotion is all-or-nothing: every selection index in `EntityShadowLights` must appear in `affinity_lights` at least once. Missing, malformed, or partial direct deltas clear both sections at load so a selected light never runs as both direct SH and runtime light. Layout mirrors `DeltaShVolumes` affinity cells (`affinity_factor = 4`, `affinity_dims = ceil(DirectShVolume.grid_dimensions / 4)`, CSR `affinity_offsets`, and flat `affinity_lights`) but carries no animation descriptor mapping. Each affinity cell has its own coarsening level; each CSR entry stores tiles only for that cell's kept probes, by kept rank. L2 stores one synthesized valid-brick mean tile. Production bakes coarsen id 41 by default under the runtime-safe direct envelope; worldspawn `_sh_coarsen "0"` short-circuits classification and preserves uniform L0. Ids 27 and 45 remain uniform L0 until their animation/script amplitudes have a bounded runtime contract. `affinity_lights` entries are selection indices: zero-based positions in the `EntityShadowLights` list, not `AlphaLights` indices. Deltas use the same direct bake math as `DirectShVolume` before BC6H compression and are clipped to each selected light's reach. Compiler-side conservative dropping may omit a bounded-negligible entry through the existing CSR absence encoding, but always retains coverage for every selection index — including a selection whose baked direct-SH contribution is entirely zero (e.g. a spot whose cone reaches no valid probe). The baked delta is the far-LOD crossfade term, independent of the light's runtime near-tier shadow-pool term, so a zero entry means "baked contribution is zero here, use the runtime term," never "drop the light"; any reach- or contribution-based cull must preserve this canonical entry, or promotion clears both sections at load. Planning and baking share the same affinity-reach policy and assert exact CSR identity at their handoff, so the working-set gate cannot budget a different id-41 shape from the one that is materialized.

**ShadowmaskAtlas (id 42):** per-face shadowmask owners and their baked masks, read by world static specular and the promoted-light union subtraction (`rendering_pipeline.md` §4, Static specular ownership). Little-endian layout (source of truth `crates/level-format/src/shadowmask_atlas.rs`): `u32` format tag `SMB7`, `u32 face_count` equal to id 17's, then one owner entry per id-17 face in face order — four `u16` compact static spec-light indices (the `!is_dynamic` order of `AlphaLights`), ascending, `0xFFFF` for an empty channel, empties trailing — then `u32 block_count` and one 28-byte record per block in id-22 block order: `u64 group_a_offset`, `u32 group_a_len`, `u64 group_b_offset`, `u32 group_b_len`, `u32 reserved` (= 0). Blobs follow in record order: each block's group A plane (channels 0/1), then its group B plane (channels 2/3), adjacent, each BC5 `.rg` at the id-22 block's width × height; decoded `255` = fully visible. Block `i` pairs with id-22 block `i`: the two install together or not at all. Parsing rejects, identically in both load modes: an unknown tag; the retired `SMB5` and `SMB6` tags, as named re-bakes; a face count that differs from id 17's, checked from the 8-byte header before the owner table is read; an owner index at or past the static spec-light count; a duplicate or descending owner, or an owner after an empty; an owner table too large for the section; a block count that differs from id 22's; a group length that disagrees with its block's extent; a blob range outside the section blobs; a group B blob that does not start where group A ends; a block whose blobs start before the previous block's end; a nonzero reserved field. Any such fault, or id 42 without id 22, fails the load. Id-42 validation reads no id-40 state, so clearing ids 40/41 at load keeps the owner table. The owner table loads whole at install and never streams. The loader stamps each face's entry onto that face's vertices, recovering face index ranges from the BVH leaves (one non-empty leaf per face, ordered by index offset); a leaf/face mismatch, an out-of-range vertex, or two faces sharing a vertex with different entries logs one `[PRL]` error and leaves every vertex ownerless. Pack refuses to write id 42 when a Geometry face has no triangle, when the owner count differs from the face count, or when an entry fails the loader's validation. `_bake_only 1` lights are absent from `AlphaLights` and own nothing.

**ShadowmaskAtlas at rest.** BC5 `.rg` behind the in-payload format tag, at half the raw bytes; raw RGBA is retired, with no second decode path. Four mask channels per texel: owner position `s` addresses group `s / 2`, channel `s % 2`. The runtime pool layer holds a block's two groups **side by side**, so it is two pool edges wide (4096) and shares the lightmap pool's layer count. Masks mix hard 0/255 texels with penumbra, so the encoder picks BC4's six-value mode (explicit 0 and 255) per block when it lowers error; normal-map BC5 keeps min/max mode. The encode runs in the bake before the whole-section memo, so warm output stays byte-identical to uncached — the BC6H lossy exemption does not extend here. No block is too wide for id 42: the oversize-face cut and cell packing bound every block by the pool layer edge, and the doubled pool layer fits the pinned 8192 texture width by construction. A misaligned atlas fails the build in every profile rather than emitting a truncated payload. `--verbose` reports peak per-face owner demand, the input to raising mask capacity; nothing acts on it.

**ShadowmaskAtlas ownership.** Each lightmapped id-17 face keeps up to four owner lights, one per mask channel. A light that does not own a face never reads a channel there, so occlusion edits change stored values and owners, never another face's channels. Runtime consumption is in `rendering_pipeline.md` §4 (Static specular ownership).
- **Eligibility.** Every non-dynamic, non-animated, non-bake-only `static_light_map` light, directional included. Id 40 stays the promotion set only.
- **Owner-index cap.** More static spec lights than a `u16` index below the `0xFFFF` sentinel can name is a named compile error, checked as soon as the light namespace is built, before any bake stage; the fused stage repeats the check as a guard.
- **Admission.** A light competes on a face only where its quantized baked visibility is non-zero on a chart-interior texel. Padding never admits, and non-finite visibility neither admits nor contributes.
- **Ranking.** At most four owners per face:
  1. Candidates above a lit-texel floor (16 texels), measured from the candidate's own texels, rank ahead of those below it.
  2. Above the floor, id-40 lights rank ahead of specular-only lights.
  3. Within each tier, the larger summed shadowed contribution wins.
  4. A tie goes to the lower light index.

  The result never depends on arrival order, worker count, window size or cache state.
- **Fill.** One zero-initialized raw RGBA8 bake layer is live as the fused walk consumes partitions in global light order. A per-layer texel-to-face map routes chart-interior texels. For uncut faces, per-face shadowed sums rank candidates online: a newcomer that outranks the weakest owner takes its channel after that channel's padded rect is zeroed. Each layer closes before the next opens: owners sort ascending with their planes permuted to match, padding dilates, and finished blocks encode to BC5. Only cell blocks holding a cut face's earlier sub-faces remain raw until its last sub-face layer closes; cuts sharing a block hold it until the latest such close. At that close, estimated owners that baked nothing release before sorting and encoding. The final layer closes after the streamed id-22 cache write. Both compose and lightmap-hit/shadowmask-miss walks use this lifetime. No per-(light, texel) visibility is retained, and no texel is traced twice.
- **Cut faces.** All sub-faces of one cut face share one owner set, fixed just before the walk because sub-faces span bake layers. The estimate samples the parent grid on the lattice `g ≡ 2 (mod 4)` per axis; a sub-face whose own segment holds no lattice coordinate on an axis is sampled at its centre on that axis. Each sample goes through the sub-face that owns it, and a light is traced only where its unshadowed term reaches the sample. Ranking scales lit samples and contribution by 16, one sample per floor's worth of texels. A light the estimate missed takes no channel, even a free one; an estimated owner that bakes nothing is cleared when its last sub-face layer closes. The estimate runs only on an id-42 memo miss.
- **Owned channels.** An owned channel reads zero on interior texels its owner did not reach. Its chart padding repeats the owner's edge by the lightmap's dilation rule (8-neighbour mean of the previous ring, confined to the face's own padded rect), so an edge occluded along its length pads with zero and no padding texel holds another light's value.
- **Presence.** Id 42 exists whenever any face admits an owner, whatever id 40 holds, and load never drops it with id 40.
- **Drop report.** Each above-floor light that gets no channel is warned by name with its face. A cut face is named once, as the pre-cut face with its sub-face range, and its count is the parent's exact lit-texel count after the walk. `--verbose` reports peak per-face demand. The report is identical on cold and warm builds (§Build Cache).

**KinematicGeometry (id 43):** optional section for deterministic brush movers. Version 5 carries mover identity/path, local collision/render geometry, static spin authoring, block/crush policy, optional auto-close override, named transition events, and per-mover portal associations for movers closed at their docked pose. Version 6 appends per-mover carried dynamic-light links (`AlphaLights` index plus local offset). Both additions are presentation-only: portal associations drive camera portal occlusion, while carried links resolve to local runtime entities; neither enters `CellVisibility`, non-camera coupling, or multiplayer content parity. Version 7 appends optional per-mover sound keys (open, close, blocked, crush) after the carried-light links, encoded like the named transition events; a blank KVP reads as absent. The keys are presentation-only: the mover component carries them for the app's sound drain, and they stay outside the static-content hash, the wire, and the snapshot. Versions 1–6 remain loadable; v1–v5 decode with no carried-light links, and v1–v6 decode with no sounds. The auto-close override has an explicit presence marker: absence inherits the mod default, including through a blank KVP; present zero disables it. Version 3 has its historical scalar rule: zero means absent/inherit and non-zero means authored. Version 2 defaults blocking fields and carries spin authoring; version 1 also defaults spin to zero and `carry_yaw = false`. Loader rejects non-zero degree-domain spin values that become zero in runtime radians, so pure-rotator admission and ramp semantics cannot change across the conversion boundary. Waypoint records carry `name`, `next`, and origin. Runtime hashes only deterministic prediction and collision inputs into the multiplayer static-content gate; host-only block policy and timers remain outside it. Mover brushes are not present in static `Geometry`, `Bvh`, static collision, lightmaps, SDF, portals, or navmesh.

**TriggerVolumes (id 44):** optional section for brush triggers. Each record carries a world-space AABB, activation (`touch` or `use`), target tag, closed mover-command vocabulary, fire mode, rearm delay, enabled-on-spawn state, tags, and named `on_fire`/`on_exit` events. Version 2 appends the event names; version 1 decodes both as empty. Runtime preserves direct mover commands and dispatches bound event reactions. `trigger_volume` brushes are not present in static `Geometry`, `Bvh`, static collision, lightmaps, SDF, portals, or navmesh; `switch` brushes are, and their records are otherwise indistinguishable.

**EnvironmentVolumes (id 52):** always written, so its absence identifies a PRL built before level gravity left FogVolumes. The loader checks for it before decoding FogVolumes and fails a stale file with `StaleFormatMissingSection` and a recompile message, rather than a FogVolumes layout error. Each record carries one convex leaf per authored brush (AABB plus planes; a point is inside when `dot(p, n) − d ≤ 0` for every plane) and, for a fluid, its surface faces (texture index, polygon positions and texel UVs). Decode rejects a dry volume with faces and a volume that sets no field. Map entity order is persistent: it is the overlap rule's final tiebreak. A per-record push `force` rides the optional sibling id 53, so id 52 and its version are unchanged and maps built before `force` load as they are. The runtime builds its query tree from these leaves at install, never from baked tree data (`entity_model.md` §7). The level content digest binds every record field — source entity, priority, gravity, push, fluid name and every brush plane and bound — and names two skips: surface faces (presentation) and the header's default gravity, which replicates instead (`networking.md` §Level identity and level content digest).

**PushForces (id 53):** optional sibling of id 52, paired to its records as `LightTags` pairs with AlphaLights: a `u16` version (1), a `u32` count, then one `f32` force per id-52 record in record order. The compiler writes it only when some push volume authors a force other than 1, so a map authoring no `force`, push-only maps included, emits none; absent means every force is 1. Load fails on a version other than 1, a payload not exactly `6 + 4 × count` bytes, a count unequal to id 52's record count, a negative or non-finite force, or a force other than 1 on a record without push. `-0` loads as 0. Each environment volume carries its record's force, and bodies feel the winning push times its force (`movement.md` §6). The level content digest hashes forces other than 1 (`networking.md` §Level identity and level content digest).

**ReflectionProbes (id 54):** optional; absent means no probes, and a map built before the section loads. Little-endian, `u32` counts and indices, `f32` scalars. Header: `u16` internal version (2), tile dimension, tile border (1, the SH atlas convention), colour format tag (id 22's irradiance tags; the compiler writes BC6H, the runtime also accepts RGBA16F), distance format tag (f16 inverse L1 distance, the only value), lights per probe K (4), lobe count (5), probe count, binding count. Probes run authored in map entity order, then derived in fluid entity order. Each probe: source entity (the authored brush, or a derived probe's fluid entity), box min and max in engine space, capture point, tile layer, then K share entries — animated-light roster index (`u32::MAX` marks an unused entry), RGB reference, RGB share per lobe. Bindings are (volume, face, probe) triples in id 52's numbering, ascending strictly by (volume, face). Then the colour payload, layer-major, each tile in its format; then the distance payload, layer-major, one f16 tile per probe at the same dimension and border, 0 meaning no surface. Decode rejects another version, an unknown format tag, a K or lobe count other than the engine's, more probes than the cap, a probe count of 0, a layer past the count or reused, a box whose min exceeds its max or a capture point outside it, a share outside 0..=1, an unused entry carrying values, two used entries of one probe naming the same light (its change would count twice), a binding naming a probe past the count, a face bound twice or out of order, a payload of the wrong length, and a negative or non-finite distance, −0 included. The loader adds a binding naming a volume or face past id 52's counts, a share naming a light past the animated-light roster, and a roster-to-descriptor table that is not the identity in DeltaShVolumes (id 27), whose roster the shares are numbered in, or AnimatedDirectShDeltaVolumes (id 45): the fluid pass reads descriptor `r` for roster `r`, while SH compose and animated direct compose map `r` through those tables (`rendering_pipeline.md` §10). Presentation only: the level content digest does not hash it.

**CellDrawIndex (id 37):** per-cell CSR of each cell's owned BVH-leaf spans, baked from the already-sorted global leaf array joined to the compiler cell records (a BVH leaf is drawable when `index_count > 0` and its cell is `!is_solid && face_count > 0`). Layout: `version`, `cell_count` (= `Cells.cell_count`), `span_count`, reserved, `cell_span_offset[cell_count + 1]` prefix sums, then `Span { leaf_start, leaf_count }[span_count]`. A cell touching K material buckets owns K disjoint spans (BVH leaves are bucket-sorted). **Presence rule:** emitted whenever the BVH has non-empty leaves; omitted for zero-leaf maps; independent of portal presence. The runtime cross-validates it at load and drives the candidate cull from it (`rendering_pipeline.md` §7.1); absence when required or any validation failure is a load error. Derived from the BVH stage and baked uncached, like the BVH itself.

**Cells (id 38):** runtime visibility cell records copied from the compiler's final empty/solid/exterior partition. Cells preserve compiler spatial ids one-to-one so portal endpoints, BVH leaf `cell_id`, fog masks, and diagnostics share a stable id space without a runtime remap. Each cell stores bounds, solid/exterior/drawable flags, face range summary, and a range into the flat portal-reference list. BSP split planes are not serialized here.

**CellLocator (id 39):** runtime point-to-cell locator. Version 1 encodes a validated decision tree derived from the compiler BSP, but the runtime contract is locator-to-cell, not BSP traversal. On-plane positions choose the front child. Runtime camera visibility, mesh/particle placement culling, and diagnostics use this locator through `LevelWorld::locate_cell`.

**DeltaShVolumes (id 27):** sparse CSR companion for animated-light indirect deltas. Its affinity-cell structure uses `affinity_factor = 4`, `affinity_dims = ceil(base_dims / 4)`, CSR `affinity_offsets`, and flat `affinity_lights`. Each affinity cell has its own coarsening level; each CSR entry stores octahedral delta tiles only for that cell's kept probes, by kept rank. L2 stores one synthesized valid-brick mean tile. The delta bake stores indirect-only unit-radiance transport; animated direct lighting lives in `lm_anim`, and runtime descriptors apply authored color and intensity exactly once to both compose paths. Its reach is the conservative falloff cube for every light, including spotlights, because indirect bounce is not confined to the authored cone. `DELTA_SH_VOLUMES_VERSION` is section-internal and stale sections are rejected. Compiler-side conservative dropping may omit an entry through the existing CSR absence encoding only when its composed output is within the fixed error budget; script-mutable animated entries remain present. The pre-base-SH planner and bake share this reach policy and assert exact CSR identity.

**AnimatedDirectShDeltaVolumes (id 45):** sparse CSR companion for animated direct SH. Each affinity cell has its own coarsening level; each CSR entry stores tiles only for that cell's kept probes, by kept rank. L2 stores one synthesized valid-brick mean tile. Immutable spotlights use the same conservative cone-aware direct reach as ids 35 and 41. Script-mutable descriptor slots deliberately retain cube reach because later curve replacement can make an at-rest zero record nonzero; the existing zero-drop policy keeps those records present. An all-empty result is omitted rather than emitted as an empty section. Script membership is applied before planning, and the planner and bake assert exact CSR identity.

**Direct affinity reach.** Direct-SH consumers (ids 35, 41, and immutable id-45 entries) first intersect the falloff reach with a conservative spotlight-cone bound. The test encloses an affinity-cell AABB in a sphere and evaluates it in the same `f32` coordinate domain as the bake, with outward angular tolerance. It may retain a false positive, but it must never reject a cell containing a contributing probe. Directional and point lights keep their prior reach, and any degenerate or non-finite light/cell state conservatively keeps the cell. Reach changes affect every cache consumer of the resulting direct CSR: the DirectShVolume (id 35) stage and static billboard direct scatter. Their independently owned stage epochs must advance together. The id-41 and id-45 delta bakes cache nothing, so a reach change needs no epoch bump for them.

### Ray traversal

Every bake ray query walks the compiler BVH through `ray_traversal::BoundedRay` with the stock iterator. Each node test is one slab pass: the stock ray-box test, minus boxes that start clearly beyond a distance bound. Closest-hit queries shrink the bound to the nearest hit kept; occlusion queries fix it at the segment end. Visit order and the tie winner are unchanged, so the node test changes no output byte. A box is pruned only when it starts beyond the bound by a slack. Byte identity is verified by the fixture digest gate and full-scan parity tests, not proven for every float input: the triangle test's distance error grows for grazing rays and near a sliver triangle's ends, so a segment end or finite bound within that error of such a hit is the open case. A new bake ray query uses `BoundedRay`, not a bare traversal.

**Occluder cache.** The lightmap layer bake walks one `(light, chart)` on one thread and tests the last occluding triangle first. Neighbouring texels mostly share an occluder, so most shadowed rays skip traversal. The cached triangle uses the traversal's ray and hit predicate, so it can only turn a clear answer into a blocked one, on a real hit. The stock box test can reject, by rounding, a box whose triangle the ray hits on the box's boundary (an axis-aligned edge or a vertex); the cache catches that leak. Cache state never crosses threads. The frozen monolithic oracle keeps one cache per light per chart, walked in texel order, so the layered composite still matches it bit-for-bit.

### Runtime visibility

Portal traversal normally computes visibility: per-frame flood-fill from the camera cell with frustum narrowing at each portal. Its visible set is a conservative superset of the exact portal set; a per-cell bounded-region walk is decided, not yet built (`rendering_pipeline.md` §2). Solid-cell, exterior-camera, and no-portals cases fall back to per-cell AABB frustum culling; a walk that exceeds its step budget falls back too, but stays bounded — bounded frustum-culled sets drive drawing and fog reach. `CollisionWorld` remains the physics source of truth; cells and portals do not answer collision contacts. See `rendering_pipeline.md` §2.

---

## Navigation bake

Walkable navigation is baked offline into a `NavMesh` PRL section (SectionId 36). Baked after the BVH stage and before the lightmap bake; the runtime loader decodes it. Runtime pathfinding traverses regions with A* and follows the resulting portal sequence with funnel string-pulling.

**Query contract.** Runtime consumes convex walkable **regions joined by portals** — the pathfinding query surface (A* over regions, funnel over portal segments). The shape is the durable contract; the bake *algorithm* swaps behind it (rectangular decomposition first, a contour tracer later). Off-mesh links and hints (jump links, cover) extend it as future portal kinds / region attachments — additive, no format break. Seed of a broader baked spatial-AI layer.

**Navmesh ↔ collision.** The bake reads the same surface the collision trimesh uses, so it never marks walkable what collision rejects. The navmesh bakes from pre-cut triangles; collision reads the oversize-face cut's coplanar sub-faces. The navmesh **routes** (which regions connect); `CollisionWorld` owns ground height and final movement (agents sweep real collision along a region path).

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

**Participating stages.** Lightmap bake and SH volume bake, plus the ShadowmaskAtlas memo, the indirect SH delta bake, CellVisibility, CellResidencySet, ChunkLightList, static and animated billboard direct-scatter, animated-light weight-map, SDF-atlas, navmesh and reflection-probe capture stages. Parse, BSP, portals, geometry, and BVH run uncached — they are fast enough that caching yields no measurable speedup. So do the direct and animated-direct SH delta bakes: their per-entry sub-blocks cost more to cache, in per-file I/O, than to compute.

**Cache grain.** Several channels are cached below whole-stage grain, so editing one light refreshes only the affected entries:

- **Lightmap — per-light, per-atlas-layer partitions.** Each static light's contribution for one atlas array layer is a separate `"lightmap_layer"` entry, keyed on that light's params, its influence-bounded geometry slice, density/sample-count, atlas layout, and target atlas layer. A partition header names the target layer; its ordered 8-byte records hold only a reached texel index and raw soft visibility. Unreached texels are absent; zero and NaN visibility remain present. The payload is also the ShadowmaskAtlas raw-mask source. `LAYER_FORMAT_VERSION` covers the partition payload. Both warm and cold builds fold partitions in global light order out of the fused walk's bounded light window (§Compiler pipeline step 12). The fold analytically reconstructs irradiance and weighted direction from the unshadowed Lambert term and visibility, derives coverage from record presence, normalizes once per covered texel, then dilates/encodes/appends that layer. This reproduces the uncached reference output byte-for-byte before BC6H. Exact in both warm and cold builds.
- **Lightmap — composited section (second level).** A `"lightmap_section"` entry memoizes the encoded `Lightmap` (id 22) section itself, keyed on the prepared atlas layout, ordered per-light layer fingerprints, and encode parameters (texel density, irradiance format, direction texel scale). The direct atlas-layout input keeps the empty-fold all-Sdf fallback dimension-sensitive. Decoded hits are also checked against current atlas and encode metadata before use. A no-edit rebuild hits it and skips reading the layers, compositing, and BC6H-encoding entirely — the per-light layers are the recompose fallback when any light, geometry, or atlas input changes. Pure memoization of an already-exact pipeline output, so it cannot perturb byte-identity; warm-only, like the layers.
- **ShadowmaskAtlas — owner section (second level).** A `"shadowmask_atlas"` entry memoizes the encoded `ShadowmaskAtlas` (id 42) section with its drop report and peak per-face demand, so a hit logs the same warnings and `--verbose` peak as a cold build. The key covers the stage version, `LAYER_FORMAT_VERSION`, the lit-texel floor, the id-17 face count, every eligible light's compact spec index, id-40 membership and `"lightmap_layer"` fingerprints, the atlas dimensions and layer count, and the cut-face estimate's parameters (stride, sampling mode, cut map). Spec indices are keyed because an earlier unownable static light (SDF-shadowed or animated) shifts them without touching any partition. The memo probe runs after the owner-index cap check and before the cut-face estimate and the lightmap memo, so a hit traces nothing. On a miss the fill runs inside the fused walk. A lightmap hit plus shadowmask miss reads each eligible partition at most once per atlas layer into one raw fill, with no second visibility trace; a one-light edit re-bakes only that light's partitions. Warm-only, like the lightmap section memo. Cold `--no-cache`/`--release` recomputes through the same fused walk.
- **SH — per-probe-group entries.** The probe grid is partitioned into 4³-probe groups; each is a `"sh_group"` entry baked over its probe subset with a *bounded reaching-light set* (`falloff_range` dilated by a finite reach cutoff), then assembled (byte-copy placement) into the volume. Bounding the light set is what localizes a light edit; it also makes warm SH a benign approximation (out-of-reach lights drop — dimmer-or-equal, never miscolored). The soft-visibility sample-lattice seed mixes each light's **global** `static_lights` index (not its position in the bounded slice), so a kept light gets the same rotation whether the bake sees the full set (cold) or the bounded set (warm) — that is what makes "dimmer-or-equal, never brighter" hold strictly. The cold `--no-cache` path runs the exact whole-volume bake instead. SH rays trace full geometry, so any geometry edit re-bakes every group.
- **Indirect SH delta bake — per CSR entry.** The indirect delta bake caches each `(affinity cell, light)` sub-block, so editing one light re-bakes only its entries. The direct and animated-direct delta bakes share its entry code but pass no cache, so they key nothing at runtime. Unlike the approximate warm base-SH volume, delta caching is **exact** — each entry bakes a single light in isolation, and its soft-visibility noise seeds from stable cell and probe coordinates already folded into the entry key, with no light-global seed axis. A bounded light set therefore never changes a kept entry's payload. Byte-identical warm and cold, through the full downstream post-bake pipeline (drop, coarsening, valid-probe compaction, payload cap).
- **CellVisibility, CellResidencySet, ChunkLightList — whole-section.** Memoized per section, not per element. CellVisibility keys on BSP/portal structure and no lights, so a light-only edit hits. CellResidencySet keys on the Cells fields the walk reads (bounds, flags, portal-ref ranges and portal refs; face ranges stay out, since the walk reads drawability from the flags), the encoded Portals and CellLocator bytes, the Teleporters section bytes and, when it has records, the final BSP leaf bounds and half-spaces, the stage version (3), the maximum lead, and `postretro_visibility::PORTAL_WALK_EPOCH`, so light, chart, density and hint edits hit, a teleporter edit re-bakes, and a walk change that alters any visible set (which must bump the epoch) re-bakes; the loader-world check (usable portals) reruns before every lookup. ChunkLightList keys on geometry plus the static light set, so a dynamic-light-only edit hits.
- **SDF atlas — whole-section.** Keyed on every vertex position, referenced or not (the world AABB spans them all), the index buffer in order, BSP node planes and leaf solidity, and the voxel and brick sizes. Lightmap UVs, lightmap and animated block ids, texture UVs, normals, tangents, face metadata and texture names stay out, so density, scale-region, seam and animated-light edits hit. The SDF bakes the geometry as it stood before the oversize-face cut, so a density edit that moves a cut hits too. An edit that moves a vertex, changes a triangle or changes solidity re-bakes, including a brush whose faces are all culled.

**Streaming-seam edits re-bake lighting.** Lightmap blocks are stored cluster-major over the canonical cell partition, which seam portals split, so an edit to `streaming_seam_volume` brushes alone can reorder blocks and miss the lightmap layers and section memo, the ShadowmaskAtlas memo and the animated weight maps. The keys are correct: each key covers block order. The SDF atlas key does not, since the SDF bake never reads block ids.

**Warm vs cold builds (dev-default / release-on-purpose).** The interactive default is a *warm* (cached) build: fast iteration, exact direct lightmap, approximate indirect SH. The `--release` flag selects the *cold* exact build — every stage baked exact — and is the only artifact a final map should ship from. `--release` is the intent-named ship mode; mechanically it bypasses the cache exactly like `--no-cache` (it implies `--no-cache`; passing both is fine and identical). A warm build trades exactness for speed and is not shippable. The split is per channel. The direct lightmap is exact in both modes: a cached lightmap is byte-identical to a full bake (pre-compression). Indirect SH is exact only in a release/cold build. A warm build bakes SH at a finer-than-whole-volume grain, bounding each region's light set — a benign approximation, dimmer-or-equal in far-bounce regions, never miscolored. A warm build emits a one-line warning naming `--release` as the ship flag. Judge final indirect lighting on a release build. Run production and release bakes with `--release` (or `--no-cache`).

**Budget observability.** Hit, miss, and eviction lines are info-level, and the compiler's default filter is `warn`, so a cache thrashing against its budget presents as an unexplained slow build. Each cached build tracks a deduplicated set of entries it successfully read or wrote. It emits exactly one end-of-build warning when the total spared set (every live map's record plus this build's set; see Eviction) exceeds `--cache-max-size`, naming that total and the budget. The prune keeps the spared set whole, so the cache then outgrows its budget rather than evicting work. Repeated touches and write-then-read paths count once per entry. `--no-cache` and `--release` emit no cache-budget warning.

**Key composition.** `blake3(stage_id || stage_version_le_bytes || input_hash)`.

| Component | Form |
|-----------|------|
| `stage_id` | string literal — `"lightmap_layer"` (per-light), `"lightmap_section"` (composited-section memo), `"shadowmask_atlas"` (owner section memo), `"sh_group"` (per-probe-group), `"indirect_delta_sh_subblock"` (indirect SH delta, per affinity-cell/light CSR entry), `"cell_residency_set"`, `"billboard_direct_scatter"` (static scatter), `"animated_billboard_direct_scatter"` (animated scatter deltas), `"animated_lm_weight_maps"`, `"sdf_atlas"`, `"navmesh"`, or `"reflection_probes"` (whole id-54 section) |
| `stage_version` | `u32` cache epoch in each stage's module, bumped manually when that stage's algorithm or payload format changes. Each stage owns its own epoch and version-bumps independently — the per-light-layer and per-group-SH formats version separately from each other and from the legacy whole-stage bakes |
| `input_hash` | Stage-defined blake3 over serialized inputs/config — covers the data the stage reads. Postcard serialization is the common pattern; stages may use a fixed-order byte stream when that is the stage contract. |

**Stage version bump rule.** Bump a stage's epoch when its output computation changes (algorithm, sampling, formula, or atlas packing) or its key's inputs change; a key change usually misses old entries anyway, and the bump makes the invalidation explicit. The substrate invalidates every entry for that stage on the next build. Do not bump for unrelated changes. Each stage's current epoch lives as a `u32` constant in its own module — the source is authoritative; this doc does not pin the number.

**Determinism invariant.** Byte-identical output for identical inputs — with two scoped carve-outs. The guarantee holds for the direct lightmap before compression and for the cold whole-volume SH bake (the ship path). New code in `lightmap_bake.rs` or `sh_bake.rs` must preserve it. Avoid common non-determinism sources: `HashMap` iteration feeding output ordering, non-order-preserving parallel reductions. **Exempt:** (1) lossy compressed output (BC6H irradiance) — correctness is round-trip within tolerance, not byte-equality; (2) indirect SH baked finer than the whole volume (warm incremental builds) — a deliberate bounded approximation; the cold whole-volume bake stays exact. Either way the cache stays correct: it keys on inputs, not outputs. Every bake is self-consistent — same inputs, same bytes.

**CLI flags.**

| Flag | Effect |
|------|--------|
| `--cache-dir <PATH>` | Use a custom cache directory instead of `.build-caches/prl-cache/` at the workspace root |
| `--cache-max-size <SIZE>` | Size budget for the cache, swept at build start (default 2 GiB). The sweep keeps every map's record and evicts the rest, oldest first, down to it; see Eviction. Accepts a byte count or a binary-unit suffix (`2GiB`, `512MiB`, `1.5GiB`) |
| `--sh-delta-max-size <SIZE>` | Post-drop raw-payload cap for sparse SH delta sections ids 27, 41, and 45 (default 256 MiB). It accepts the same byte syntax as `--cache-max-size`; an overage is a named compiler error before packing, never an automatic quality reduction. **64 MiB is an authoring diagnostic target: the compiler emits a non-failing post-compaction warning above it, with per-section and aggregate bytes; it is not the default cap or a rejection threshold.** Compiler-only: no map KVP or runtime representation. |
| `--working-set-max-size <SIZE>` | Per-stage host-RAM working-set ceiling (default 16 GiB): no single stage's predicted working set may exceed it. Not a whole-process RSS bound; earlier stages' outputs stay resident. Two gates read it, each with its own estimator: the lightmap stage (step 12) and the SH-delta gate. The SH-delta gate projects the pre-base-SH dense ids 27, 41, and 45 working set; its production planner applies data-script membership, static-light selection, and each transport's current reach policy before any SH payload is materialized; plan/bake CSR identity is asserted for all three sections. A zero-byte budget is a cheap real-map projection that intentionally refuses before base-SH rays. The SH-delta gate charges 3× cumulative dense bytes normally: 2× for dense-plus-compaction residency and one share reserved for co-resident base-volume copies. Coarsened `--sh-analyze` charges 4× because it retains another dense delta clone. Values use the same byte syntax as `--cache-max-size`; finite values above `u64` are rejected. Compiler-only. |
| `--no-cache` | Disable the cache entirely — neither read nor write, no directory created (no prune either) |
| `--release` | Produce a shippable map: the exact ship path (exact fused direct-lightmap/shadowmask bake + exact whole-volume SH). Intent-named ship mode; implies `--no-cache` (passing both is fine and identical). The interactive default is a warm build — ship only `--release` artifacts. |
| `--soft-shadow-samples <N>` | Soft-shadow penumbra escalated full-sample count (default 32). Folds into the lightmap-layer cache key and, through eligible lights' layer fingerprints, the ShadowmaskAtlas memo key. Raising it invalidates those cache entries. It also folds into the animated weight-map stage key. Run `prl-build --help` for the full flag list. |
| `--direction-texel-scale <N>` | Per-axis static dominant-direction-atlas reduction (default 2). Must be a positive power of two no larger than 64; `1` preserves full-resolution direction bytes. It folds only into the warm composited-section key, never per-light layer keys or chart placement. |

**Entry format.** One file per entry, named by the hex key. Every write stages to a per-writer `<digest>.<pid>-<n>.tmp` stage file, then renames atomically into place. `get()` validates integrity before returning payload; mismatch is a soft failure (warning, cache miss). Durability is by verification, not sync: a killed or torn write leaves a temp file or a hash mismatch, so the next build misses, never hits wrong. Entries are not fsynced. A hit opens the entry once and never touches its mtime; use is recorded in the build's use journal instead (see Eviction).

**Eviction.** Size cap, enforced by a sweep at the start of every cached build (before the bake writes a fresh generation). The sweep spares each map's record. Per map, that is the entries its last successful build read or wrote, plus every entry later builds of that map read or wrote, until the next successful build of that map replaces the record. A killed or failed build never costs work, and building another map never exposes this map's set. A section-memo hit counts as use of the per-light partitions it summarizes. The budget (`--cache-max-size`, default 2 GiB) caps everything else: unspared entries are deleted oldest write first until the total fits or none remain, so the cache ends at most the larger of the budget and the spared set. Orphaned generations — the tail content addressing leaves behind whenever an input changes — fall out of every record and age out. Records retire with their maps: each map's record directory stores the map's canonical path, and the sweep stops sparing a record whose map file no longer exists, so its entries age out like any other. A record whose map path is missing is still spared until the path has been missing for at least 24 hours (tracked by a `missing-since` marker in the record directory) and no build of that map is running; then it retires and the retirement is logged. A prune that sees the map present clears the marker, so the 24 hours count from the first prune that saw it missing. The end-of-build budget report only reads records; it never retires them, and if it cannot read them it warns and counts only this build's entries. If the records cannot be listed, or any record cannot be read, the sweep skips eviction for that build and warns; the cache may stay over budget until a later build can read every record, and a persistently unreadable record keeps eviction off until it is removed. Stage files (see Entry format) are never evicted as entries: the sweep deletes one older than 24 hours, debris from a killed build, and counts younger ones, live writes, toward the directory total.

Records live under `records/<map>/` in the cache directory, `<map>` a digest of the input path. Each build appends every entry it reads or writes to its own journal as it goes, so a killed build's touches persist; a successful build writes its set as the map's `last-success` record and deletes the journals it supersedes: those that existed when it began and are no longer live, plus its own. A running build's journal is kept; live journals are marked by an OS file lock on a sidecar lock file. The sweep reads every map's record before the build starts its own journal. The sweep is off the bake path (one directory listing, the records, a few unlinks) and best-effort: any I/O error is logged and the build proceeds. `--no-cache`/`--release` skip the cache (and the sweep) entirely. A corrupted entry is still discarded as a cache miss without touching other entries. The cache remains safe to delete manually at any time.

---

## Baked texture mips

Per-texture mip-chain sidecars are **runtime-required compiled output** living under the top-level `baked/materials/` tree — not in the disposable `.build-caches/` stage cache (see §Build Cache). prl-build writes them; the engine reads them at level load.

**`.prm` files.** Each sidecar bundles up to four material slots — diffuse,
specular, normal, and emissive — each optional. Multi-slot filenames use the
canonical bundle hash over slot mask and every present slot's raw PNG bytes.
A `{name}_h.png` height sibling folds into that hash as an additional tagged
input **only when present** — when absent nothing at all is folded in, so every
bundle authored before height existed keeps its address and does not rebake.
A diffuse-only bundle uses `blake3(diffuse PNG content)` so world and model
diffuse-only materials share one sidecar. Other single-slot bundles use
`blake3(tag_byte || PNG content)`; the tag distinguishes specular, normal, and
emissive. Two bundles with the same diffuse but different siblings therefore
have distinct runtime-loadable filenames. Stored at
`<baked root>/materials/<hex>.prm`. Cross-mod dedupe is intended:
identical complete bundles produce the same `.prm` regardless of which mod
authored them.

**Root derivation.** Compiler and runtime reach the materials tree by two
different relative routes. prl-build walks up from the map source to the nearest
`Cargo.toml` ancestor and appends `baked/materials`. The engine derives the root
from the loaded content root's grandparent plus `baked/materials`. The two
routes agree while the mod root is `content/<mod>` under the tree root. A mod
nested deeper or shallower would send the runtime derivation to a directory the
sidecars are not in, and every world material would degrade to a placeholder
with a warning rather than a failure. That is why a mod is selected by **name,
never by path**: the engine's `--mod <name>`, the manifest's `mod = "<name>"`,
and `postretro-tool mint-identity <name>` all place the mod at
`content/<name>` themselves and refuse a value holding a separator, so no
name can produce another shape. (A bare engine launch with no `--mod` still
derives its content root from the map path's grandparent — the checkout's dev
bypass, which `xtask run` uses; every tool launch and launcher passes `--mod`.)
It is also what lets a packaged tree
(§Distribution packaging) carry its own `baked/materials` beside its own
`content/`.

The walks stay deliberately distinct — collapsing them into one implementation
would break shipping layouts, where no `Cargo.toml` exists at all — so the one
thing that makes them agree outside those two layouts is an explicit override.
**`--baked-root <dir>` on both `prl-build` and `postretro`** names the directory
that *contains* `materials/`, and each binary applies it in front of its own
walk. Absent, both resolve exactly what they resolved before it existed. The
invariant is cross-binary, not per-binary: given one directory, the path the
compiler writes must equal the path the engine reads, and passing the flag to
only one of the two reproduces the placeholder degradation it exists to prevent.
That is why the flags are never typed by hand in the intended workflow — the
tool that owns the project manifest passes both (§Distribution packaging).

This is what lets game content live in a developer's own version-controlled
repository rather than inside an engine installation: such a repository has no
`Cargo.toml`, so without the override prl-build falls back to
`<map parent>/baked/materials` while the engine reads the install's tree, and
the two never meet.

> **Value-taking engine flags.** Every one must also join the skip list
> in `resolve_map_path` (`crates/postretro/src/startup/session.rs`). That scan
> treats the first non-flag argument as the map path, so a flag it does not know
> about leaves its *value* exposed — `postretro --baked-root <dir>` with no map
> silently loads `<dir>` as the level.

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

**Surface map (specular slot).** The specular slot has two forms. Without a
`{name}_h.png` sibling it is single-channel `R8Unorm` (wire tag 2), exactly as
before. With one it is two-channel `Rg8Unorm` (wire tag 4): R specular (0 when
`_s.png` is absent), G baked as `255 - height` from the author's mid-gray-centered
height map. The stored byte carries no zero point: the runtime re-centers it on
128 (`resource_management.md` §4.6). Keep it that way. The bundle hash covers
source PNG bytes, not the bake transform, so changing what a stored byte means
at bake time would silently reuse stale sidecars unless `STAGE_VERSION` bumps.
`STAGE_VERSION` stays 3 —
the widening is additive and every pre-existing `.prm` parses unchanged — and
`PrmSlots` bits 4-7 stay reserved: height is not a fifth slot. The SPECULAR
slot-mask bit is set if either sibling is present. See
`resource_management.md` §4.6.

**Byte accounting.** The `TextureMips` stage reports the texture memory a level
costs, at `info` level: a grand total, a per-slot breakdown, a per-mip-level
breakdown, and the largest few material bundles. The accounting primitive is
`postretro-level-format::prm_accounting` — per material, per slot, per mip,
derived from each slot's declared `(format, width, height, level_count)`
rather than from payload bytes, so it needs no wire-format change. (The
compiler's route to it does read the file: the model and sprite bakes hand back
only a cache key, so the stage reads the `.prm` back through
`PrmFile::from_bytes_partial` to recover its declared headers. That read is
cheap next to the bake and keeps the report honest about what landed on disk.)
It lives in `level-format` rather than beside `render-cpu`'s `level_byte_size`
because `prl-build` and the renderer sit on opposite sides of the layering and
`level-format` is the only crate below both; `level_byte_size` delegates to it
so the format → bytes table has one definition. Reporting is deterministic
(ordered by material name, never by hash-map iteration) and covers cache hits
as well as fresh bakes, so a warm and a cold build report identically. This is
accounting only — no budget, cap, or eviction — and is the intended foundation
for asset streaming's per-mip residency decisions.

**The report covers all three of the stage's bakes**, not just world material
bundles: world bundles, `prop_mesh` model sidecars (`model:<path relative to
the content root>`) and billboard sprite collections (`sprite:<collection>`).
Sprite collections are the heaviest bundles the format produces — one complete
mip chain per frame — so a level with a billboard emitter was under-reported by
far more than a rounding error while the report flushed after the world loop
alone. `bake_world_texture_mips` hands its accounting back instead
of logging it, the model and sprite bakes fold theirs in, and `prl-build` logs
one report at the stage boundary.

Three rules keep that total meaningful:

- **A bundle is charged once per cache key, never once per name.** `.prm` files
  are content-addressed, so one file is reachable under several names at once —
  two map names normalizing to one texture, two glTFs carrying byte-identical
  base-color PNGs, a diffuse-only world material at exactly the address a model
  bake derives, two sprite collections with identical frame sets. The first
  name to reach a key owns its report line, and because world bundles bake
  first, a shared address reports under its world material name.
- **A model is charged its diffuse slot only.** Model rendering binds nothing
  else, and the model bake deliberately keeps a richer world bundle already
  parked at the diffuse-only address.
- **Every way a bundle can drop out of the report warns.** An unreadable
  `.prm`, an unparseable header, and a declared slot whose payload does not
  parse each log a `warn!` naming the material and the file — a byte total that
  shrinks silently is the failure this accounting exists to rule out.

**Filtering.** Mitchell-Netravali separable filter (B = C = 1/3) in linear
space throughout. sRGB diffuse and emissive color decode via a 256-entry LUT
before filtering and re-encode via IEC 61966-2-1. Specular filters as linear
R8. Normal filters linearly then renormalises per output texel; `(0, 0, 1)`
substitutes when magnitude < 1e-4. Output is then BC5-encoded (RG channels
only; the shader reconstructs Z).

**Cache invalidation.** Multi-slot filenames cover the complete bundle; the header repeats that bundle hash for cache validation. A world-material cache hit requires a matching bundle hash and structurally valid payload for every declared slot. Truncated or corrupt declared slots trigger a full rebake and atomic overwrite (tempfile `<hex>.prm.tmp.<pid>` → `std::fs::rename`). Diffuse-only model and world bundles retain their shared diffuse-content filename without colliding with richer world bundles. A version mismatch in the header triggers rebake. To force a full retexture rebuild, delete `baked/materials/` (the next bake repopulates it; doing so leaves the runtime without world-material mips until then).

**Runtime.** Level load resolves each `TextureNamesSection` entry's blake3 key from `TextureCacheKeysSection`, opens the corresponding `.prm`, and uploads each slot's mip chain directly. A zero key (`[0u8; 32]`) substitutes per-slot placeholders silently. A corrupt or missing `.prm` substitutes per-slot placeholders and logs a `warn!`; load continues. Sampler `lod_max_clamp` is set to `mip_count - 1` per texture.

**Model textures.** `prop_mesh` model base-color textures bake the same way, content-driven from the model placements in the map — no CLI flag, mirroring how world materials follow from `TextureNames`. prl-build resolves each placed model's glTF base-color PNG(s) and bakes a diffuse-only `.prm`, content-addressed by `blake3(base-color PNG)` — byte-identical to a diffuse-only world sidecar. Richer world bundles use complete-bundle filenames and cannot be replaced by a model bake. Model rendering still consumes only the diffuse slot and substitutes neutral specular and normal placeholders. Unlike world materials, no PRL section carries model keys: the runtime content-hashes the same PNG when it loads the glTF and opens `<key>.prm` directly, so the compiler only has to make the sidecar exist. The glTF base-color path resolver is shared by runtime and compiler through the `gltf-resolve` feature of `postretro-level-format`. Missing or malformed glTF fails the whole model load, so the model is skipped. Only an unresolved, missing, or unreadable base-color PNG or material degrades to the texture placeholder. Compiler resolution and bake failures warn; compilation continues. For standalone model prep, `postretro-tool bake-model-textures <scene.gltf>` runs the same model-texture sidecar bake without compiling a map. Output stays under the project's `baked/materials/`: gitignored, regenerable, runtime-required.

---

## Distribution packaging

> **Key invariant:** a payload is a whole tree, not a bag of files. Every content path the engine resolves is joined against the process working directory, and the launcher pins that directory to the payload root.

`postretro-tool dist` assembles a folder that runs on a machine with no repository, no Rust toolchain, and no build tools. Every runtime-required artifact but the committed assets — `.prl` levels, `.prm` material mips, the mod entry script — is a build product, and a release engine produces none of them: TypeScript compilation is debug-only (`scripting.md` §8), levels live in git as `.map` sources, mips come from prl-build. Packaging is the stage that makes them together.

### The project marker

`postretro.toml` marks a directory as a Postretro project. The tool finds it by walking parents of the working directory, the way cargo finds `Cargo.toml`; `--manifest <path>` names one instead, and its parent becomes the project root. Every path the tool touches hangs off that one runtime anchor — mod root, `baked/`, `.build-caches/`, `dist/`.

The marker is what lets a content repository be a project in its own right, which is the point of the whole arrangement: a game's content lives in its author's own version-controlled tree, and the engine install is a replaceable sibling (§Baked texture mips). It carries the package name, the mod's name (`mod = "dev"`), and the optional bake recipes (§Shipped level set). The tool places the mod at `content/<mod>` — the shape §Baked texture mips requires — so the manifest names no path and a value that is not one plain directory name is refused. A recipe `source` stays a project-relative file path, since a player payload may bake from a `.map` kept outside the mod tree.

### Why the tool is not xtask

`xtask` cannot ship. Its workspace root is `env!("CARGO_MANIFEST_DIR")`, resolved at compile time, so a shipped `xtask` would carry the build machine's absolute path; every command routes through it. `postretro-tool` replaces that with runtime discovery and resolves nothing at compile time — a crate test enforces the absence of compile-time path macros, because the rule is easier to hold by checking than by remembering.

The tool compiles nothing and links no script VM. It locates the engine, `prl-build`, `scripts-build`, `mint-identity`, and a copy of itself by explicit flag, each defaulting to a conventional path beside its own executable — `bin/prl-build` beside `bin/postretro-tool` in a bundle, `target/release/prl-build` beside `target/release/postretro-tool` in a checkout. That default is what makes a bundle's tool work from any working directory: the project comes from the walk, the binaries come from the executable's own directory.

The seam is stage 1. Building release binaries needs cargo; stages 2 through 7 are pure content work and do not. `cargo run -p xtask -- dist` therefore cargo-builds the binaries and then invokes `postretro-tool dist` with their paths. Anywhere else the seam would mean either shipping cargo or splitting a stage. It also keeps `mint-identity` out of the tool's link graph: that binary pulls in the scripting runtime, so linking it would drag rquickjs and mlua into a tool that needs neither. One multicall binary rather than several, because the subcommands share the manifest parser, the containment guard, and the completion-gate machinery.

**Host builds for host.** A payload targets the machine that produced it; a Windows payload comes from a Windows host. `rquickjs-sys`, `luau0-src`, and `blake3` all compile native C/C++ through `cc`, so cross-compiling is a sysroot-and-untested-target problem rather than something a packaging command owns.

### Stage order

The stages run in this order, and the order is a contract other tooling reads:

1. **Release binaries.** `postretro`, `prl-build`, and `scripts-build` at `--release`, with no non-default features, so a payload carries no dev-tools, observability, or capture surface. This is the one stage that needs cargo, and the one stage the tool does not own: it receives the resulting paths.
2. **Entry-script bundle.** The mod root's `start-script.ts` bundles through `scripts-build` into scratch, or its `start-script.luau` copies into scratch verbatim. Exactly one of the two may be present: `dist` has no rule for picking an entry language and refuses to invent one, which is stricter than the pair the engine itself rejects at init (`scripting.md` §2).
3. **Level-set resolution.** Reads the shipped set out of the script stage 2 emitted (§Shipped level set).
4. **Model-texture bake.** Every glTF under the mod's `models/` bakes its base-color sidecars into the project's materials tree. prl-build bakes model textures only for `prop_mesh` placements (§Baked texture mips), so a mod declaring its rigs, viewmodels, and enemies in script reaches a payload with no sidecar for them and renders placeholders without failing.
5. **Payload assembly.** Deletes the payload root, then writes the engine binary, the launcher, the engine-owned `core/` tree, and the mod tree, with source-only and stale generated files excluded.
6. **Level bakes.** One `prl-build --release` per resolved level, written straight into the payload.
7. **Materials copy.** The project's materials tree copies into the payload, after every writer of it has run — assembling it earlier ships a directory that is absent or half-written, which the engine degrades to placeholders without failing.

Stages 1 through 4 write nothing into the payload root. That is what makes stage 5's delete safe, not any claim about which inputs they read: everything ahead of it is regenerable build output, so a failure before assembly leaves the previous payload byte-for-byte intact.

`--release` is the only shippable bake (§Build Cache), so stage 6 supplies that flag and a manifest recipe may not. Stage 6 also supplies `--baked-root` and `--cache-dir` together: the first keeps the compiler writing `.prm` sidecars where stage 7 will find them, the second keeps the disposable stage cache out of the author's `maps/` directory, whose own fallback would put it there. Bakes run one at a time — prl-build already saturates the machine internally, and a second concurrent bake multiplies peak bake memory — ordered by ascending effective lightmap density with ties broken lexicographically by output path. That order is a cheap proxy for descending peak memory, chosen to fail an over-large bake early; it is not derived from the memory model.

### Payload layout

A payload reproduces the tree the engine expects, rooted at the payload directory instead of the project:

```
<payload>/
  postretro[.exe]              engine binary
  <package name>.{bat,sh}      launcher: pins cwd to its own directory, passes --mod <mod> --app-name <package name>
  core/                        engine assets: UI descriptors, splash, font licences
  content/<mod>/               the developer's mod, published under its declared root
  baked/materials/             .prm sidecars
```

**A distribution publishes the developer's mod at `content/<mod>` for the mod their `postretro.toml` declares**, keeping that name rather than renaming it — and the launcher mounts that same mod by name. `base` is the recommended convention for a downstream game; the engine's own `dev` is what a distribution of this workspace keeps. Because the manifest names only the mod and the tool places it under `content/`, the runtime grandparent derivation resolves `<payload>/baked/materials` (§Baked texture mips) for whatever the project named — so the name is honored without special-casing. Honoring it also aligns the published tree with the engine's own bare-launch default (`content/dev`): a payload started without its launcher finds its mod where the engine already looks. Level paths stay mod-root-relative throughout, so nothing expressed against the source mod root — the scanned `maps/<name>.prl` literals, the completion marker's lines, the runtime catalog — changes with the root's name. This is a destination-path parameter at assembly, not a redesign of the stages.

`core/` holds what the engine owns and a mounted game never replaces, so it sits outside `content/` and `--mod` never redirects it. It is deliberately one path component, which keeps it outside the two-component shape a mod root must have (§Baked texture mips) — engine assets are not a mod. Stage 5 copies it from the **install root**, never from the project: a game repository carries no engine assets, and a project that happens to hold a directory of that name does not get to stand in for the engine's. The install root is named by `--install-root <dir>` or derived from the tool's own executable location; a checkout is the one layout that derivation cannot see, so `xtask` names the workspace explicitly.

Content paths resolve cwd-relative (`ui.md` §5), so the payload is correct only as a whole tree with the working directory pinned to its root. Flattening it, or moving `core/`, breaks paths the engine hardcodes. The launcher pins that directory rather than trusting the caller's, so a shortcut or a launch from elsewhere resolves the same, and passes no map argument, so the mod's frontend drives the first screen (`boot_sequence.md` §1).

### Authoring launch

`postretro-tool run` discovers the project and launches the engine with `--mod`, `--app-name`, `--baked-root`, and `--core-root` already correct, with the working directory pinned to the project root. Flags the caller supplies win outright rather than being shadowed, since the engine reads the first occurrence of each. Because `--mod` is always present, a map argument forwarded to the engine is relative to the mod folder (`postretro-tool run maps/arena.prl`) — the engine's rule, not a rewrite in the tool, which never inspects the arguments it forwards.

Content and engine assets are two independent lookups here, exactly as they are for the packaging commands: `--mod` and `--baked-root` come from the project, `--core-root` is the install's `core/`, and neither falls back to the other. `--core-root` is passed absolute, because the engine would otherwise resolve it against the working directory this command pins to the project — the one directory that does not hold `core/`. An install carrying no `core/` refuses the launch rather than starting an engine whose pause menu, frontend menu, on-screen keyboard and splash are all absent behind warnings; `--install-root` names the install when the tool's own location cannot imply it, which is the checkout case (`xtask run` is the checkout's launcher).

The engine learns nothing about `postretro.toml` — it keeps plain flags, so changing the manifest schema stays a tool change. What the command removes is the three paths a modder would otherwise type on every launch, none of whose failure modes is an error: a wrong materials root is the silent placeholder degradation §Baked texture mips describes, and an unreachable `core/` is four warnings and four missing surfaces. The tool has no single-level build, so a direct `prl-build` invocation is the one place an author still supplies `--baked-root` and `--cache-dir` by hand.

### Player-data directory

The package name is a game's player-data identity. Every launch that knows its project — the payload launcher, `postretro-tool run`, an SDK bundle's launcher — passes `--app-name <package name>`, and the engine resolves its config and data directories from it at boot stage 1 (`boot_sequence.md` §1) through `ProjectDirs::from`, so each platform normalizes the name its own way: macOS turns spaces into `-`, Linux lowercases and drops whitespace, and case-insensitive file systems fold case. A bare engine launch, `cargo run -p xtask -- run` included, uses `postretro`. The flag is the whole interface: the engine still learns nothing about `postretro.toml`. An authoring run writes where the shipped game writes, so an author sees their players' first launch and saves.

**One resolution, one chokepoint.** Config and data directories resolve together, once, in one engine module — the only `ProjectDirs::from` call in the workspace, held there by a source-scan test. `state.json`'s per-player rows are keyed by the claim id derived from `settings.toml`'s `player_id` (`player_options.md` §2), so split resolutions would orphan every per-player save. `postretro-sim` receives the data directory and resolves none of its own.

**The tool refuses what the engine refuses.** `dist`, `sdk-dist`, and `run` reject at manifest read a package name the engine's `--app-name` rule rejects, so no launcher ships that fails at boot. One shared case table holds both checks to the same verdict per name. An SDK bundle is its own project — its marker names the package `<package>-sdk` — so a modder's runs never touch the installed game's saves, and a derivative built from the bundle does not collide with the original by default. From a game's first release, renaming `[package].name` strands its players' settings, `player_id` and saves.

### Output-root containment

Every payload root lies strictly under `<project>/dist/`, the project being the marker's directory. A guard proves it before the first build and again immediately before the delete, since the filesystem can change under a multi-minute build. It refuses on the first of these to trip:

- **Containment.** The payload root is not strictly under `<project>/dist/`. Comparison is component-wise over canonicalized paths, never a string prefix, so a sibling named `dist-old` is outside, and a symlink under `dist/` that resolves elsewhere is outside, while a project reached through a symlink keeps its own `dist/` usable. A path that cannot be canonicalized fails here: containment is exactly what the failure leaves unproven.
- **Directory.** The payload root exists and is not a directory in its own right. Assembly renames, removes, and writes into that path as a directory throughout, so the check reads the root's own metadata rather than following it, and a symlink is refused whatever it resolves to.
- **Provenance.** The payload root exists, is non-empty, and holds at its top level neither the completion marker nor the engine binary. A mistyped output path under `dist/` therefore cannot delete a directory no `dist` run produced. The states this permits: absent; empty, which is what a kill between the root's creation and the marker's write leaves; marker-bearing, a run stopped mid-payload; binary-bearing, a completed run whose epilogue removed the marker.

**Provenance, not containment, carries the safety argument.** In the engine's own repository `<project>/dist` is gitignored and holds no committed input, but an arbitrary content repository makes no such promise. Containment now bounds the blast radius to one directory the project owns; the provenance check is what proves a distribution run produced what is about to be deleted.

In the engine's own workspace, the cargo target directory is refused at or under `dist/` before stage 1 builds anything: `<target-dir>/release` holds the engine binary at its top level, so provenance would read it as a payload root and permit a delete of the binaries stages 5 and 6 read.

### Completion gate

A payload root carries `.dist-incomplete` while a run is still working in it:

- Written into the payload root before any other content enters it, through a temp file outside the root renamed into place.
- Rewritten after each level bake, and written into a partial tree before that tree is restored after a failed delete.
- Removed only after a sweep over the finished payload passes.

Its first line names the stage the run is attempting; every line after it is one resolved level not yet baked, in bake order, written as the mod-root-relative `maps/<name>.prl` path. The file therefore always holds at least its status line, so a truncated write is detectable.

The gate is one-sided. A directory holding content and **no** marker was produced by a completed, swept run. A directory holding one is not known complete. The single uncovered window is a kill between the root's creation and the marker's write, which leaves an empty directory behind.

### Publication locks

`prl-build` takes a `.<name>.prl.pack.lock` beside every pack it writes and leaves the file behind on purpose: unlinking it would let a waiter keep the old inode open while the next compiler creates and locks a fresh one, so `crates/level-compiler/src/pack.rs` is not where this is fixed. The consequence is that locks reach a distribution from both directions — stage 5 copies a mod tree that already holds the author's, and stage 6 bakes new ones straight into the payload after the copy filter has run.

Packaging removes them. Both outputs delete every lock in the finished tree before the sweep, and both sweeps refuse one that survived, which is what stops a later change reintroducing them. Nothing recompiles a shipped `.prl` in place, and a modder who rebakes inside an SDK bundle recreates the lock on demand, so removal costs nothing. The refusal matches on the file's name rather than its extension: `.prl.pack.lock` is a compound suffix behind a leading dot, and the single extension a path API reports for it is a bare `lock` — too narrow to identify these files and too broad to be safe.

Compiler scratch is the other build leftover: `prl-build`'s `.<name>.prl.pack-*.tmp` and `.<name>.prl.cluster-sh-*.spool`, and the texture baker's `<name>.tmp.<pid>`, each renamed into place or deleted on success. Only a killed bake strands one, and it strands it in the author's tree, so both copy filters skip these exact shapes and both sweeps refuse them — matched by name for the same reason as the lock, since a bare `tmp` extension is one a mod could legitimately use.

### Shipped level set

The levels a payload ships are the `maps/<name>.prl` string literals in the entry script stage 2 emitted, found by a textual scan. The mod's catalog is the record of which levels the mod offers, and the emitted bundle carries every catalog `path` as a literal; the maps directory holds fixtures, feature demos, and capture rigs under the same extension as the sources behind offered levels, and tells them apart by nothing.

That scan is the whole of the visibility. A catalog path assembled at runtime rather than written as a literal ships nothing and reports nothing — no stage ever saw the level, so no check fires and the recipient gets a menu button that loads nothing. The exposure is accepted, not mitigated.

Each resolved level bakes from `content/<mod>/maps/<stem>.map` at default flags. A manifest `[[recipes]]` entry exists for a level whose source or compiler flags that default cannot infer; it is keyed by output path, and a recipe matching no scanned literal is reported as an orphan rather than passing silently.

A recipe may not supply the arguments the tool owns. `-o`, `--release`, and the TUI switches decide what kind of bake a distribution runs. `--baked-root` and `--cache-dir` are the pair that must stay consistent across the compiler and the engine: prl-build reads the last occurrence of each, so a recipe naming one would win and reintroduce exactly the degradation the flags exist to close.

### SDK bundle (modder distribution)

Distribution has two outputs under `dist/`: the player payload (`dist`, above) and the modder SDK bundle (`postretro-tool sdk-dist`). Both bake the maps — the SDK bundle is **content-complete**: it runs the same level and material bakes as the player payload, so it is playable on arrival, and *additionally* ships what authoring needs. The invariant that separates them is subtraction, not baking: the player payload carries only released runtime artifacts, while the SDK bundle is a superset that also carries the compilers, the tool, the SDK, the docs, and the mod's `.map`/`.ts` sources beside the baked output. Its sweep is correspondingly lighter — it confirms the required entries exist rather than forbidding sources.

**The bundle is a project in its own right.** It carries a generated `postretro.toml` naming the source project's own mod (`dev` for this workspace) and the package `<package name>-sdk` (§Player-data directory), so its recipient can produce a player payload from it with no repository, no Rust toolchain, and no cargo. That is the whole reason for shipping the tool, and it is what makes the bundle's own `dist` run the real acceptance for this machinery: every workspace test has a `Cargo.toml` ancestor and a cargo binary, which are the two things the recipient does not have.

**The SDK engine is a debug build with `--features dev-tools`, never `--release`.** One engine both plays the baked maps and authors. TS startup auto-compile and TS/Luau hot reload are gated on debug builds, not on the `dev-tools` feature — the feature only adds the debug inspector overlay, and a release engine links no TypeScript compiler at all (`scripting.md` §8), so it cannot serve an edit-and-reload authoring loop. The bundle needs both bits set: debug for the compile/hot-reload loop, the feature for the inspector.

Bundle root is `<package name>-sdk` under `dist/`, sibling to the player payload's `<package name>` root; the `-sdk` suffix is what keeps the two from colliding under the same output directory, and it is the bundle's package name, so its launcher passes `--app-name <package name>-sdk`. Beyond the baked levels, baked materials, and `core/` tree the player payload also holds, the bundle carries:

- the debug authoring engine at the bundle root, with `scripts-build` beside it because the engine's own hot-reload discovery looks there rather than in `bin/`;
- `bin/` holding `postretro-tool`, `prl-build`, `scripts-build`, `mint-identity`, and a release engine under the distinct name `postretro-release` — the authoring engine at the root owns the plain name, and a payload must carry an optimized engine;
- `sdk/`, `docs/`, and `tools/`, copied from the install root alongside `core/` — all four are engine-owned, and the project is never consulted for any of them;
- the mod tree whole, published under the project's declared mod root like a player payload's, with its `.map`/`.ts` sources beside the freshly baked `.prl` and the emitted entry `.js`;
- a generated `README.md` quickstart and the project marker.

Because the bundle's binaries sit where the tool's own sibling search looks, `bin/postretro-tool dist` from the bundle root needs no flags at all.

It reuses the player payload's bake stages (model-texture bake, per-level `prl-build --release`, materials copy) and its containment and completion-gate machinery (§Output-root containment, §Completion gate). The same host-builds-for-host native-toolchain constraint applies: a bundle's binaries are as host-specific as the player payload's.

**`docs/` is modder-facing only.** `sdk-dist` copies that tree verbatim into every bundle, so every command in it must be runnable by someone holding only a bundle. The engine-developer workflow — `cargo run -p xtask -- …` — lives in `CLAUDE.md`, `AGENTS.md`, and here, never in `docs/`.

---

## Non-Goals

- Runtime level compilation
- Format plugin registry — adding an input format is a code change in `format/`, not a registration surface
- WAD file support
- Runtime lightmap baking
