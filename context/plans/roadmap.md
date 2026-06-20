# Implementation Roadmap

> **Lifecycle:** reviewed and updated at the start of each milestone. Deleted when all milestones are complete.
> **Purpose:** milestone-by-milestone plan from "wgpu window exists" through a moddable, playable game. Each milestone produces something visible and testable.
> **Sequencing:** Milestones 1–9 built in order, each on the last. Milestones 10+ are parallel tracks in unrelated domains — animated enemies, advanced movement, sound, UI, the behavior-IR foundation. They share the Milestone 6/7 entity-and-collision foundation but none is a prerequisite to another. Build in any order. A milestone is a *grouping of specs*, not a single linear unit: specs within a milestone ship in sequence, but the milestones themselves are not a strict prerequisite chain.
> **Related:** `context/lib/index.md`, `context/lib/rendering_pipeline.md`
> **Status markers:** `[x]` shipped and in the tree · `[ ]` not yet built · cut-after-build items keep `[x]`, strike the description, and append **✂ Cut (YYYY-MM):** with the reason — so a "done" item that no longer exists in the tree reads as such · a later-revived cut item appends **↩ Revived (YYYY-MM):** pointing to the active work.

---

## Milestone 1: BSP Loading and Wireframe ✓

- [x] Integrate qbsp crate; load a compiled BSP2 file at startup
- [x] Parse BSP geometry: vertices, edges, faces, models
- [x] Upload vertex data to wgpu buffers
- [x] Render BSP faces as wireframe (no textures, no lighting)
- [x] Minimal free-fly camera (raw winit keyboard/mouse, enough to navigate — replaced by action-mapped input in Milestone 2)
- [x] Basic PVS culling: determine camera leaf, decompress PVS, skip non-visible leaves

**Testable outcome:** fly through a BSP level in wireframe, PVS culling visibly reduces draw count. ✓

---

## Milestone 1.5: PRL Compiler and Voxel-Based Visibility ✓

- [x] PRL binary format (postretro-level-format crate): header, section table, typed sections
- [x] Level compiler (postretro-level-compiler crate): .map parsing via shambler, spatial partitioning, geometry extraction, PVS, binary output
- [x] Voxel grid: rasterize brush volumes into 3D solid/empty bitmap for spatial queries
- [x] Exterior void sealing: flood-fill from player spawn, mark unreachable empty space as solid
- [x] Spatial grid with voxel-aware cell classification: solid cells skipped, boundary cells subdivided, air cells merged into face-containing clusters
- [x] Ray-cast PVS via 3D-DDA through voxel grid (replaces BSP portal flood-fill)
- [x] Engine PRL loader: file extension dispatch, cluster-based wireframe rendering with per-cluster coloring
- [x] Visibility confidence diagnostics: --diagnostics flag, PRL confidence section, engine gradient rendering
- [x] Test maps: varied-scale rooms (gen_test_map_4.py), contract test suite (107 tests, all passing)

**Testable outcome:** compile .map → .prl, fly through in wireframe with voxel-based PVS culling. Visibility matches expectations across varied room sizes. ✓

**Status note:** superseded by the BVH + portal pipeline in Milestone 4. Voxel code remains in repo as reference.

---

## Milestone 2: Input and Frame Timing ✓

- [x] Fixed-timestep frame loop: accumulator, interpolation factor, delta-time clamping
- [x] Input subsystem: action mapping (keyboard/mouse via winit, gamepad via gilrs)
- [x] Mouse capture, sensitivity, invert-Y
- [x] Replace raw free-fly camera with action-driven camera (still no collision)
- [x] Gamepad support: analog sticks, dead zones, trigger axes

**Testable outcome:** action-driven camera navigating wireframe levels with stable frame timing. Keyboard, mouse, and gamepad all work. ✓

---

## Milestone 3: Textured World ✓

- [x] Load PNG textures at runtime, matched by texture name strings
- [x] Depth buffer and back-face culling for solid rendering
- [x] Create render pipeline: base texture with flat uniform lighting (no lightmaps yet)
- [x] Material derivation from texture name prefixes (table lookup, logged warnings for unknown prefixes)
- [x] CSG face clipping to eliminate z-fighting from overlapping brushes (PRL path).

**Testable outcome:** textured level with uniform lighting. Navigate with action-mapped input. No z-fighting. ✓

---

## Milestone 3.5: Rendering Foundation Extension ✓

- [x] **Vertex format upgrade** — packed normals and tangents per vertex (octahedral `u16 × 2` each, plus bitangent sign).
- [x] **Per-cell draw chunks** — world geometry grouped into per-portal-cell chunks with explicit AABB and index range.
- [x] **GPU-driven indirect draw path** — compute cull → `multi_draw_indexed_indirect`.

**Testable outcome:** textured level with flat ambient, navigable, rendering via GPU-driven indirect draws with portal + frustum culling. ✓

---

## Milestone 4: BVH Foundation ✓

- [x] **Compile-time BVH** — global SAH BVH over all static triangles, flattened to dense node/leaf arrays in DFS order, new `Bvh` PRL section.
- [x] **Runtime BVH traversal** — WGSL skip-index DFS traversal with visible-cell bitmask fed by portal DFS.
- [x] **Check-in gate** — visual parity with Milestone 3.5 confirmed.

**Testable outcome:** ✓ identical visual output to Milestone 3.5, rendered through a global BVH. Milestone 5 unblocked.

**Durable decisions migrated to `context/lib/`:**
- Global vs. per-region rationale → `rendering_pipeline.md` §5
- `Bvh` PRL section layout → `rendering_pipeline.md` §5 + `build_pipeline.md`
- WGSL skip-index traversal pattern → `rendering_pipeline.md` §7.1

---

## Milestone 5: Lighting Foundation ✓

- [x] **FGD light entities** — `light`, `light_spot`, `light_sun` in `assets/postretro.fgd`; canonical light format; `_bake_only` property distinguishes runtime-dynamic lights from probe-grid-only contributors.
- [x] **SH irradiance volume baker** — prl-build stage; ray-casts through the Milestone 4 BVH; SH L2 projection; validity mask.
- [x] **Direct lighting loop** — flat per-fragment light loop over runtime lights; per-type evaluation; Lambert diffuse.
- [x] **Light influence volumes** — per-light sphere bounds in PRL; runtime spatial culling; gates CSM slot assignment and SDF sphere-trace per-light activation.
- [x] ~~**CSM sun shadows** — 3 cascades, 1024², bounding-sphere fit with rotation-invariant texel snapping. Hard edges match aesthetic.~~ **✂ Cut (2026-04):** retired with the old lighting stack (alongside the SDF sphere-trace) ahead of the lighting rework; no runtime cascade pass remains in the tree.
- [x] **Runtime probe sampling** — parse SH section as 3D texture; trilinear sample in world shader for both static surfaces and dynamic entities.
- [x] **Animated SH layers** — per-light monochrome SH layers, animation descriptor + sample buffers, per-frame brightness/color curve evaluation in the fragment shader.
- [x] **Lightmaps** — per-face baked direct lighting; static surfaces sample lightmap atlas; dynamic entities fall back to probe grid.

**Testable outcome:** textured level with probe-sampled indirect, lightmapped static surfaces, CSM-driven sun shadows, and animated light layers. ✓

**Scope note:** SDF sphere-traced soft shadows and specular maps were descoped here. Specular maps later shipped; SDF was cut (2026-04) then revived (2026-05) as the static-lighting rewrite, and shipped. See `plans/done/sdf-filterable-atlas/`, `plans/done/sdf-per-light-shadows/`, `plans/done/sdf-shadow-lightmap-uv-prepass/`, `plans/done/sdf-static-occluder-shadows/`.

---

## Milestone 6: Scripting + Entity Foundation ✓

Establish the entity model and scripting layer together. Scripting and entities are co-designed from the start: the entity API is the scripting API, and most entity behaviors are written as scripts rather than Rust. This avoids the two-pass "Rust-only stabilization then bind" approach — the scripting surface constraint shapes the entity model from day one.

- [x] **Language selection** — dual-runtime approach: QuickJS (rquickjs) for TypeScript/JavaScript, Luau (mlua) for Luau. Both runtimes run side by side; scripts dispatched by extension.
- [x] **Entity model** — typed collections (spawn / query / destroy, stable numeric ID); classname registry for FGD-defined types; lifecycle (spawn, tick, destroy); parent/child relationships with transform inheritance; world-space transforms with interpolation state for the render stage.
- [x] **Event system** — typed owned events; classname- or ID-scoped subscriptions. Event types are scripting-bindable by construction (no Rust-specific types in the surface).
- [x] **Scripting runtime** — both VMs embedded; shared definition + behavior contexts; pre-warmed context pool; primitive registry (one registration installs in both runtimes and all future contexts); pooled-context isolation (QuickJS: `Object.freeze(globalThis)`; Luau: sandbox flag). See `context/lib/scripting.md`.
- [x] **Entity API bindings** — spawn / query / move / destroy; event subscribe/emit. All bindings use IDs/handles rather than Rust references; no lifetimes in the surface.
- [x] **Map entity parsing** — `.map` entity lump → typed entities at compile time, classname-keyed. Entities spawn from map data at level load.
- [x] **Hot reload** — file watcher monitors script directory; changed scripts reload on next frame drain. Debug builds only.
- [x] **Reference behaviors (script)** — ~~`RotatorDriver` and `DamageSource` written as scripts~~. **✂ Cut (2026-06):** the per-tick behavior scripts went with the live-VM removal (`plans/done/remove-live-vm/`); only their data-archetype descriptors survive (`sdk/behaviors/reference/entities.{ts,luau}`). The damage-source role is superseded by the `applyDamage` reaction primitive (M10 entity health + damage surface).
- [x] **Modder-facing API reference** — covers all bound APIs. See `docs/scripting-reference.md`.

**Testable outcome:** spawn a scripted entity from a `.map` file; confirm it ticks and emits events at the fixed tick rate. Hot-reload the script during gameplay. ~~The `DamageSource` debug entity is available for future destruction testing.~~ ✓

---

## Milestone 7: Grounded Movement ✓

Player controller with world collision, gravity, and jumping. The player is an entity. Movement behavior is scripts (TypeScript and Luau) with enforced parity; the engine exposes collision and gravity primitives. Quake-inspired grounded movement with air control as modder-configurable data parameters.

**Prerequisite:** Milestone 6 (entity model + scripting) ✓

Plans ship in this sequence:

- [x] **Scripting primitives folder** — refactor flat `primitives.rs` / `primitives_light.rs` into a `scripting/primitives/` domain folder. Prerequisite for collision and gravity plans. `context/plans/done/scripting-primitives-folder/`
- [x] **Mod script layer** — mod-level script execution layer that runs before any level loads. Player entity types declared here; prerequisite for player spawn. `context/plans/done/M7--mod-script-layer/`
- [x] **Collision foundation** — parry3d dependency; `CollisionWorld` backed by PRL static geometry trimesh; Rust-owned, not script-visible. `context/plans/done/M7--collision-foundation/`
- [x] **Gravity primitives** — `initialGravity` worldspawn KVP; `world.getGravity()` / `world.setGravity()` behavior-scope primitives; SDK and docs updated. Depends on scripting primitives folder. `context/plans/done/M7--gravity-primitives/`
- [x] **Player spawn** — `player_spawn` FGD entry with `entity_class` KVP; level load spawns player entities from it. Depends on mod script layer.
- [x] **Movement scripts** — TypeScript and Luau reference movement scripts with full feature parity (gravity, wall slide, step-up, jump, strafe, air control); contract test asserts matching output. Depends on collision foundation, gravity primitives, player spawn. `context/plans/done/M7--movement-scripts/`

**Testable outcome:** player walks through a PRL level with full collision response — no clipping, wall slide, step-up, jump. Modder can edit and hot-reload the movement script in either TypeScript or Luau. ✓

---

## Milestone 8: Material Optimization ✓

Texture and material pipeline polish. Move mip generation offline, establish Post Retro (hardware aniso + in-shader texel-grid reconstruction) as the foundational default look, and shrink normals on disk and in VRAM. Post Retro is now the sole texture-filtering path. Independent of Milestone 7 — ships in either order.

Plans ship in this sequence:

- [x] **Baked texture mips** — move mip generation from runtime renderer into prl-build. Gamma-correct linear-space Mitchell-Netravali filtering. Output as `.prm` sidecar files in per-mod `.prl-cache/tex/<blake3>.prm`, not embedded in PRL. `.prm` is a material bundle: per-slot (diffuse / specular / normal) with format tag, mip chain, payload bytes. Source PNGs remain the authoring source of truth; conversion is implicit during prl-build. `context/plans/done/baked-texture-mips/`
- [x] ~~**Shader anisotropic filtering** — per-pixel manual aniso in `forward.wgsl`, derivative-gated, N taps of `textureSampleGrad` along the major axis. Preserves nearest-filter chunky look in-plane while killing grazing-angle shimmer. Depends on baked texture mips. `context/plans/done/shader-anisotropic-filtering/`~~ **✂ Cut (2026-05):** retired with True Retro mode; hardware aniso is the sole path.
- [x] ~~**Graphics mode toggle** — introduced Post Retro and True Retro runtime filtering modes; `GraphicsMode` enum, `defaultGraphicsMode` mod-manifest key, egui combo.~~ **✂ Cut (2026-05):** True Retro mode + the `GraphicsMode` toggle scaffolding removed by *Retire True Retro mode*; Post Retro survives as the sole filtering path. `context/plans/done/graphics-mode-toggle/`
- [x] **BC5 normal compression** — BC5 encoder in prl-build, BC5 `format_tag` value in `.prm`, GPU upload path. Normals only — BC1/BC7 fight the pixel-art aesthetic on diffuse. Additive: `format_tag` is extensible from day one, no version bump. Also retires the Post Retro normal-averaging bias under hardware aniso. `context/plans/done/prm-bc5-normals/`
- [x] **Retire True Retro mode** — deleted manual-aniso shader code and True Retro branches in `forward.wgsl`; unwound `GraphicsMode` enum, `defaultGraphicsMode` mod-manifest key, nearest sampler pool, and egui mode combo. Post Retro normal path retains existing `(rgb*2-1)->normalize` decode unchanged. `context/plans/done/retire-true-retro/`
- [ ] **Texture pack format (optional)** — shipping consolidation of `.prl-cache/tex/` into a single pack file. **Deferred** until there are more real textures and the iteration-vs-ship tension actually appears. `context/plans/drafts/texture-pack-format/`

**Testable outcome:** Post Retro mode renders with no grazing-angle shimmer and crisp hardware-aniso filtering; True Retro opt-in is removed; normals are ~50% smaller on disk and in VRAM; level load does zero CPU mip work. ✓

**Status note:** all tasks shipped except the optional Texture pack format, deferred until there are more real textures.

---

## Milestone 9: Diffuse GI Upgrade (depth-aware probes + fog) ✓

Kill light-leak-through-walls by adding per-probe visibility data to the Milestone 5 SH irradiance volume, then extend the fog system. The depth-aware interpolant replaces the plain trilinear SH sample entirely — one runtime path. Probe streaming is deferred: this milestone keeps probes resident and produces the VRAM-fit measurement that decides whether streaming ever becomes its own milestone.

**Assumes the shipped Milestone 5 lighting foundation** — SH irradiance volume + baker, runtime probe sampling (SH as a 3D texture), lightmaps, and CSM sun shadows are all in place. Milestone 9 is a pure upgrade layer on top; it builds nothing M5 already delivers. Independent of Milestones 7–8.

**Pre-milestone fix — already satisfied:** the fog pass (`src/render/fog_pass.rs`) is imported, owned by `Renderer`, and runs in every frame's render stage. It was wired as part of the portal-fog-culling work, not as a standalone M9 prerequisite. See `rendering_pipeline.md` §7.5 and `plans/done/perf-portal-fog-culling/`. Directional fog (below) builds directly on the live pass.

Plans ship in this sequence:

- [x] **Probe weight correctness (no new data)** — in the world shader: reject trilinear corners facing away from the surface normal, exclude invalid (zero-packed) probes from the blend, renormalize remaining weights. Pure ALU. Fixes a latent bug where invalid probes drag near-wall surfaces toward black — independent of DDGI, and a prerequisite the depth-aware interpolant needs anyway. **Measurement gate:** record residual smear/leak here to quantify what the depth atlas buys before paying for it.
- [x] **Probe depth/visibility atlas (bake)** — prl-build stage baking per-probe depth moments alongside the existing SH bands, ray-cast through the Milestone 4 BVH. Format kept chunk-friendly so a later brick split needs no interpolant rewrite (deferred-streaming insurance). New/extended PRL section.
- [x] **Depth-aware runtime interpolant** — replace the trilinear SH sample with a visibility-weighted (Chebyshev) interpolant in the world shader, for both static surfaces and dynamic entities. Removes the plain-trilinear path. Depends on the depth atlas.
- [x] **Directional fog** — extend the live fog pass with the directional term, on top of the existing volumetric fog scope.
  - [ ] *(Optional)* **Back-scatter** — expose the signed `scatter_bias` range (negative `g`). Needs a third cached directional SH read. Wire format already stores signed `g`; no PRL format break when this lands. Deferred to future.
- [x] **Memory-budget checkpoint + coarse open-area spacing** — VRAM budget readout plus coarser probe spacing in open volumes; produces the empirical resident-fit number that gates any future streaming milestone.

**Testable outcome:** near-wall surfaces no longer darken from invalid-probe averaging; indirect light no longer bleeds through walls (visibility-weighted probe sampling); a single resident probe representation drives both static and dynamic surfaces; the fog pass runs with a working directional term; residual-smear and VRAM budget for a representative large map are both recorded. ✓

---

## Milestone 10: Animated Enemies

The first combat-capable enemy, and the per-entity 3D mesh render path it rides on. **North star:** a skinned-model enemy spawns from a map, walks the level toward the player without clipping, attacks, takes weapon damage, and dies at zero HP. Today the only dynamic render path is billboards/particles — there is no per-entity mesh pass and no skinning. That GPU path is the milestone's net-new spine; the combat layer (weapon already shipped, plus enemy, navigation, AI) is mostly script on top of the Milestone 6/7 foundation.

**Asset format.** Models load as glTF at runtime — mesh, skeleton, and animation clips read via the `gltf` crate, no per-mesh bake. Steady-state frame cost matches a baked path, and at low-poly scale baking geometry buys negligible load-time or VRAM. Textures are the one exception that still bakes: a model references external PNGs, the existing `.prm` pipeline mips and BC5-compresses them unchanged, so Blender authors against PNGs with no `.prm` plugin. A full mesh bake stays deferred — added only against a measured need (load time, swarm-scale animation sampling, or baked per-model data), kept additive by `format_tag`-style sidecars. LOD (`meshopt`) is a non-goal at this poly count.

**Contracts first.** A few runtime decisions are expensive to reverse: the GPU vertex layout (rigid + skinned), the bone-palette layout, the mesh render-pass shape (reusable in the depth/shadow pass), and the `MeshComponent` entity surface. Design these to the endpoint now — including to scale. Enemies arrive in waves, so the pass and palette are built **instance-friendly** (per-instance transform plus a palette index into a shared buffer), continuous with the Milestone 3.5 GPU-driven indirect draw path. A front-loaded thin vertical slice — one real model driven through the *live* path, code that survives rather than throwaway — locks them by building the narrow real version first. The render path is **skinned-capable from the start**; a rigid mesh is the degenerate single-bone case, so future props, pickups, and weapon viewmodels reuse the same pass without a separate tier.

**Behavior stays shallow.** The animation state machine, navigation, and AI are named here but detailed on open (the M13-UI / detail-on-open pattern). Navigation is the highest-uncertainty system on the roadmap — isolated so it can't quietly balloon, front-loaded to surface any foundation problem early.

**Lean rule:** smallest primitive surface that reads as game-y. Defer richness (projectile variety, line-of-sight queries, patrol graphs, multiple archetypes) to later passes. Each plan ships a foundation to grow, not a throwaway stub and not a finished feature.

**Prerequisite:** Milestone 6 (entity model + scripting + damage events) ✓ and Milestone 7 (grounded movement + collision world) ✓.

Plans ship in this sequence. The render foundation and combat tracks converge twice: skeletal hit zones need the posed bone palette, and the AI behavior plan needs both navigation and the animation runtime. Logical combat work — enemy HP/death, navigation — runs in parallel with the render track; the integrated outcome needs both.

**Render foundation** (net-new per-entity mesh path):

- [x] **Thin vertical slice (first model on screen)** — drive one real skinned glTF end-to-end through the *live* path, not throwaway code: load it via the real resource pipeline (material PNG → `.prm`), draw it through the real mesh pass at the interpolated transform, flat-lit (lighting integration deferred — the dynamic-entity lighting interface is mid-rewrite; SH-lit lands in the *Mesh render pass* task below against the settled interface), portal/frustum-culled via camera-leaf lookup, with one animation clip sampled into the bone-matrix palette. The asset is hardcoded behind a single named seam; the pass and palette are built **instance-friendly** (per-instance transform plus a palette index into a shared buffer), continuous with the Milestone 3.5 GPU-driven indirect draw path, even at one instance. Lands in the target module layout from the first commit — loader, mesh pass, `MeshComponent`, and animation modules each get their thin real slice behind seams the broadening tasks below fill *in place* (no dump-and-split). **Locks the contracts** — GPU vertex layout (rigid + skinned), instance-indexed bone-palette layout, instance-friendly mesh-pass shape — by building the narrow real version of each; durable layout decisions migrate to `context/lib/` (`rendering_pipeline.md`, `build_pipeline.md`, `entity_model.md`). **Measured findings** (measure-and-report, not pass/fail gates): whether runtime glTF loading stays within the boot northstar (else a mesh bake earns its place, additive via `format_tag` sidecars), and whether projected per-frame sampling cost at wave scale warrants an `ozz`-style baked pose buffer. **Breadth cut, not correctness:** one hardcoded archetype, no classname spawning, no LOD, shadows deferred, raw single-clip sampling (no state machine or blending) — each generalized by a task below.
- [x] **glTF mesh loading** — generalize the slice's single hardcoded asset into the full runtime loader: read arbitrary glTF mesh geometry and skinning vertex attributes (positions, normals, UVs, joint indices, weights) via the `gltf` crate into engine-side structs and GPU buffers in the slice's locked layout. Material textures resolve to their `.prm` equivalents through the existing texture pipeline (external PNG reference → `blake3` cache). `extras` metadata is read here and carried onto the entity. Renderer consumes handles, never raw glTF. The slice already proved the skinned vertex path on one model; full skeleton/clip generalization lands in *glTF skeleton + clip loading* below. Shape ≈ the runtime-loader half of *Baked Texture Mips*, without the bake stage.
- [x] **Mesh render pass + `MeshComponent`** — generalize the slice's single hardcoded draw into the general per-entity pass: many instances drawn at the interpolated transform, SH-lit, portal/frustum-culled (camera-leaf lookup locates the entity cell), each carrying a per-instance transform plus a palette index into the shared bone-matrix buffer. `MeshComponent` carries a model handle; classname wiring spawns mesh-bearing entities from a map (the slice's asset was hardcoded behind a seam this resolves). Pass stays reusable for the depth/shadow path (`depth_prepass.wgsl` precedent). Shape ≈ *Emitter Entity* (component + render integration + classname routing), minus its reuse of the billboard pass — this pass is genuinely new.
- [x] **Dynamic mesh shadow casting** — NPC meshes render into the 96-slot spot 2D-array depth pool (`lighting/spot_shadow.rs`) and a new point cube-array depth pool (`lighting/cube_shadow.rs`), which the static-lighting (SDF) rewrite has left consumer-less — NPC casting becomes their consumer (SDF handles static occluders; the depth pools handle dynamic ones). Adds a skinned-depth variant of the mesh pass. *Can defer* — not on the walk/attack/die critical path — but the pass is built depth-reusable from day one, so deferring costs nothing later. Grounds enemies visually.
- [x] **Dynamic mesh direct lighting** — wire the Milestone 5 flat per-fragment runtime-light loop (with per-light influence-volume culling) into the skinned-mesh shader, so dynamic meshes receive dynamic *direct* light on top of their SH-lit indirect. Fills the reserved group-2 lighting slot the slice left open in the mesh pass (`render/mesh_pass.rs` group 2 — "SH ambient + dynamic direct"). SH-lit (the *Mesh render pass* task) is the indirect baseline; this adds the direct term against the settled dynamic-entity lighting interface, reusing the world shader's existing light loop. *Can defer* — not on the walk/attack/die critical path (the north star is "lit by baked SH"), so deferring costs nothing later; enemies simply read flatter under dynamic lights until it lands. Depends on the mesh render pass.
- [x] **Dynamic mesh shadow receipt** — the inverse of *Dynamic mesh shadow casting*: dynamic meshes *receive* crisp runtime shadows, not just cast them. Once *Dynamic mesh direct lighting* opens the group-2 per-light term, attenuate that term per dynamic light by sampling the light's existing shadow map — the M10 spot 2D-array (`lighting/spot_shadow.rs`) and point cube-array (`lighting/cube_shadow.rs`) pools, consumed here from the *entity* shader side rather than only the world shader — giving **world→entity** (static geometry shadowing an enemy) and **entity→entity** (enemies shadowing each other) crisp shadows. Today entities get only the soft, probe-coarse, occlusion-tested SH-direct approximation of world→entity shadow (baked); this adds the crisp per-light dynamic version. **Depends on** *Dynamic mesh direct lighting* — there is no runtime per-light term to multiply by a shadow factor until group 2 is allocated — and reuses *Dynamic mesh shadow casting*'s pools, so it is purely additive on top of both. *Can defer* — visual polish, not on the walk/attack/die critical path; enemies read correctly-lit but softly-shadowed until it lands.
- [x] **glTF skeleton + clip loading** — generalize the slice's minimal single-clip read into the full loader: the complete joint hierarchy, inverse-bind matrices, and all animation clips into engine-side structs. Feeds the animation runtime. Smaller than the mesh loader; keyframe sampling itself lives in the animation runtime. `context/plans/done/M10--gltf-skeleton-clip-loading/`
- [x] **Skinned animation runtime** — build the animation state surface on the slice's raw single-clip sampling: per-frame clip sampling and pose blending → bone-matrix palette, with a shallow state surface (idle / locomotion / attack / death with crossfade). Engine owns sampling and blending — `ozz-animation-rs` kernels are a candidate (the slice's pose-buffer measurement decides) — while the state machine stays small and script-authored, not a visual editor or imported graph. Distant or off-screen agents sample at a reduced rate (animation time-slicing) to carry waves cheaply. Shape ≈ *Animated SH Volumes* (runtime + shader, one multi-task plan). Depends on the loaded clips and the mesh-pass palette binding. `context/plans/done/M10--skinned-animation-runtime/`

**Combat** (script-led, on the Milestone 6/7 foundation):

- [x] **Weapon primitives** — script-declared weapon archetype + Rust hitscan fire system against the Milestone 7 collision world; spawns an impact, emits a typed `Hit(DamagePayload)` activation outcome and `activate`/`impact` sound events. Hitscan only; projectile, ammo, viewmodel deferred. `context/plans/done/M10--weapon-primitives/`
- [x] **Entity health + damage surface** — minimal health/damage primitive on the Milestone 6 entity model: an entity carries HP, consumes a `DamagePayload`, dies at zero HP. Demonstrated on the enemy (the weapon's target) and reused for the player (the enemy's target), so the damage loop closes both ways. Pure Milestone 6 — no render, nav, or AI dependency. Shootable as a static proxy, so it gives the shipped weapon a target the day it lands. The `player.health` slot schema (typed, ranged, readonly) is the **published contract** Milestone 13's UI binds against — keep it stable. Keep HP/death minimal here: the component *representation* (a dedicated health kind vs. a generic scalar-stat kind shared with shields) is an internal choice, and *policy* (regen, recharge, resistance, elemental damage types) defers to the Shields + damage-type system (Future / Gameplay) authored as command-buffer policy over the behavior-IR foundation. `context/plans/done/M10--entity-health-damage/`
- [x] **Navigation representation (baked)** — resolve the expensive, hard-to-reverse question first: where do walkable surfaces come from? Lead candidate is an offline bake in prl-build — derive a navmesh from world geometry (agent radius/height, slope filter), emitted as a new PRL section, kin to the baked BVH and collision trimesh. This is also the seed of a broader baked spatial-AI layer: the navmesh is the first hint data, and later intelligent-interaction data (cover points, jump links, hint nodes) extends the same section additively, no format break. The heavy, uncertain piece — front-loaded to surface any foundation problem early. Depends only on world geometry. `context/plans/done/M10--navigation-representation/`
- [ ] **Pathfinding + path following** — runtime query and steering: A* (or equivalent) over the baked representation, plus path following that moves an agent toward a target around obstacles without clipping. Smallest workable primitive that actually routes past walls and corners — naive steer-to-target would snag on the first concave wall. Richer queries (line-of-sight, patrol paths) deferred. Depends on the navigation representation and a movable agent entity (Milestone 6 transform + Milestone 7 collider). Specced and reviewed: `context/plans/ready/M10--pathfinding-path-following/` (closing wave, plan 1 of 2).
- [x] **Skeletal hit zones** — dynamic hittable volumes: bone-parented proxy capsules posed each frame from the skeleton, raycast separately from the static collision world — net-new, since the weapon hitscans only static geometry today. Hit-zone identity comes from glTF `extras` tags (`head`, `limb`); per-archetype damage multipliers live in the descriptor script. Model ships the spatial tag, script ships the balance — mirroring map `_tags` → entity behavior. Depends on the skinned animation runtime's posed palette. `context/plans/done/M10--skeletal-hit-zones/`
- [ ] **Enemy AI behavior** — simple state machine (idle → alert → attack → death), authored in the SDK as a reference behavior. Drives navigation (move toward player), attack (emit a damage hit at the player), and animation state (select the clip per logical state). The behavioral convergence: depends on the entity health/damage surface, pathfinding + path following, and the skinned animation runtime. Behavioral time-slicing (distant agents think less often) is named for waves but stays shallow. A foundation to refine, not a stub. Specced and reviewed: `context/plans/ready/M10--enemy-ai-behavior/` (closing wave, plan 2 of 2).

**Testable outcome:** a skinned-model enemy spawns from a map, walks toward the player without clipping playing its locomotion clip, switches to an attack clip and damages the player in range, takes hitscan weapon damage, and plays a death clip then despawns at zero HP — lit by baked SH (optionally casting a real-time shadow via the dynamic shadow pool and receiving dynamic direct light). Weapons and enemies emit typed sound events throughout; audible playback lands with the Sound Foundation milestone.

---

## Milestone 11: Advanced Movement

Modern-FPS movement layered on the Milestone 7 grounded controller. A movement state machine splits the player tick into a shared physics substrate plus per-state velocity-intent functions. A sequence of traversal states — dash, crouch, slide, wall-run, vault — plug into that seam. The author surface is declarative: native Rust states tuned through descriptor data, not per-tick script. Design intent lives in `context/lib/movement.md`.

This earns a milestone because the later specs cannot be fully written until earlier ones ship and reveal emergent implementation details. Chiefly two cross-cutting policies — momentum conservation across state transitions, and input forgiveness (coyote time, jump buffering) — must be settled before the states that depend on them. This milestone tracks those specs-to-be-written under the detail-on-open pattern already used for Milestone 10's behavior layer and Milestone 13 UI.

**Prerequisite:** Milestone 7 (grounded movement + collision world) ✓.

Plans ship in this sequence:

- [x] **movement--state-machine** — split the monolithic player tick into a shared physics substrate (sweep-and-slide, step-up, ground-stick — moved intact) plus a per-state velocity-intent seam; refactor current walk/run/jump into a behavior-identical `Normal` state; ship dash/air-dash/double-jump on the new seam; establish the declarative descriptor author surface. `context/plans/done/movement--state-machine/`
- [x] **Cross-cutting movement policies** — settle momentum conservation (velocity carry across transitions) and input forgiveness (coyote time, jump buffering) as explicit foundations before the states that consume them. Detail-on-open from the state-machine seam. See `movement.md` §6. `context/plans/done/movement--cross-cutting-policies/`
- [x] **movement--crouch** — `Crouching` state on the state-machine seam: feet/head-anchored capsule resize plus stand-up ceiling probe, factored as reusable substrate helpers (consumed by slide), a crouched speed tier, eye-height smoothing, crouch-jump (never suppressed), and toggle/hold `crouch_mode` resolved in the input layer. `context/plans/done/movement--crouch/`
- [x] **movement--view-feel** — render-only first-person view feel (head bob, strafe tilt, ambient sway) as a declarative `viewFeel` sub-descriptor; never touches the movement tick, collision, or gameplay state. Independent thin slice, draftable early — gated only on the already-shipped state-machine descriptor surface and pawn-follow camera, not on the momentum policy or any traversal state, and a prerequisite for none of them. Reads pawn velocity generically, so it auto-extends to slide/wall-run roll for free as those land. `context/plans/done/movement--view-feel/`
- [ ] **movement--slide** — speed-preserving slide (Titanfall model); owns and consumes the momentum-conservation policy. Detail-on-open: depends on that policy and crouch's capsule resize.
- [ ] **movement--wall-run** — first environment-probe state; consumes the momentum policy. Detail-on-open.
- [ ] **movement--vault** — environment-probe state; parallelizable with wall-run once the momentum policy is fixed. Detail-on-open.
- Grapple is explicitly deferred — constraint physics, renderer rope, and aiming make it its own future draft (the one place a scoped Rapier solver may earn a place; see `movement.md` §1).

**Testable outcome:** the player chains modern-FPS traversal — dash, crouch, slide, wall-run, vault — on top of grounded movement, all tuned through descriptor data; movement identity stays composable (Ultrakill / Neon White are the flexibility-band yardstick) without engine internals becoming convoluted.

---

## Milestone 12: Sound Foundation

A real audio foundation: kira integration, spatial/3D audio, and reverb zones. Builds behind the Milestone 6 entity event system — entities emit typed sound events (weapons already do; enemies will), and audio is their playback sink. No weapon or enemy code changes when audio lands.

Independent of the other upcoming milestones. It does not wait on animated enemies; it needs only the entity event system to route through, and a richer combat soundscape simply follows whenever enemies ship.

**Prerequisite:** Milestone 6 (entity event system — the sound-event source to build behind) ✓.

Plans ship in this sequence:

- [x] **kira integration** — audio subsystem in its own module (subsystem-boundary principle); mixing, buses, lifecycle. `context/plans/done/M12--audio-foundation/`
- [ ] **Spatial audio** — positional sources with attenuation; listener driven by the player/camera.
- [ ] **Reverb zones** — runtime playback for `env_reverb_zone` acoustic zones (baked data already resolves them to leaves at load; see `context/lib/audio.md`).
- [ ] **Sound-event playback** — route entity-emitted sound events through real mixed, spatialized playback; entities raise events, audio plays them.

**Testable outcome:** spatialized combat and ambient audio; reverb zones audibly change acoustics; entity-emitted sound events drive real playback with no changes to weapon or enemy code.

---

## Milestone 13: UI

The full UI/HUD layer — health, ammo, crosshair, menus — replacing the debug egui stand-in. The design is captured in `context/research/ui-layer.md`; this milestone realizes it as a sequence of individually draftable goals, each its own `/draft-spec` → `/orchestrate` cycle rather than one monolithic plan.

**The decoupling seam.** The state system (`StateValue<T>` handles + engine-owned readonly slots, research §9) is what lets UI build now. HUD widgets bind to slots like `player.health`; a static proxy feeds those slots today, real game logic feeds them later — no code dependency either direction. This is the Milestone 10 shootable-static-proxy pattern applied to UI. **Nuance:** decoupling breaks the *code* dependency but couples the *slot schema*. The State-system goal publishes the engine-owned slot schema (`player.health` as a typed, ranged contract) as a **published contract** that Milestone 10's entity health/damage task is on the hook to honor — the schema is the coordination point, not a shared module. Otherwise the only cross-milestone touchpoint is merge coordination on the renderer module: the UI pass is a sibling pass, a logical peer (research §4), not a dependent — **except** the post-UI screen-space-effects goal, which reaches into the scene compositor where Milestone 9/10 post lives and needs explicit merge coordination.

**Prerequisite:** Milestone 6 (entity event system / reaction registry) ✓ — the reaction surface goals E and SE reuse. It does **not** require the entity health/damage surface; the static proxy stands in. The slot-schema contract (above) coordinates with Milestone 10's health/damage task, but neither blocks the other. Independent of the other upcoming milestones; runs concurrently.

**Rendering model.** Modern text rendering, not pixel-art text: `glyphon`-shaped, anti-aliased glyphs at device resolution are the default from Goal A. UI lays out in a **1280×720 logical reference space** (the supported floor is 720p 16:9) scaled by a factor to the native backbuffer, and renders at native resolution — no fixed low-res target, no nearest-neighbor upscale. The "blocky" retro feel comes from art (9-slice panel sprites, flat fills) and integer device-pixel snapping of quads/panels, while glyphs stay AA-crisp at any resolution ≥ 720p. This supersedes the bitmap-font default (research §8), the fixed design-resolution + nearest upscale (research §7), and the no-AA-text non-goal (research §20) — see the amendment note at the head of `ui-layer.md`.

Plans ship in this sequence:

- [x] **A — UI render pass + thin vertical slice (splash reimplementation).** The slice is a *real* screen, not a throwaway demo: **reimplement the boot splash** on the new UI foundation, retiring `render/splash.rs`'s `SplashPipeline` instead of standing a second quad pipeline beside it. Its panel + logo image + shaped text draw end-to-end through a real new `render/ui/` peer pass — native-resolution render, 1280×720 logical-reference layout, device-pixel-snapped quads, `glyphon` AA text, alpha-blended into the final color target after scene / before present (research §4, §13). The splash's existing fade/timing and boot-sequence integration stay intact — only its *drawing* moves to the UI pass. It runs **pre-gameplay** (before any level, game logic, or state), which is exactly Goal A's decoupled scope; the egui debug overlay is untouched. **Locks** — render-pass placement in frame order, the instanced-quad / 9-slice pipeline shape, the native-render + logical-reference scaling model, `glyphon` shaped text as the default path, the once-per-frame published read handle; **and pulled forward**: the Input-stage UI-dispatch seam plus the modal capture-vs-passthrough contract (research §4 — a UI event resolved on frame N must reach game logic on frame N+1; a frame-ordering contract, not an interaction feature); **and** the automated test strategy for a GPU-drawn UI — CPU draw-list / layout-tree assertions as the hard gate (AA text makes exact golden images backend-fragile; goldens are tolerance-scoped or skipped). The splash is structured as a descriptor so B + G1 later make it fully script-authored — a product author replaces it with no rework — with no script ingestion in A itself. Narrow real version of each, built in place. `context/plans/done/M13--ui-render-pass-slice/`
- [x] **B — Descriptor model + retained tree + layout.** serde descriptor structs ↔ Rust enum variants, Rust-owned retained tree, taffy layout (research §5, §7), anchor + offset, dirty-tree relayout, core widget vocab (text / panel / image / vstack / hstack / grid / spacer, research §6). Built in Rust and tests, no script ingestion yet. **Locks (pulled forward):** the descriptor **wire format** as a first-class deliverable with a Boundary Inventory (Rust ↔ JS ↔ Luau ↔ wire casing) and the discriminated-union-per-kind decision (research §6, §15) — not deferred to the SDK goal. Integrates the **measure** seam against `glyphon` shaped-text metrics (the slice already proved the shaped-text path in A) so `taffy` sizes text nodes from real glyph measurement. Depends on A. `context/plans/done/M13--descriptor-tree-layout/`
- [x] **C — State binding (the UI decoupling seam).** Consumes the **Mod State Store** (`plans/done/mod-state-store/`, a scripting-foundation prereq that ships the slot table, `defineStore`, engine-owned-readonly vs. modder-declared slots, clamp/validate, the `persist: true` save wire format, and branded `StateValue<T>` — extracted because the store is not UI-only: game logic owns values the HUD merely displays). C owns the *UI* half: the once-per-frame published read handle, descriptor bind-by-slot-name, subscriber-aware value diffing → relayout/redraw split, the retained `UiTree` (closing B's deferred follow-up), and the static proxy populating `player.health` / `player.ammo`. **Publishes the engine-owned `player.*` slot schema as the contract Milestone 10 honors.** Depends on B and the Mod State Store. `context/plans/done/M13--state-system/`
- [x] **Game State SDK surface.** Shared engine-state catalog, generated `getGameState()` reference tree, pure returned `defineStore` declarations, readonly/writable state-ref types, `bindState` / `stateEquals` / `updateState`, and QuickJS/Luau runtime + typedef parity. Ships the author-facing durable game-state surface that later HUD/BIS code consumes without `.get()` handles or import-time store registration. `context/plans/done/game-state-sdk-surface/`
- [x] **D — Fonts + theming.** Multi-font registration (TTF assets beyond the engine default A ships), the theme-token table, widgets referencing tokens by name (research §8). The `glyphon` shaped-text *engine* lands in A; D generalizes font supply and adds the theming layer on top. Depends on C. `context/plans/done/M13--fonts-theming/`
- [ ] **E — HUD dynamics.** `styleRanges` (renderer-local continuous value → style, research §10) + `onStateCrossing` (discrete crossings firing reaction lists, research §11), reusing the existing entity reaction registry. Absorbs the UI reaction helpers — `flashScreen` / `playSound` / `rumble` / `showDialog` / `closeDialog` / `openMenu` (research §15). After D: `styleRanges` resolve theme tokens D defines. Concurrent with F. `context/plans/ready/M13--hud-dynamics/`
- [ ] **F — Input breadth.** Hit-testing, single-focus focus ring, template-typed nav intents, hold-to-repeat, pointer-vs-focus input-mode switching, the modal UI stack, gamepad via gilrs, button / input activation (research §11, §12, §16) — filling the seam A locked, plus the first interactive widgets (`button` / `slider` / `bar`) and the `setState` slot-write reaction sliders require. Concurrent with E (both build on the foundation; the two extend `descriptor.rs`/`tree.rs` — merge-coordinate). Depends on B (and the A seam). `context/plans/ready/M13--input-breadth/`
- [ ] **TE — Text entry (on-screen keyboard + hardware keys).** Hardware typing and an on-screen keyboard — the gamepad accessibility accommodation — drive one engine text-edit surface over a writable string slot (`ui.textEntry`). The keyboard is a JSON descriptor asset built from F's grid / spatial focus / buttons / modal stack: the wave's integration consumer, the role the splash played for A. IME composition deferred. Trails E + F. `context/plans/ready/M13--text-entry/`
- [x] **TW — UI value tweening (animated / eased values).** Time-driven animation of a UI value toward a target — eased transitions with a duration and easing curve — that *produces* animated values, distinct from E's `styleRanges` (continuous value → style map) and `onStateCrossing` (discrete crossings → reactions); neither animates a value over time, so tweening is unowned today. Motivating case: a **display value decoupled from its authoritative slot** — a health bar animating from a cosmetic 80% up to the authoritative `player.health` (=100) on level load (the "systems booting up" flourish), purely presentational with zero game-logic involvement — the decoupling seam's payoff. Model: the authoritative slot is the target, a separate UI-owned display value eases to it, the widget binds the display value (never the authoritative slot). **Resolved (spec):** a UI-owned animated-value primitive, renderer-local in the retained tree — no mod-facing slot writes; C's deferred `setState` decision stays with E/F, untouched. Depends on C (the values it animates); independent of D / E / F; runs in the D → (E ‖ F) band. `context/plans/done/M13--ui-value-tweening/` The literal eased health bar is **deferred** (owner, 2026-06): it additionally needs F's `bar` widget, so it lands as a BIS built-in once TW + F (incl. `bar`) exist.
- [ ] **G1 — SDK core + lifecycle.** Factory functions, props-first-then-children, branded handles, modder-defined components (with component-local `ui.localState()` state scoped to the instance lifecycle — distinct from the Mod State Store's global slots; working name, was `liveValue()`, renamed per the computed-vs-stored naming rule in `scripting.md` §11), the `sdk/lib/ui/` file layout, the mod-init / level-load register → VM-drop lifecycle (research §15, §18). The convergence point D + E + F build toward. **Require:** route all user-facing text through a single text-alias chokepoint from day one, so the future `LocalizedText` swap is a type-alias change, not a rewrite (research §15). Depends on D, E, F.
- [x] **G2 — Reactive UI (selection + visibility) + SDK type-safety + a11y.** Two layers in one goal — **scoped up 2026-06 from the original a11y-only future-proofing goal** (owner decision, roadmap-alignment reviewed and kept). **(1) Reactive primitives:** a selection-predicate bind over `localState` (a `Predicate` = bind-source + optional `equals` → resolved once to 0/1), `Display::None`-based `visibleWhen` + `Switch` sugar — making tabs / segmented controls / toggles buildable, with a11y `selected`/`checked` **derived from the same predicate** (no static-flag desync). Consumed now. **(2) A11y metadata + type-safety:** `label` / `labelledBy` required, `role`, image alt/decorative, modal `accessibleName`, the `Announce` node, template-literal nav intents, discriminated unions per kind (research §15, §16); a11y *consumption* (screen readers) stays deferred (research §19, §20). **BIS depends on this** (its stateful/tabbed screens use the selection + visibility primitives), so G2 lands **before** BIS — no longer the trailing future-proofing goal. Depends on G1. `context/plans/done/M13--sdk-typesafety-a11y/`
- [x] **SE — Post-UI screen-space effects.** Vignette, flash, screen shake as full-screen quads driven by slots (research §13). These reach into the scene compositor, where Milestone 9 tonemap/fog and Milestone 10 post live — its own goal precisely because it carries the one real cross-milestone merge. **Coordinate with Milestone 9/10.** Depends on the foundation (A–C) and the slots E exposes. `context/plans/done/M13--screen-space-effects/`
- [ ] **BIS — Built-in screens + egui retirement.** HUD, pause, dialog, level-load, death / respawn, damage vignette (research §14) — descriptor authoring on the finished primitives, the milestone payoff (the Milestone 10-style "integrated outcome needs both" convergence). Includes the **egui-retirement checklist:** the new UI must replicate whatever the egui overlay provides — the `POSTRETRO_GPU_TIMING` / frame-stats diagnostics (per `CLAUDE.md`) — before egui is removed. Depends on G1, E, F, SE, and G2 (the reactive selection/visibility primitives its stateful/tabbed screens build on).

**Deferred / detail-on-open (unchanged):** minigames-as-built-in-entity-types (research §17), in-world viewport UI (research §19), localization runtime (research §19), screen-reader a11y (research §19, §20).

The spine A → B → C is sequential. Then D → (E ‖ F): D before E (token dependency), F concurrent with E, with the F/D layout-module merge-coordination noted. TW (value tweening) joins this band, dependent only on C and concurrent with D/E/F. TE (text entry) trails E + F as the wave's integration consumer. G1 converges D + E + F. Then G2 (reactive selection/visibility + a11y/type-safety) lands on G1, before BIS consumes its primitives. BIS and SE follow (SE needs compositor coordination with Milestone 9/10; SE is independent of G2 and may ship in parallel). The deferred set stays detail-on-open.

**Testable outcome:** a playable HUD (health / ammo / crosshair sourced from the static proxy), functional gamepad-navigable menus with the modal stack, the design from `ui-layer.md` realized, and the egui debug stand-in retired.

---

## Milestone 14: Behavior IR (Typed Command Buffer)

Realize the typed command buffer (`scripting.md` §11) — today a recorded principle with nothing implemented. Authored behavior that depends on live state crosses the FFI as **typed, serializable IR**, not a retained function: the author calls a builder API at load time, the call constructs a tree of closed-vocabulary opcodes whose leaves reference engine state by name, the VM drops, and a Rust **total evaluator** binds the named leaves to live state and evaluates each tick. This is the durable form of "scripts declare, Rust executes" for live-state behavior, and it pays down accumulated debt: reactions, light/fog animation channels, and behavioral descriptor fields were each invented as fixed-shape special cases *before* the general pattern was named (`scripting.md` §11) — they are proto-command-buffers this milestone consolidates onto one substrate.

This is a milestone because the substrate, its versioning, and its first real adopter are expensive to reverse and must settle before broad adoption. It is not on the critical path of any other milestone — a parallel track sharing only the Milestone 6 scripting foundation.

**Prerequisite:** Milestone 6 (scripting runtime + primitive registry + the `conv.rs` FFI bridge) ✓.

Plans ship in this sequence:

- [x] **IR substrate + evaluator** — the core: builder opcodes registered like any primitive (emitted into the typedefs, so the vocabulary *is* the contract), the typed serializable IR crossing the FFI as data, and the pure / total / bounded Rust evaluator that binds named-input leaves to live state and evaluates the tree each tick. Start the node set minimal (`scripting.md` §11): named-input leaves, arithmetic, `clamp`, `lerp`, `select`, comparisons. No wall-clock, no RNG, no unbounded loops — Turing-incompleteness is the feature. The foundation the rest rides. Shipped: closed 15-opcode `IrNode` tree + serde wire format, the pluggable `BindingScope` seam, the total / zero-alloc bind→eval split, store + stub scopes (engine/script write capabilities), and dual-runtime TS/Luau builders + typedef emission — substrate only, no adopter yet. `context/plans/done/M14--behavior-ir-substrate/`
- [x] **IR versioning** — the `scripting.md` §11 obligation: a serialized command buffer baked into a mod survives engine-version changes. Format version stamp plus the migration discipline for opcode-vocabulary evolution. One versioning story shared with the mod state store's persist format and the deferred UI-reaction `setState` IR — all three are serialized-behavior-as-data. Designed before the first shipping use, not after. Shipped folded into the substrate plan: a `u32`-stamped `BakedIr` envelope with a load-time check mirroring the persist loader (unsupported version ignored-with-warning, adopter falls back). `context/plans/done/M14--behavior-ir-substrate/`
- [x] **First adopter (narrow real version)** — pull one concrete live-state-dependent behavior onto the IR end-to-end, code that survives rather than a throwaway: a movement velocity-intent function (`movement.md` §2) or a shield recharge policy. Proves the substrate against a real author surface and a real per-tick bind. **Resolved (spec):** movement dash tuning — six dash descriptor fields become expression-capable over a movement-local scope, prepended by the modder-facing `ir` → `runtime` SDK rename. Shield recharge becomes adopter #2, the write-path proof, opening the Shields milestone. `context/plans/done/M14--movement-dash-runtime-values/`
- [ ] **Primitive consolidation** — migrate the pre-IR special cases (reactions, animation channels, behavioral descriptor fields) onto the shared substrate, demand-driven. Static-config descriptors (a light color, a max-HP scalar) stay as plain data — only *computed / conditional / derived* behavior consolidates. Incremental, never a big-bang rewrite.

**Testable outcome:** an author expresses a live-state-dependent behavior (e.g. `boost = f(speed, charges, grounded)`) as a builder call that produces serializable IR; the VM drops; the Rust evaluator runs it deterministically each tick; at least one previously bespoke primitive is migrated onto the shared substrate with behavior-identical output; a baked IR survives a simulated engine-version bump.

---

## Milestone 15: Multiplayer (Co-op Netcode)

Authoritative client-server multiplayer: a host runs the world and up to 16 players share a campaign level, co-op campaign first. **North star:** two-plus players join a host's level (one mid-game), move and fight together against shared server-authoritative enemies and set-pieces, with local-player movement and projectiles client-predicted so the game stays responsive under latency. The model is **authoritative server + snapshot replication** (Quake/Source/Overwatch lineage), deliberately **not** deterministic lockstep — the engine simulates in f32 (glam, parry3d) and cross-architecture f32 is not bit-identical, so lockstep would desync; snapshots tolerate that drift by construction. This reverses the standing "Multiplayer / networking" non-goal (`index.md` §4, `entity_model.md` §9, reconciled).

**Stack.** `renet` 2.0 + `renet_netcode` transport (Bevy-free since 2.0, synchronously frame-polled, no tokio), hand-rolled replication (`lightyear` as design blueprint, not a dependency — it is Bevy-coupled; the registry is bespoke), `bitcode` serialization, custom per-entity snapshot delta. A new `crates/net/` sibling crate owns transport + replication; the headless simulation seam (Phase 0) is what lets a dedicated server split out from the listen-server later.

**Build shape:** horizontal phases, each ending in a runnable checkpoint with a crisp single-contract acceptance bar — deliberately **not** a vertical slice (a slice forces fuzzy "it all kind of works" AC). The one empirical question — does basic predict/reconcile *feel* right — lives in the Phase 0 spike, where fuzzy measured-finding AC belongs.

**Epic milestone**, realized as detail-on-open phases — each its own `/draft-spec` → `/orchestrate` cycle (the M10/M13/M14 pattern). The full design — model, stack, crate decisions, named-pattern references, architectural-invariant reconciliations, risk ledger, seam map — lives in `context/research/netcode/`. Per-phase specs are drafted against it as each phase opens.

**Prerequisite:** Milestone 6 (entity model + scripting) ✓, Milestone 7 (grounded movement + collision world) ✓, and **Milestone 10 (animated enemies)** — co-op combat needs server-authoritative enemies; M10's `Agent` + AI-brain components replicate as ordinary entities. Phases 0–3 are enemy-free and need only M6/M7 (Phase 2's on-the-wire test entity is a dumb AI-less mover); the combat-bearing phases (4–7) build on M10.

Phases ship in this sequence (detail-on-open; critical path `0 → 1 → 2 → (3 ‖ 4) → 5 → 6 → 7`):

- [ ] **Phase 0 — Headless seam + determinism harness + spike.** Extract the fixed-tick game logic out of the render-interleaved frame loop into a headless `simulate` seam (no wgpu/winit) — the shared server+client tick path; write the determinism test first (green-and-stays-green gate). Spike: measure cross-arch f32 divergence (sets the reconciliation tolerance) + a throwaway predict/reconcile feel-prototype. Split-before-extend `main.rs` (5,792 lines) and `movement/mod.rs` (6,055 lines) first; budget 2–3×. Specced + reviewed (structural, codebase-anchor, implementability): `context/plans/ready/M15--p0-headless-sim-seam/`.
- [ ] **Phase 1 — Transport + wire + handshake.** renet 2.0 + renet_netcode in a new `crates/net/` (polled non-blocking, no tokio) + protocol/version handshake (mismatch rejected, no state) + bitcode wire (native `Encode/Decode` on wire-bound component types — serde-tagged enums can't round-trip on a binary format). A snapshot struct round-trips; a remote pawn appears and moves. Latency-sim harness lands here (in-process conditioner + `tc netem`, not turmoil). `networking.md` context doc lands at this phase's promotion.
- [ ] **Phase 2 — Replication: delta/baseline/ack + time-sync + interpolation + lifecycle.** Per-entity delta vs. per-entity acked baseline (eventual consistency, lightyear-style); time-sync; join-in-progress AND player-leave/disconnect; remote interpolation with jitter-sized delay. Proven on a dumb AI-less server-authoritative mover (no M10). Exit gate: smooth at 150 ms RTT + 5% loss + jitter.
- [ ] **Phase 3 — Movement prediction + reconciliation.** Client predicts its own pawn from buffered input (command frames); reconciles against snapshots; respawn reconciles as a teleport (snap). Reconciliation *smoothing* (esp. dash corrections) is the hard part. May run in parallel with Phase 4.
- [ ] **Phase 4 — Co-op set-piece design (gating).** Trigger ownership, reveal/spawn fan-out, progress, co-op respawn + player-leave policy, set-piece-progress replication for joiners — proven by one playable co-op set-piece with real M10 enemies. Gates the combat phases ("is co-op fun"). Parallel with Phase 3.
- [ ] **Phase 5 — Server-authoritative hitscan combat.** Fire server-authoritative with immediate cosmetic feedback; favor-the-shooter against a short single-entity history — not full server-rewind. HP changes only on confirmation.
- [ ] **Phase 6 — Predicted projectiles.** Client-predicted projectiles with predicted-entity → server-confirmed handoff, for rocket/grenade feel.
- [ ] **Phase 7 — Scale + dedicated-server readiness.** Validate 16 players within a host-upstream bandwidth budget (priority-accumulator; interest management via portal/PVS if needed); prove the headless server entry point (dedicated-server split).

**Testable outcome:** two-plus players connect to a host (one joining mid-level, another dropping with the session surviving), move together with predicted local movement that stays responsive at 150 ms + loss + jitter, fight shared server-authoritative enemies through a co-op set-piece, and fire predicted projectiles — all reconciling to the host's authority with no full server-rewind. A headless server entry point compiles, confirming dedicated-server readiness.

---

## Future / Speculative

Features below are intended but not yet sequenced. Rough priority ordering within each group.

### Gameplay systems

- **Weapons** — foundation (weapon primitives + first SDK weapons) lands in Milestone 10. Remaining/speculative refinements: projectile variety, viewmodel hooks, and at least one weapon that triggers chunk destruction.
- **Shields + damage-type system** — an engine shield/stat component with authored *policy* via typed command buffers (behavior-IR milestone): elemental / resistance damage-type interactions and recharge models (fast like Halo, slow-and-delayed like Borderlands). The engine owns the component and its per-tick recharge system; the modder owns the policy IR. Health (Milestone 10) is the minimal scalar precedent; shields generalize it. Promote to an active milestone once the behavior-IR foundation ships.
- **NPC Entities** — first enemy plus the nav/AI foundation lands in Milestone 10. Remaining/speculative refinements: richer scripted AI state machines (patrol / chase / attack), line-of-sight and navigation queries, and multiple enemy archetypes.
- **Baked spatial-AI data** — the Milestone 10 navmesh bake is the first layer of a broader compile-time hint set for intelligent enemies. Speculative extensions reuse the same additive PRL section: cover points, jump/drop links, hint nodes (sniper perches, ambush spots), and precomputed influence/flow data. Authored in TrenchBroom where useful, derived in prl-build where it follows from geometry.
- **World Entities** — common base scripts for doors, pickups, trigger volumes, timeline/sequence helpers; a scripted ambush set piece with destruction choreography.

### Moving and destructible geometry

- **Kinematic Clusters** — sub-worlds compiled like the main world but with a runtime transform (elevators, barges). Cluster authoring in TrenchBroom, compiler emits per-cluster geometry, `KinematicDriver` entity sets transform each tick. Dynamic portals at cluster boundaries when aligned with a static sector portal.
- **Destruction (Pre-Fracture + Promotion)** — brushes pre-fractured into pieces with dependency edges at compile time. Runtime promotes pieces from static to dynamic on damage; reveals pre-authored interior break-faces. Requires a full rigidbody solver (Rapier) for debris physics. Latent portals activated on fracture to open hidden areas.

### Rendering and visual polish

- **Billboard sprite rendering** — ~~character and effect sprites; depth-sort against world geometry.~~ **Shipped.** `BillboardEmitter` entity type, particle sim, and additive billboard pass (`src/fx/smoke.rs`, `billboard.wgsl`). See `plans/done/scripting-foundation/plan-3-emitter-entity.md`. Character rendering moved to 3D models (Milestone 10), not sprites.
- **Per-entity 3D models** — the skinned/rigid mesh render path lands in Milestone 10 (animated enemies). Speculative reuse: rigid props, pickups, and weapon viewmodels build on the same pass — a rigid mesh is the degenerate single-bone case — not yet sequenced.
- **Specular maps** — ~~per-texel specular highlights in the direct light loop. Shading model decision (Phong vs. PBR) required first.~~ **Shipped.** Blinn-Phong per-texel specular via `_s.png` siblings, chunk-list multi-source loop, bumped-Lambert correction. See `plans/done/normal-maps/`.
- **Fog volumes** — `env_fog_volume` brush entity wired to a runtime fog pass. Pass is wired and runs each frame (with portal-fog culling — see `plans/done/perf-portal-fog-culling/` and `rendering_pipeline.md` §7.5). Only the **directional fog** term remains, tracked under Milestone 9.
- **Emissive / fullbright surfaces** — texture-driven self-lit surfaces. Not started. An earlier `neon_`-based lighting-replacement stub was built then **✂ Cut (2026-05):** removed as incorrect. The correct approach is additive HDR emissive + bloom, designed alongside the post-processing pass.
- **Moving-light shadow-depth invalidation** — extends Milestone 10's static/dynamic shadow-depth split (cached static-world depth + per-frame dynamic occluders, `done/M10--dynamic-mesh-shadows` Task 7 (cut)) to *moving* lights: re-render a slot's cached static depth only when the light's transform changes, so projectile-attached and other moving lights don't re-render the full world every frame. The M10 plan builds the cache + restore seam fixed-light-first; this adds the invalidation-on-move path. Not sequenced until moving/projectile lights land.
- **Post-processing** — bloom, optional CRT/scanline filter.
- **Baked cubemap reflections** — `env_cubemap` point entity baked to a cubemap atlas at compile time.

### Infrastructure

- **Sector Graph + Portal Culling** — replace BSP-as-runtime-scaffolding with an author-defined sector graph. Latent portals (activate on event) support destruction reveals. Prerequisite for kinematic clusters that need their own sector graphs.
- **Chunk Primitive** — unify static world geometry, kinematic clusters, and dynamic debris into one record type (mesh + collider + transform + sector membership). Deferred until two or more of those consumers exist and the duplication cost is clear.
- **Audio foundation** — kira integration, spatial audio, reverb zones. → Sequenced as Milestone 12 (builds behind the entity event system; independent of the other upcoming milestones).
- **HUD and UI** — health, ammo, crosshair, menus. → Sequenced as Milestone 13 (see `context/research/ui-layer.md`).
- **`canonicalName` rename** — rename `classname` to `canonicalName` in scripting API and PRL. Source formats translate their identifier (Quake `.map` `classname`, UDMF thing-type, Blender prop) to this canonical name at compile time. Absence on an archetype means not directly placeable from source — script-spawned or marker-indirected only. Subsumes the `spawn_only` / `map_entity_classname` patterns into one field's presence.
- **FGD generated from script registry** — scripts are the single source of truth for entity archetypes. FGD emitted at script compile time, not hand-edited. Removes the divergence class of bug where registry and FGD describe different archetypes.
- **Composable archetypes via `@BaseClass` mixins** — `@BaseClass` declarations map to component lists; property bags drive behavior instead of proliferating archetype names. Reference patterns from `bevy_trenchbroom`: `Default::default()` as the property-fallback source, recursive depth-first base spawn with TypeId dedup, two-phase spawn (component insertion at load, subsystem registration at lifecycle hook).
- **Property-driven editor previews** — TrenchBroom expression-language helpers (`model({{ ... }})`, `iconsprite({{ ... }})`) drive per-instance preview variation. One canonical name can display different models or icons based on property values, reducing pressure to multiply archetype names.
- **Multi-format map support** — UDMF and others via `format/<name>.rs` sibling modules. All formats normalize to the canonical-name vocabulary at compile time, so runtime sees one identifier shape regardless of source.

### Dropped

- **SDF atlas + sphere-traced soft shadows** — ~~descoped in favor of the lightmap pipeline. Hard shadow edges fit the aesthetic; SDF complexity not justified.~~ **✂ Cut (2026-04):** old sphere-trace retired with the lighting-stack rework. **↩ Revived (2026-05):** SDF re-added as the static-lighting *direct-shadow* path. **Shipped (2026-06):** `plans/done/sdf-filterable-atlas/`, `plans/done/sdf-per-light-shadows/`, `plans/done/sdf-shadow-lightmap-uv-prepass/`, `plans/done/sdf-static-occluder-shadows/`.
- **Cubemap bake tool** — deferred indefinitely; baked cubemap reflections remain on the speculative list above but the standalone tool is dropped.
