# Implementation Roadmap

> **Lifecycle:** reviewed and updated at the start of each milestone. Deleted when all milestones are complete.
> **Purpose:** milestone-by-milestone plan from "wgpu window exists" through a moddable, playable game. Each milestone produces something visible and testable.
> **Related:** `context/lib/index.md`, `context/lib/rendering_pipeline.md`

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
- [x] **CSM sun shadows** — 3 cascades, 1024², bounding-sphere fit with rotation-invariant texel snapping. Hard edges match aesthetic.
- [x] **Runtime probe sampling** — parse SH section as 3D texture; trilinear sample in world shader for both static surfaces and dynamic entities.
- [x] **Animated SH layers** — per-light monochrome SH layers, animation descriptor + sample buffers, per-frame brightness/color curve evaluation in the fragment shader.
- [x] **Lightmaps** — per-face baked direct lighting; static surfaces sample lightmap atlas; dynamic entities fall back to probe grid.

**Testable outcome:** textured level with probe-sampled indirect, lightmapped static surfaces, CSM-driven sun shadows, and animated light layers. ✓

**Scope note:** SDF sphere-traced soft shadows and specular maps were descoped. See the future section.

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
- [x] **Reference behaviors (script)** — `RotatorDriver` and `DamageSource` written as scripts. See `content/dev/scripts/`.
- [x] **Modder-facing API reference** — covers all bound APIs. See `docs/scripting-reference.md`.

**Testable outcome:** spawn a scripted entity from a `.map` file; confirm it ticks and emits events at the fixed tick rate. Hot-reload the script during gameplay. The `DamageSource` debug entity is available for future destruction testing. ✓

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
- [x] **Shader anisotropic filtering** — per-pixel manual aniso in `forward.wgsl`, derivative-gated, N taps of `textureSampleGrad` along the major axis. Preserves nearest-filter chunky look in-plane while killing grazing-angle shimmer. Depends on baked texture mips. `context/plans/done/shader-anisotropic-filtering/` (shipped, then retired with True Retro mode)
- [x] **Graphics mode toggle** — introduced Post Retro and True Retro runtime filtering modes; `GraphicsMode` enum, `defaultGraphicsMode` mod-manifest key, egui combo. Scaffolding subsequently removed by Retire True Retro mode. `context/plans/done/graphics-mode-toggle/`
- [x] **BC5 normal compression** — BC5 encoder in prl-build, BC5 `format_tag` value in `.prm`, GPU upload path. Normals only — BC1/BC7 fight the pixel-art aesthetic on diffuse. Additive: `format_tag` is extensible from day one, no version bump. Also retires the Post Retro normal-averaging bias under hardware aniso. `context/plans/done/prm-bc5-normals/`
- [x] **Retire True Retro mode** — deleted manual-aniso shader code and True Retro branches in `forward.wgsl`; unwound `GraphicsMode` enum, `defaultGraphicsMode` mod-manifest key, nearest sampler pool, and egui mode combo. Post Retro normal path retains existing `(rgb*2-1)->normalize` decode unchanged. `context/plans/done/retire-true-retro/`
- [ ] **Texture pack format (optional)** — shipping consolidation of `.prl-cache/tex/` into a single pack file. **Deferred** until there are more real textures and the iteration-vs-ship tension actually appears. `context/plans/drafts/texture-pack-format/`

**Testable outcome:** Post Retro mode renders with no grazing-angle shimmer and crisp hardware-aniso filtering; True Retro opt-in is removed; normals are ~50% smaller on disk and in VRAM; level load does zero CPU mip work. ✓

**Status note:** all tasks shipped except the optional Texture pack format, deferred until there are more real textures.

---

## Milestone 9: Diffuse GI Upgrade (depth-aware probes + fog)

Kill light-leak-through-walls by adding per-probe visibility data to the Milestone 5 SH irradiance volume, then extend the fog system. The depth-aware interpolant replaces the plain trilinear SH sample entirely — one runtime path. Probe streaming is deferred: this milestone keeps probes resident and produces the VRAM-fit measurement that decides whether streaming ever becomes its own milestone.

**Assumes the shipped Milestone 5 lighting foundation** — SH irradiance volume + baker, runtime probe sampling (SH as a 3D texture), lightmaps, and CSM sun shadows are all in place. Milestone 9 is a pure upgrade layer on top; it builds nothing M5 already delivers. Independent of Milestones 7–8.

**Pre-milestone fix — already satisfied:** the fog pass (`src/render/fog_pass.rs`) is imported, owned by `Renderer`, and runs in every frame's render stage. It was wired as part of the portal-fog-culling work, not as a standalone M9 prerequisite. See `rendering_pipeline.md` §7.5 and `plans/done/perf-portal-fog-culling/`. Directional fog (below) builds directly on the live pass.

Plans ship in this sequence:

- [ ] **Probe weight correctness (no new data)** — in the world shader: reject trilinear corners facing away from the surface normal, exclude invalid (zero-packed) probes from the blend, renormalize remaining weights. Pure ALU. Fixes a latent bug where invalid probes drag near-wall surfaces toward black — independent of DDGI, and a prerequisite the depth-aware interpolant needs anyway. **Measurement gate:** record residual smear/leak here to quantify what the depth atlas buys before paying for it.
- [ ] **Probe depth/visibility atlas (bake)** — prl-build stage baking per-probe depth moments alongside the existing SH bands, ray-cast through the Milestone 4 BVH. Format kept chunk-friendly so a later brick split needs no interpolant rewrite (deferred-streaming insurance). New/extended PRL section.
- [ ] **Depth-aware runtime interpolant** — replace the trilinear SH sample with a visibility-weighted (Chebyshev) interpolant in the world shader, for both static surfaces and dynamic entities. Removes the plain-trilinear path. Depends on the depth atlas.
- [ ] **Directional fog** — extend the live fog pass with the directional term, on top of the existing volumetric fog scope.
- [ ] **Memory-budget checkpoint + coarse open-area spacing** — VRAM budget readout plus coarser probe spacing in open volumes; produces the empirical resident-fit number that gates any future streaming milestone.

**Testable outcome:** near-wall surfaces no longer darken from invalid-probe averaging; indirect light no longer bleeds through walls (visibility-weighted probe sampling); a single resident probe representation drives both static and dynamic surfaces; the fog pass runs with a working directional term; residual-smear and VRAM budget for a representative large map are both recorded.

---

## Milestone 10: First Playable (Vertical Slice)

The first playable slice. Its job is to answer one question: does this foundation feel good enough to commit a full game to? Build the real weapon and enemy primitive layers — genuine primitive surface plus SDK reference implementations, refined in later passes, **not throwaway stubs** — and a placeholder sound layer so combat and enemies aren't judged silent. Enemy AI/navigation is the highest-uncertainty system on the whole roadmap; it is sequenced here, early, precisely to surface any foundation problem while the engine bet can still change.

**Prerequisite:** Milestone 6 (entity model + scripting + damage events) ✓ and Milestone 7 (grounded movement) ✓.

Plans ship in this sequence:

- [ ] **Basic weapon primitives** — engine-level weapon primitive surface (hitscan first, projectile to follow); first weapons authored in the SDK as reference behaviors. Testable against the static world (impacts, the `DamageSource` entity) before any enemy exists. A foundation to refine, not a stub.
- [ ] **Basic enemy primitives** — enemy primitive surface (simple AI state machine + navigation against the Milestone 7 trimesh collider, damageable); first enemy authored in the SDK. Front-loaded as the highest-uncertainty system — it validates whether the foundation carries AI and navigation. A foundation to refine, not a stub.
- [ ] **Stub sound layer (Nintendo-style SFX)** — placeholder retro SFX (fire, impact, footstep, alert, pain) wired through entity-emitted sound events so combat reads correctly. Explicitly a stub: the sound-event hooks are durable, the assets and backend are placeholder. The real spatial audio foundation lands in Milestone 11.

**Testable outcome:** shoot a foundational weapon and kill a foundational enemy that navigates and reacts, with placeholder SFX selling the feel — enough to judge whether to commit a full game to PostRetro.

---

## Milestone 11: Sound Foundation

Replace the Milestone 10 stub SFX layer with a real audio foundation: kira integration, spatial/3D audio, and reverb zones. The Milestone 10 sound-event hooks were designed so this swaps in behind them — no weapon or enemy code should need to change.

**Prerequisite:** Milestone 10 (stub sound layer + sound-event hooks to build behind).

Plans ship in this sequence:

- [ ] **kira integration** — audio subsystem in its own module (subsystem-boundary principle); mixing, buses, lifecycle.
- [ ] **Spatial audio** — positional sources with attenuation; listener driven by the player/camera.
- [ ] **Reverb zones** — runtime playback for `env_reverb_zone` acoustic zones (baked data already resolves them to leaves at load; see `context/lib/audio.md`).
- [ ] **Replace stub SFX** — route the Milestone 10 sound-event hooks through real mixed, spatialized playback; retire the placeholder layer.

**Testable outcome:** spatialized combat and ambient audio; reverb zones audibly change acoustics; the Milestone 10 stub layer is fully replaced with no changes to weapon or enemy code.

---

## Milestone 12: UI

The full UI/HUD layer — health, ammo, crosshair, and menus — replacing the debug egui stand-in used during the vertical slice. The detailed design is captured in `context/research/ui-layer.md`; it is promoted to a ready plan when this milestone opens.

**Prerequisite:** Milestone 10 (gameplay state to surface — health, ammo, hit feedback).

Plans ship in this sequence:

- [ ] **UI layer** — HUD, menu system, and supporting UI primitives per `context/research/ui-layer.md`. Detailed sequencing is finalized when the research doc is promoted.

**Testable outcome:** a playable HUD (health / ammo / crosshair) and functional menus drive the slice without the debug overlay; the design from `ui-layer.md` is realized.

---

## Future / Speculative

Features below are intended but not yet sequenced. Rough priority ordering within each group.

### Gameplay systems

- **Weapons** — foundation (weapon primitives + first SDK weapons) lands in Milestone 10. Remaining/speculative refinements: projectile variety, viewmodel hooks, and at least one weapon that triggers chunk destruction.
- **NPC Entities** — first enemy plus the nav/AI foundation lands in Milestone 10. Remaining/speculative refinements: richer scripted AI state machines (patrol / chase / attack), line-of-sight and navigation queries, and multiple enemy archetypes.
- **World Entities** — common base scripts for doors, pickups, trigger volumes, timeline/sequence helpers; a scripted ambush set piece with destruction choreography.

### Moving and destructible geometry

- **Kinematic Clusters** — sub-worlds compiled like the main world but with a runtime transform (elevators, barges). Cluster authoring in TrenchBroom, compiler emits per-cluster geometry, `KinematicDriver` entity sets transform each tick. Dynamic portals at cluster boundaries when aligned with a static sector portal.
- **Destruction (Pre-Fracture + Promotion)** — brushes pre-fractured into pieces with dependency edges at compile time. Runtime promotes pieces from static to dynamic on damage; reveals pre-authored interior break-faces. Requires a full rigidbody solver (Rapier) for debris physics. Latent portals activated on fracture to open hidden areas.

### Rendering and visual polish

- **Billboard sprite rendering** — ~~character and effect sprites; depth-sort against world geometry.~~ **Shipped.** `BillboardEmitter` entity type, particle sim, and additive billboard pass (`src/fx/smoke.rs`, `billboard.wgsl`). See `plans/done/scripting-foundation/plan-3-emitter-entity.md`.
- **Specular maps** — ~~per-texel specular highlights in the direct light loop. Shading model decision (Phong vs. PBR) required first.~~ **Shipped.** Blinn-Phong per-texel specular via `_s.png` siblings, chunk-list multi-source loop, bumped-Lambert correction. See `plans/done/normal-maps/`.
- **Fog volumes** — `env_fog_volume` brush entity wired to a runtime fog pass. Pass is wired and runs each frame (with portal-fog culling — see `plans/done/perf-portal-fog-culling/` and `rendering_pipeline.md` §7.5). Only the **directional fog** term remains, tracked under Milestone 9.
- **Emissive / fullbright surfaces** — ~~texture prefix or material flag for self-lit surfaces.~~ **Shipped.** `emissive_` prefix → `Material::Emissive`; `emissive_intensity` uniform; bloom-ready bypass in forward shader. See `plans/done/emissive-surfaces/`.
- **Post-processing** — bloom, optional CRT/scanline filter.
- **Baked cubemap reflections** — `env_cubemap` point entity baked to a cubemap atlas at compile time.

### Infrastructure

- **Sector Graph + Portal Culling** — replace BSP-as-runtime-scaffolding with an author-defined sector graph. Latent portals (activate on event) support destruction reveals. Prerequisite for kinematic clusters that need their own sector graphs.
- **Chunk Primitive** — unify static world geometry, kinematic clusters, and dynamic debris into one record type (mesh + collider + transform + sector membership). Deferred until two or more of those consumers exist and the duplication cost is clear.
- **Audio foundation** — kira integration, spatial audio, reverb zones. → Sequenced as Milestone 11 (real layer replacing the Milestone 10 stub SFX).
- **HUD and UI** — health, ammo, crosshair, menus. → Sequenced as Milestone 12 (see `context/research/ui-layer.md`).
- **`canonicalName` rename** — rename `classname` to `canonicalName` in scripting API and PRL. Source formats translate their identifier (Quake `.map` `classname`, UDMF thing-type, Blender prop) to this canonical name at compile time. Absence on an archetype means not directly placeable from source — script-spawned or marker-indirected only. Subsumes the `spawn_only` / `map_entity_classname` patterns into one field's presence.
- **FGD generated from script registry** — scripts are the single source of truth for entity archetypes. FGD emitted at script compile time, not hand-edited. Removes the divergence class of bug where registry and FGD describe different archetypes.
- **Composable archetypes via `@BaseClass` mixins** — `@BaseClass` declarations map to component lists; property bags drive behavior instead of proliferating archetype names. Reference patterns from `bevy_trenchbroom`: `Default::default()` as the property-fallback source, recursive depth-first base spawn with TypeId dedup, two-phase spawn (component insertion at load, subsystem registration at lifecycle hook).
- **Property-driven editor previews** — TrenchBroom expression-language helpers (`model({{ ... }})`, `iconsprite({{ ... }})`) drive per-instance preview variation. One canonical name can display different models or icons based on property values, reducing pressure to multiply archetype names.
- **Multi-format map support** — UDMF and others via `format/<name>.rs` sibling modules. All formats normalize to the canonical-name vocabulary at compile time, so runtime sees one identifier shape regardless of source.

### Dropped

- **SDF atlas + sphere-traced soft shadows** — descoped in favor of the lightmap pipeline. Hard shadow edges fit the aesthetic; SDF complexity not justified.
- **Cubemap bake tool** — deferred indefinitely; baked cubemap reflections remain on the speculative list above but the standalone tool is dropped.
