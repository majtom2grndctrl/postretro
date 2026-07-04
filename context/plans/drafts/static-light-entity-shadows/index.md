# Static-Light Entity Shadows with SH Direct LOD

## Goal

Let compiler-selected static (baked-tier) lights cast crisp runtime shadows onto moving entities from the shadow pool — static geometry shadows the entity, entities shadow each other, and entities self-shadow — while the baked direct SH atlas serves as the distance LOD for entities too far to earn a pool slot. World-surface lighting stays fully baked and unchanged.

## Scope

### In scope

- Compiler heuristic that selects entity-shadow-eligible static lights (no per-light authoring KVP).
- Runtime promotion of selected lights into the existing spot/cube shadow pools, gated on portal reachability and entity presence.
- Entity receivers: promoted lights evaluated as runtime per-light terms in the skinned-mesh and billboard passes, attenuated by pool shadow maps (mesh) — crisp world→entity, entity→entity, and self-shadow.
- The LOD handoff: per-light direct SH delta tiles baked for selected lights; a compose pass subtracts each promoted light's delta from the direct SH atlas, weighted by a crossfade, so no receiver counts a light twice.
- Promoted-slot static depth cache: world depth rendered once per slot assignment, entities re-rendered per frame.
- Minimal idle cost: with no entity inside any eligible light's influence, the feature adds no per-frame GPU work.

### Out of scope

- Entity→world shadow receipt (world surfaces darkening under an entity's shadow from a static light) — the dependent plan `static-light-shadowmask-world-receipt`.
- Animated static lights (their direct term lives in the animated lightmap, not `DirectShVolume`; a different subtraction seam).
- Sun/directional lights (the pool holds spot and point shadows only; sun casts no runtime shadows today).
- Fog beam scatter from promoted lights (fog keeps the dynamic-only light set; adding beams on promotion would pop).
- Kinematic movers as shadow *occluders* (E17 decides mover draw paths; movers as *receivers* work automatically — they are lit like entities).
- Per-light shadow LOD beyond the single crossfade (no resolution tiers, no soft-shadow matching).
- Specular from promoted lights on entities (the mesh dynamic loop is diffuse-only today; promoted lights match it).
- Any change to SDF shadow machinery (untouched; this feature does not use it).

## Acceptance criteria

- [ ] Compiling a fixture map marks eligible static lights and excludes: lights below the intensity-ratio threshold, lights below the falloff-range floor, and decorative spots aimed into a nearby mounting surface — each exclusion covered by a compiler test.
- [ ] Per-light direct SH delta bake round-trips: base direct atlas minus all selected lights' deltas matches a bake with those lights excluded, within a pinned numeric tolerance (compiler test).
- [ ] A visible entity inside an eligible static light's influence gets that light promoted (within pool budget): the entity shows crisp shadow-map shadowing from static geometry, from other entities, and from itself (manual verification on `campaign-test.prl`, plus occluder-submitted counters).
- [ ] An entity outside promotion range is lit exactly as today — direct SH atlas sample, no runtime term. With nothing promoted, entity lighting output is unchanged from before this feature.
- [ ] Promotion and demotion crossfade over the pinned window: no single-frame brightness or shadow pop on the entity (manual verification; weights ramp monotonically).
- [ ] Total light energy on the entity holds across the handoff: a promoted light's runtime term replaces its subtracted SH delta at matching weight, so an unoccluded surface point does not brighten or dim beyond crispness differences (visual check with lighting isolation modes).
- [ ] With no entities inside any eligible light's influence: zero promoted slots, zero added shadow depth passes, direct-SH compose pass skipped (counters/log).
- [ ] After a promoted slot's cache warms, its per-frame world-geometry depth draws are zero; only entity occluders re-render (counter).
- [ ] World lighting is unchanged: lightmap bake output for existing maps is identical, and the forward world path does not evaluate promoted lights.
- [ ] Renderer budget guard tests still pass with unchanged sampled-texture counts; existing dynamic-light shadow tests stay green.

## Tasks

### Task 1: Split slot-ranking logic out of `spot_shadow.rs`

Behavior-preserving split. `crates/renderer/src/lighting/spot_shadow.rs` (~1123 lines) holds the pure ranking math (`assign_ranked_slots`, `rank_lights`) that Task 4 must extend. Move the wgpu-free ranking/eligibility code — spot ranking, and the cube ranking in `cube_shadow.rs` (`rank_point_lights`) — into the CPU-only `postretro_lighting` crate (`crates/lighting/`), alongside the existing `entity_occluder_eligible` predicate. GPU pool resource code stays in the renderer. All call sites (`renderer_light_slots.rs`) update in the same change; existing ranking tests move with the code and stay green. No behavior change.

### Task 2: Compiler selection of entity-shadow-eligible static lights

In `prl-build`, select eligible lights from the same domain `DirectShVolume` bakes (baked-tier point/spot lights, `shadow_type static_light_map`, not animated, not bake-only; sun excluded). Exclude a light when any of: (a) intensity below `entity_shadow_min_intensity_ratio` × the map's maximum static-light intensity (worldspawn KVP, default 0.5); (b) `falloff_range` below `entity_shadow_min_range` (worldspawn KVP, default 4.0 m); (c) decorative-fixture test, spot lights only — cast 256 stratified rays over the cone using the existing bake raytracing context; if ≥ 75% of rays hit geometry within min(1.5 m, 0.25 × falloff_range), the spot is aimed into its mounting surface and is excluded. Emit the selected set as a new PRL section (`EntityShadowLights`, flat `u32` light indices — see Wire format); allocate its SectionId from the registry in `build_pipeline.md` and update the registry in the same change. Add the two worldspawn KVPs to the FGD worldspawn class. Loader (`postretro-level-format` + `level-loader`) exposes the set; absent section = empty set (older PRLs load fine). Compiler tests cover each exclusion rule and the section round-trip. Note: `lightmap_bake.rs` (~3818 lines) must not grow — selection lives in a new compiler module.

### Task 3: Per-light direct SH delta bake

For each selected light, bake its individual contribution to the direct SH atlas — the same per-light math `direct_sh_bake.rs` accumulates before summing (occlusion-tested radiance → L2 SH → cosine lobe; the cosine lobe is linear per coefficient, so per-light deltas sum exactly to the summed bake pre-compression). Store sparsely as a new PRL section (`DirectShDeltaVolumes`) mirroring the animated `DeltaShVolumes` layout (section id 27): affinity-cell CSR (`AFFINITY_FACTOR = 4`) keyed by selected-light index, one dense 64-probe f16 RGBA octahedral sub-block per (cell, light) entry, deltas clipped to each light's reach (reuse the existing affinity reach index in `direct_sh_bake.rs`). SectionId from the registry, updated in the same change. Loader decodes to the same CPU-side shape the animated delta loader produces. Compiler test: base minus all deltas equals a bake excluding the selected lights within pinned tolerance (pre-BC6H values).

### Task 4: Runtime promotion, weights, and the count-split light buffer

Extend slot candidacy beyond `is_dynamic`: a selected static light is a shadow-slot candidate when it passes the existing reachability gate (`light_reaches_visible_cell`) AND at least one shadow-relevant entity instance intersects its influence sphere — use the same instance set that can render into shadow depth passes (the shadow-caster set), not the narrower forward-visible set. Candidates rank in the existing pools with the existing `(falloff_range / distance)²` score, competing with dynamic lights; promoted static slots are entity-occluder-eligible unconditionally (extend the `entity_occluder_eligible` predicate from Task 1's new home). Each selected light carries a promotion weight `w ∈ [0,1]`: fades in over 0.3 s on promotion, fades out over 0.3 s on demotion with a 0.5 s sticky window before the slot frees (mirror the fog-pass hysteresis pattern; a demoting slot stays assigned until `w` reaches 0). Light buffer becomes count-split: dynamic-tier records first, promoted static records appended (packed via `pack_light_with_slot` with color premultiplied by `w`, slot indices patched as today). The forward world loop, fog scatter loops, and any other dynamic-only consumer must bound iteration by the dynamic-only count; the mesh and billboard passes bound by the total count (mesh already carries its own `light_count` in `mesh_light_params`; billboard needs the total in its uniforms). Enumerate and pin every loop-bound consumer with a test — if any consumer currently bounds by `arrayLength`, switch it to the count uniform in this task. Publish the per-frame promoted set (light index, slot, `w`) for Tasks 5–6. No mesh/billboard shader lighting-math changes: promoted records evaluate through the existing loops, shadow sampling included (mesh) / diffuse-only unshadowed (billboard, matching its dynamic handling today).

### Task 5: Direct SH compose pass

New compute pass producing a composed direct atlas: `composed = base − Σ (w_i × delta_i)` over the promoted set, clamped at zero — the direct-atlas sibling of the shipped `sh_compose.rs` animated-delta pass, reusing its CSR binding shape and texel→probe→affinity-cell mapping. Output is a storage-writable `Rgba16Float` texture at base-atlas dimensions (BC6H base is sampled, not copied). The mesh (group 4, `BIND_SH_DIRECT_ATLAS`) and billboard (group 3, binding 15) direct-atlas bindings switch to the composed texture; BGL type unchanged, sampled-texture counts unchanged. Pass scheduling: runs when any `w > 0`, plus one copy-through dispatch on level load and one on the transition back to all-zero; skipped otherwise. Subtraction targets pre-compression radiance, so BC6H decode error is the accepted residual (bounded by the Task 3 tolerance AC).

### Task 6: Promoted-slot static depth cache

For a slot occupied by a promoted static light, world depth is constant while the assignment holds: render cone/face-culled world geometry into a cache layer once on assignment (and on level reload), then per frame copy the cached depth into the live slot/face and draw only entity occluders on top (`LoadOp::Load` after the copy). Spot slots and cube faces both covered; the cube path must preserve the occupied-faces-always-initialized invariant (`cube_face_needs_clear` semantics — the copy replaces the clear, never leaves stale depth). Invalidate on slot reassignment. Dynamic lights keep today's per-frame world render. Add counters: promoted count, cached-world-render skips, entity occluders submitted for promoted slots. Touches `renderer_shadow_passes.rs` (~879 lines) — keep additions in a new module (e.g. a depth-cache helper) rather than growing the pass file.

## Sequencing

**Phase 1 (concurrent):** Task 1 (ranking split — blocks Task 4), Task 2 (compiler selection — blocks Tasks 3 and 4).
**Phase 2 (concurrent):** Task 3 (consumes Task 2's selected set), Task 4 (consumes Task 1's split modules and Task 2's PRL section).
**Phase 3 (concurrent):** Task 5 (consumes Task 3's deltas and Task 4's promoted set), Task 6 (consumes Task 4's promoted slots).

## Rough sketch

- Selection: new compiler module (e.g. `entity_shadow_select.rs`) consuming the `StaticBakedLights` namespace; decorative test reuses `RaytracingCtx`.
- Promotion driver: `renderer_light_slots.rs::update_dynamic_light_slots` grows a promoted-static candidate stage; predicates live in `crates/lighting/src` next to `entity_occluder_eligible` (post Task 1).
- The double-count invariant, restated for this feature: a selected light reaches an entity through exactly one blend — `(1 − w) × SH-delta path + w × runtime-term path` — and reaches world surfaces through the lightmap only.
- Entity-presence test is lights × collected instances CPU math (sphere vs instance position, entity bound radius padded) — wgpu-free, lives in `postretro_lighting` or `render-cpu`.
- Coordination: E19 `render-cpu` (in progress) is relocating CPU packers/planners this feature touches (`mesh_instances.rs`, frame uniforms, sh-compose packers) — rebase-sensitive, not design-sensitive. `runtime-cell-spatial-contract` (in progress) renames cell-locate APIs the reachability gate uses. `single-source-animated-light-brightness` (draft) touches the same slot-eligibility seam in `update_dynamic_light_slots`.

## Boundary inventory

| Name | Rust | Wire / serde | FGD KVP |
|---|---|---|---|
| Intensity-ratio threshold | `entity_shadow_min_intensity_ratio` (worldspawn parse) | n/a (compile-time only) | `entity_shadow_min_intensity_ratio` (worldspawn) |
| Falloff floor | `entity_shadow_min_range` (worldspawn parse) | n/a (compile-time only) | `entity_shadow_min_range` (worldspawn) |
| Selected-light section | `EntityShadowLightsSection` | PRL section, new id | n/a |
| Per-light direct deltas | `DirectShDeltaVolumesSection` | PRL section, new id | n/a |

No script/Luau/JS surface. No per-light KVP — selection is compiler-owned.

## Wire format

Both sections little-endian, matching existing PRL conventions.

- **EntityShadowLights**: `u32` count, then `count × u32` light indices (ascending, indices into the level light array). Empty selection = section omitted; loaders treat absence as empty.
- **DirectShDeltaVolumes**: mirrors `DeltaShVolumes` (section id 27, `crates/level-format/src/delta_sh_volumes.rs`) field-for-field — same header/grid geometry as the direct SH volume it deltas against, `affinity_offsets` (`u32`, len cell_count+1), `affinity_lights` (`u32`, flat, grouped by cell), dense 64-probe f16 RGBA octahedral sub-blocks index-parallel to `affinity_lights` — except: light indices refer to the EntityShadowLights selection domain, and there is no animation-descriptor mapping (promotion weight is runtime state, not baked). Empty selection = section omitted.

Both SectionIds allocated as the next free entries in the `build_pipeline.md` registry, registry updated in the same commit that adds each section.

## Open questions

- Threshold defaults (0.5 intensity ratio, 4.0 m range floor, decorative 75%/1.5 m) are first-guess pins — expect tuning against `campaign-test.prl` during Task 2; the worldspawn KVPs exist so maps can adjust without engine changes. Decorative-test constants stay code constants unless tuning proves they need authoring exposure.
- No mapper force-include/exclude override per project owner's direction. If the heuristic misfires on real maps, revisit as a follow-up (likely a worldspawn list, not a per-light KVP).
- Point lights compete for only 6 cube slots; if promoted statics starve dynamic point lights in practice, a tier reservation policy is the follow-up knob.
