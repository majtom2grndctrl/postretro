# Static-Light Entity Shadows with SH Direct LOD

## Goal

Let compiler-selected static (baked-tier) lights cast crisp runtime shadows onto moving entities from the shadow pool — static geometry shadows the entity, entities shadow each other, and entities self-shadow — while the baked direct SH atlas serves as the distance LOD for entities too far to earn a pool slot. World-surface lighting stays fully baked and unchanged.

## Scope

### In scope

- Compiler heuristic that selects entity-shadow-eligible static lights (no per-light authoring KVP).
- Runtime promotion of selected lights into the existing spot/cube shadow pools, gated on portal reachability and entity presence, capped by a promoted-slot budget per pool.
- Entity receivers: promoted lights evaluated as runtime per-light terms in the skinned-mesh pass, attenuated by pool shadow maps — crisp world→entity, entity→entity, and self-shadow. Billboards evaluate promoted lights exactly as they evaluate dynamic lights today (per-vertex, unshadowed): a sprite inside a promoted light's shadowed region over-brightens by that light's term, the same artifact class billboards already accept for dynamic lights — pinned as accepted.
- The LOD handoff: per-light direct SH delta tiles baked for selected lights; a compose pass subtracts each promoted light's delta from the direct SH atlas, weighted by a crossfade, so no receiver counts a light twice. With an empty selection the composed atlas is never allocated and the base atlas stays bound.
- Promoted-slot static depth cache sized to the promoted budget: world depth rendered once per slot assignment, entities re-rendered per frame.
- Minimal idle cost: with no entity inside any eligible light's influence, the feature adds no per-frame GPU work.

### Out of scope

- Entity→world shadow receipt (world surfaces darkening under an entity's shadow from a static light) — the dependent plan `static-light-shadowmask-world-receipt`.
- Animated static lights (their direct term lives in the animated lightmap, not `DirectShVolume`; a different subtraction seam).
- Sun/directional lights (the pool holds spot and point shadows only; sun casts no runtime shadows today).
- Fog beam scatter from promoted lights (beams appearing on promotion would pop; fog's slot-driven light collection gets an explicit not-promoted filter — see Task 4).
- Kinematic movers as shadow *occluders* (E17 decides mover draw paths; movers as *receivers* work automatically — they are lit like entities).
- Per-light shadow LOD beyond the single crossfade (no resolution tiers, no soft-shadow matching).
- Specular from promoted lights on entities (the mesh dynamic loop is diffuse-only today; promoted lights match it).
- Shadowed billboard receipt (billboards receive all runtime lights unshadowed today; promoted lights match that).
- Any change to SDF shadow machinery (untouched; this feature does not use it).

## Acceptance criteria

Tags name the producing task(s); every task verifies its own tags.

- [ ] (T2) Compiling a fixture map marks eligible static lights and excludes: lights below the intensity-ratio threshold, lights below the falloff-range floor, and decorative spots aimed into a nearby mounting surface — each exclusion covered by a compiler test against fixtures authored in T2. The `EntityShadowLights` section round-trips through the loader, and is emitted only when a `DirectShVolume` section bakes.
- [ ] (T3) Per-light direct SH delta bake round-trips: base direct atlas minus all selected lights' deltas matches a bake with those lights excluded, within a pinned numeric tolerance (compiler test, pre-BC6H values).
- [ ] (T4) The promoted-set record schema is pinned by a layout/contract test: each record carries the global level-light index, the selection index (position in `EntityShadowLights` order), the pool kind and slot (spot slot, or cube slot), and weight `w ∈ [0,1]`. Tasks 5–6 and the dependent shadowmask plan consume this exact record.
- [ ] (T4+T6) A visible entity inside an eligible static light's influence gets that light promoted (within the promoted budget and pool ranking): the entity shows crisp shadow-map shadowing from static geometry, from other entities, and from itself (manual verification on `campaign-test.prl`, plus occluder-submitted counters).
- [ ] (T4+T5) An entity outside promotion range is lit exactly as today — direct SH atlas sample, no runtime term. With an empty or absent selection, the composed atlas is not allocated, the base atlas stays bound, and entity lighting is byte-identical to before this feature.
- [ ] (T4) Promotion, demotion, and eviction follow the pinned weight timeline: no single-frame brightness pop on promote/demote (weights ramp; fades reversible from current `w`); eviction may briefly under-light the entity (runtime term drops with the slot while the delta subtraction fades out) but never over-lights.
- [ ] (T4+T5) Total light energy on the entity holds across the handoff: a promoted light's runtime term replaces its subtracted SH delta at matching weight, so an unoccluded surface point does not brighten or dim beyond crispness differences (visual check with lighting isolation modes).
- [ ] (T4) Fog scatter output is unchanged while a light is promoted (packer filter test: promoted slots are excluded from fog's slot-driven light collection).
- [ ] (T4+T5+T6) With no entities inside any eligible light's influence: zero promoted slots, zero added shadow depth passes, direct-SH compose pass skipped (counters/log).
- [ ] (T6) After a promoted slot's cache warms, its per-frame world-geometry depth draws AND its per-frame shadow-cull sub-region dispatches are both zero; only entity occluders re-render (counters).
- [ ] (T4) World lighting is unchanged: lightmap bake output for existing maps is identical, and the forward world path does not evaluate promoted lights (forward bounds by the dynamic-only count; pinned by test).
- [ ] (T1) Ranking/eligibility tests relocate to their new module and stay green; existing dynamic-light shadow tests stay green.
- [ ] (T5) Renderer budget guard tests still pass with unchanged sampled-texture counts.

## Tasks

### Task 1: Split slot-ranking logic out of `spot_shadow.rs`

Behavior-preserving split. `crates/renderer/src/lighting/spot_shadow.rs` (~1123 lines) holds the pure ranking math (`assign_ranked_slots`, `rank_lights`) that Task 4 must extend. Move the wgpu-free ranking/eligibility code — spot ranking, and the cube ranking in `cube_shadow.rs` (`rank_point_lights`) — into the CPU-only `postretro_lighting` crate (`crates/lighting/`), alongside the existing `entity_occluder_eligible` predicate. GPU pool resource code stays in the renderer. All call sites (`renderer_light_slots.rs`) update in the same change; existing ranking tests move with the code and stay green. No behavior change.

### Task 2: Compiler selection of entity-shadow-eligible static lights

In `prl-build`, select eligible lights: baked-tier point and spot lights with `shadow_type static_light_map`, not animated, not `bake_only`, not directional. Note this is a strict SUBSET of the `DirectShVolume` contributor set — the atlas filters neither `bake_only` nor directional (`direct_sh_bake.rs` filters only `StaticLightMap` on top of `StaticBakedLights`), so do not reuse its filter verbatim; subtraction stays valid because selection ⊆ contributors, and `bake_only` lights must be excluded anyway (they are dropped from the on-disk runtime light array, so they have no runtime index). Exclude a light when any of: (a) intensity below `entity_shadow_min_intensity_ratio` × the map's maximum static-light intensity (worldspawn KVP, default 0.5); (b) `falloff_range` below `entity_shadow_min_range` (worldspawn KVP, default 4.0 m); (c) decorative-fixture test, spot lights only — cast 256 stratified rays over the cone using the existing bake raytracing context; if ≥ 75% of rays hit geometry within min(1.5 m, 0.25 × falloff_range), the spot is aimed into its mounting surface and is excluded. Emit the selected set as PRL section `EntityShadowLights` (SectionId 40): `u32` count, then ascending `u32` light indices into the level light array; emitted only when a `DirectShVolume` section bakes for the map, omitted otherwise or when the selection is empty. Update the SectionId registry in `build_pipeline.md` in the same change. Add the two worldspawn KVPs to the FGD worldspawn class. Loader (`postretro-level-format` + `level-loader`) exposes the set; absent section = empty set (older PRLs load fine); a nonempty section with no `DirectShVolume` present is treated as empty by the loader. Author the test fixtures (programmatic lights or a dedicated `.map`) exercising each exclusion rule, plus a section round-trip test. Selection lives in a new compiler module — `lightmap_bake.rs` (~3818 lines) must not grow.

### Task 3: Per-light direct SH delta bake

For each selected light in the `EntityShadowLights` set, bake its individual contribution to the direct SH atlas — the same per-light math `direct_sh_bake.rs` accumulates before summing (occlusion-tested radiance → L2 SH → cosine lobe; `apply_cosine_lobe_rgb` is a per-band scalar multiply, linear per coefficient, so per-light deltas sum exactly to the summed bake pre-compression). Store sparsely as PRL section `DirectShDeltaVolumes` (SectionId 41) mirroring the animated `DeltaShVolumes` layout (section id 27): affinity-cell CSR (`AFFINITY_FACTOR = 4`) — `affinity_offsets` (`u32`, len cell_count+1), `affinity_lights` (`u32`, flat, grouped by cell), one dense 64-probe f16 RGBA octahedral sub-block per (cell, light) entry — with deltas clipped to each light's reach (reuse the existing affinity reach index in `direct_sh_bake.rs`), and no animation-descriptor mapping. The `affinity_lights` entries are SELECTION indices — 0-based positions in `EntityShadowLights` order — not level-light-array indices; pin this with a test. Update the SectionId registry in the same change. Loader decodes to the same CPU-side shape the animated delta loader produces. Compiler test: base minus all deltas equals a bake excluding the selected lights within pinned tolerance (pre-BC6H values).

### Task 4: Runtime promotion, weights, and the count-split light buffer

Extend slot candidacy beyond `is_dynamic`: a selected static light is a shadow-slot candidate when it passes the existing reachability gate (`light_reaches_visible_cell`) AND at least one shadow-relevant entity instance intersects its influence sphere — use the same instance set that can render into shadow depth passes (the shadow-caster set), not the narrower forward-visible set. Concurrent promotions are capped per pool: `MAX_PROMOTED_SPOT = 8`, `MAX_PROMOTED_CUBE = 2` (named constants; the top-scoring static candidates up to the cap enter the pool ranking). Candidates rank in the existing pools with the existing `(falloff_range / distance)²` score, competing with dynamic lights; promoted static slots are entity-occluder-eligible unconditionally (extend the `entity_occluder_eligible` predicate from Task 1's new home). On adapters without `CUBE_ARRAY_TEXTURES`, selected point lights are never candidates — they stay on the SH path permanently.

**Weight timeline (pinned).** Each selected light carries a promotion weight `w ∈ [0,1]`. Promote: `w` ramps 0→1 over 0.3 s. Demote (gate fails — entity left, light unreachable): the slot is held with `w` unchanged for a 0.5 s sticky window (absorbs transients; re-promotion during the window resumes from current `w`), then `w` ramps to 0 over 0.3 s with the slot still assigned; the slot frees at `w = 0`. Evict (outranked): a slot with `w > 0` may be taken only by a candidate whose score exceeds the incumbent's by a 1.25× hysteresis margin; on eviction the runtime term and entity-occluder draws drop with the slot immediately, while `w` (now governing only the SH-delta subtraction) ramps to 0 over 0.3 s — the handoff errs dark, never over-bright. All fades are reversible from the current `w`. Mirror the fog-pass hysteresis pattern (`compute_active_mask_with_hysteresis`).

**Promoted-set contract (pinned; consumed by Tasks 5–6 and the dependent shadowmask plan).** The renderer owns a per-frame CPU promoted set; each record: global level-light index, selection index (position in `EntityShadowLights` order), pool kind + slot (spot slot, or cube slot), `w`. From it the renderer uploads a per-selected-light weight buffer (`f32`, length = selection count, indexed by selection index, zero when unpromoted) for Task 5's compose pass. No forward-pass GPU buffer is created here — the dependent plan builds its own forward upload from the CPU set. Schema pinned by a contract test (the T4 schema AC).

**Count-split light buffer.** Dynamic-tier records first, promoted static records appended (packed via `pack_light_with_slot` with color premultiplied by `w`, slot indices patched as today). Loop-bound consumers, enumerated: the forward world loop reads `uniforms.light_count` (group-0 `Uniforms`) — stays dynamic-only; the billboard pass reads the same shared group-0 uniform struct (3-way byte contract) — add a `total_light_count` field to the shared struct tail and bound the billboard loop by it (billboards evaluate promoted lights per-vertex, unshadowed, exactly as dynamic lights today); the mesh pass reads `mesh_light_params.light_count` — set to the total. Both counts written by the existing frame-uniform upload path. Fog is NOT a count consumer: `collect_fog_spot_lights` walks `spot_shadow_pool.slot_assignment` with no tier test, so promoted static spots would leak into fog scatter — add an explicit not-promoted filter there (and its cube counterpart if one exists), pinned by a packer test. Pin every enumerated bound with a test. No mesh/billboard shader lighting-math changes: promoted records evaluate through the existing loops, shadow sampling included (mesh).

### Task 5: Direct SH compose pass

New compute pass producing a composed direct atlas: `composed = base − Σ (w_i × delta_i)` over the selection (weights from Task 4's per-selected-light weight buffer, zero for unpromoted lights), clamped at zero — the direct-atlas sibling of the shipped `sh_compose.rs` animated-delta pass, reusing its CSR binding shape and texel→probe→affinity-cell mapping (delta `affinity_lights` entries are selection indices, matching the weight buffer's indexing). Output is a storage-writable `Rgba16Float` texture at base-atlas dimensions (BC6H base is sampled, not copied). Binding policy: when the selection is empty or absent, the composed texture is never allocated and the mesh (group 4, `BIND_SH_DIRECT_ATLAS`) and billboard (group 3, binding 15) bindings keep the base atlas — old maps pay zero VRAM and stay byte-identical; with a nonempty selection those bindings take the composed texture (BGL type unchanged, sampled-texture counts unchanged). Pass ordering: runs in the visibility/culling prepass tier, after the promotion driver publishes weights and alongside step 5 (SH compose) of `rendering_pipeline.md` §7.1, before the depth pre-pass and any pass sampling the atlas. Scheduling: runs when any `w > 0`, plus one copy-through dispatch on level load and one on the transition back to all-zero; skipped otherwise. Add the pass to the `POSTRETRO_GPU_TIMING` bracket list. Include a dev-tools override (single selected light, slider weight) so subtraction quality is visually inspectable as soon as Task 3's deltas exist, before Task 4 integration completes. Subtraction targets pre-compression radiance, so BC6H decode error is the accepted residual (bounded by the Task 3 tolerance AC).

### Task 6: Promoted-slot static depth cache

For a slot occupied by a promoted static light, world depth is constant while the assignment holds. Allocate a dedicated depth cache array sized to the promoted budget (`MAX_PROMOTED_SPOT` spot layers of 1024², `MAX_PROMOTED_CUBE × 6` cube-face layers of 512² — cache VRAM is O(promoted budget), never O(pool size)), with a promoted-slot ↔ cache-layer mapping. On slot assignment (and level reload): dispatch that slot's cone/face shadow-cull and render culled world geometry into the cache layer once. Per frame: copy the cached depth into the live pool slot/face (`copy_texture_to_texture`; the pool depth textures currently carry only `RENDER_ATTACHMENT | TEXTURE_BINDING` usage — add `COPY_SRC`/`COPY_DST` to pool and cache textures; this is the engine's first Depth32Float texture-to-texture copy, no precedent to reuse), then draw only entity occluders on top with `LoadOp::Load`. A warm promoted slot skips BOTH its per-frame world depth draw AND its per-frame shadow-cull sub-region dispatch (§7.1 step 6 and its cube counterpart) — pin both with counters. The cube path must preserve the occupied-faces-always-initialized invariant (`cube_face_needs_clear` semantics — the copy replaces the clear, never leaves stale depth). Invalidate on slot reassignment. Dynamic lights keep today's per-frame world render. Add counters (promoted count, cached-world-render skips, cull-dispatch skips, entity occluders submitted for promoted slots) and add the copy to the `POSTRETRO_GPU_TIMING` bracket list. Touches `renderer_shadow_passes.rs` (~879 lines) — keep additions in a new module (e.g. a depth-cache helper) rather than growing the pass file.

## Sequencing

**Phase 1 (concurrent):** Task 1 (ranking split — blocks Task 4), Task 2 (compiler selection — blocks Tasks 3 and 4).
**Phase 2 (concurrent):** Task 3 (consumes Task 2's selected set), Task 4 (consumes Task 1's split modules and Task 2's PRL section).
**Phase 3 (concurrent):** Task 5 (consumes Task 3's deltas and Task 4's weight buffer; its dev override needs only Task 3), Task 6 (consumes Task 4's promoted slots).

## Rough sketch

- Selection: new compiler module (e.g. `entity_shadow_select.rs`) consuming the `StaticBakedLights` namespace with the Task 2 narrowing; decorative test reuses `RaytracingCtx`.
- Promotion driver: `renderer_light_slots.rs::update_dynamic_light_slots` grows a promoted-static candidate stage; predicates live in `crates/lighting/src` next to `entity_occluder_eligible` (post Task 1).
- The double-count invariant, restated for this feature: a selected light reaches an entity through exactly one blend — `(1 − w) × SH-delta path + w × runtime-term path` — and reaches world surfaces through the lightmap only. The pinned exceptions: billboards (over-bright in promoted-light shadow, matching their existing dynamic-light behavior) and eviction (briefly dark, never bright).
- Entity-presence test is lights × collected instances CPU math (sphere vs instance position, entity bound radius padded) — wgpu-free, lives in `postretro_lighting` or `render-cpu`.
- Coordination: E19 `render-cpu` (in progress) is relocating CPU packers/planners this feature touches (`mesh_instances.rs`, frame uniforms, sh-compose packers) — rebase-sensitive, not design-sensitive. `runtime-cell-spatial-contract` (in progress) renames cell-locate APIs the reachability gate uses. `single-source-animated-light-brightness` (draft) touches the same slot-eligibility seam in `update_dynamic_light_slots`.

## Boundary inventory

| Name | Rust | Wire / serde | FGD KVP |
|---|---|---|---|
| Intensity-ratio threshold | `entity_shadow_min_intensity_ratio` (worldspawn parse) | n/a (compile-time only) | `entity_shadow_min_intensity_ratio` (worldspawn) |
| Falloff floor | `entity_shadow_min_range` (worldspawn parse) | n/a (compile-time only) | `entity_shadow_min_range` (worldspawn) |
| Selected-light section | `EntityShadowLightsSection` | PRL SectionId 40 | n/a |
| Per-light direct deltas | `DirectShDeltaVolumesSection` | PRL SectionId 41 | n/a |

No script/Luau/JS surface. No per-light KVP — selection is compiler-owned.

## Wire format

Both sections little-endian, matching existing PRL conventions. SectionIds pre-assigned (registry's highest is 39): **40 = EntityShadowLights, 41 = DirectShDeltaVolumes** (42 is reserved by the dependent shadowmask plan); registry updated in the same commit that adds each section.

- **EntityShadowLights (40)**: `u32` count, then `count × u32` light indices (ascending, indices into the level light array). Nonempty selections are emitted and loaded only with usable `DirectShDeltaVolumes` that structurally match the direct SH volume and cover every selection index. Empty selection, missing direct SH, missing/malformed deltas, or partial delta coverage = both sections omitted or cleared.
- **DirectShDeltaVolumes (41)**: mirrors `DeltaShVolumes` (section id 27, `crates/level-format/src/delta_sh_volumes.rs`) field-for-field — same header/grid geometry as the direct SH volume it deltas against, `affinity_offsets` (`u32`, len cell_count+1), `affinity_lights` (`u32`, flat, grouped by cell), dense 64-probe f16 RGBA octahedral sub-blocks index-parallel to `affinity_lights` — except: `affinity_lights` entries are SELECTION indices (0-based positions in EntityShadowLights order), and there is no animation-descriptor mapping (promotion weight is runtime state, not baked). Empty selection = section omitted.

## Open questions

- Threshold defaults are first-guess pins — the owner's intensity band is 50–66% and the default sits at its permissive floor (0.5); expect tuning against `campaign-test.prl` during Task 2, possibly toward 0.55–0.6. The worldspawn KVPs exist so maps can adjust without engine changes. Decorative-test constants stay code constants unless tuning proves they need authoring exposure.
- No mapper force-include/exclude override per project owner's direction. If the heuristic misfires on real maps, revisit as a follow-up (likely a worldspawn list, not a per-light KVP).
- `MAX_PROMOTED_SPOT = 8` / `MAX_PROMOTED_CUBE = 2` are first-guess budgets (cache VRAM ≈ 32 MiB + 12 MiB); tune on target hardware. If promoted statics starve dynamic point lights within the 6 cube slots despite the cap and eviction margin, a tier reservation policy is the follow-up knob.
- Per-light L2 SH subtraction can in principle ring or hue-shift under the zero clamp; the Task 5 dev-tools slider exists to catch this early. If it shows, the remedy is clamping in SH-coefficient space during the bake rather than post-projection.
