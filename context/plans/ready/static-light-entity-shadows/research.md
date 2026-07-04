# Research notes — static-light entity shadows

Grounded 2026-07-04 against source. Line numbers are ephemeral; verify before use. Serves both this plan and `static-light-shadowmask-world-receipt`.

## Review panel (2026-07-04)

Four-reviewer panel (codebase anchor, broad design, implementability/executor lens, decision-fidelity + cross-plan coherence) ran against the first committed draft; all findings applied in the follow-up commit. Highest-value corrections, kept here because they encode non-obvious source facts:

- Fog is NOT a light-buffer count consumer: `collect_fog_spot_lights` (`renderer_lighting.rs`) walks `spot_shadow_pool.slot_assignment` with no tier test — promoted statics need an explicit not-promoted filter there, a count split does nothing for fog.
- The `DirectShVolume` contributor set filters neither `bake_only` nor directional; the selection domain is a strict subset (and `bake_only` lights have no runtime light index — `light_namespaces.rs` drops them from the on-disk set).
- Three separate light-count uniforms exist (forward `uniforms.light_count`, billboard reads the SAME shared group-0 struct, mesh `mesh_light_params.light_count`); billboard needs a new `total_light_count` field in the shared struct — a 3-way byte-contract change.
- One shared GPU light buffer feeds forward and mesh group-2 b0 (`filter_dynamic_lights` → `lights_buffer`); the count-split appends to it.
- No `copy_texture_to_texture` exists anywhere in `crates/`; pool depth textures carry only `RENDER_ATTACHMENT | TEXTURE_BINDING` — the depth cache adds `COPY_SRC`/`COPY_DST` and is the engine's first Depth32Float T2T copy.
- `apply_cosine_lobe_rgb` confirmed linear per coefficient (per-band scalar multiply applied after the cross-light sum) — the delta-sum-exactness claim holds.
- SectionId registry's highest allocated id is 39 → 40/41/42 pre-assigned across the two plans.
- Union-term hazard (shadowmask plan): in a baked soft penumbra with no entity, raw `max(0, baked_vis − hard_PCF_vis)` hardens the static penumbra — a prohibited runtime static→static shadow; now a hard prototype-gate condition with ramp-bias as the committed remedy.

## Supersedes

`baked-entity-shadow-lighting-handoff.md` (formerly in `drafts/`, deleted at promotion — recover from git history if needed) — project owner reviewed and redirected: ignore its Path A recommendation (leaning away from SDF for shadows/lights entirely), go shadowmask-flavored, but never runtime static→static shadowing. Its feasibility anchors remain useful; its recommendation does not stand.

## Owner decisions (2026-07-04 session)

- Shadowmask direction; world direct stays baked; no runtime static→static shadows.
- No opt-in KVP — compiler heuristically selects lights (dim-light, short-falloff, decorative-fixture exclusions; thresholds per spec).
- SDF machinery: do not build on it; leaning away from SDF long-term.
- SH probes are the far LOD for entity light/dark space; pool promotion is the near tier.

## Directional self-shadowing question — answered

`DirectShVolume` (PRL 35) bakes per-light occlusion-tested radiance into L2 SH (9 coeff × RGB, cosine-lobe convolved) — `crates/level-compiler/src/direct_sh_bake.rs` (`bake_probe_tile`, `soft_visibility`). The mesh shader samples it **per fragment with the fragment normal** (`sample_sh_direct`, `skinned_mesh.wgsl:374-399`, Chebyshev on, backface off); billboards per vertex with N = camera-forward (`billboard.wgsl:496-516`). So directional response on moving entities is real. What SH cannot encode: cast self-shadowing (probes know nothing of the entity's own geometry) and anything above L2 frequency. Dynamic spots self-shadow because the entity renders into a real depth map. Pool promotion is therefore the only path to parity; the SH tier is the correct far LOD.

## Shadow pool (current state)

- Pools: spot 96 slots × 1024² Depth32Float (`SHADOW_POOL_SIZE`, `crates/renderer/src/lighting/spot_shadow.rs:13`); point cube 6 slots × 6 faces × 512² (`cube_shadow.rs:36-45`).
- Ranking: `assign_ranked_slots` (`spot_shadow.rs:29-48`), score `(falloff_range / max(distance, near))²`, ties by light index; spot `rank_lights` `spot_shadow.rs:399-455`, cube `rank_point_lights` `cube_shadow.rs:138-181`. No camera-orientation term (deliberate: shadowed set invariant under pitch/yaw).
- Gates: slot eligibility = `is_dynamic` alone; entity occluders additionally need `casts_entity_shadows` — `entity_occluder_eligible` at `crates/lighting/src/lib.rs:101-103`. `is_dynamic` set by classname (`light_dynamic`/`light_dynamic_spot`, `quake_map.rs:30,311`); every authored baked light parses `false`. `_cast_entity_shadows` lives on the `DynamicLight` FGD base only, warn-cleared on baked lights.
- Visibility gate: `light_reaches_visible_cell` (`lib.rs:138-154`) — influence sphere vs fog/light-reachable cell AABBs (the WIDER portal-reachable set, deliberately not drawable `VisibleCells`; own-cell-PVS gate was removed for pitch-down dropouts). Plus brightness suppression `< 0.01`. Gates slot assignment; forward b0 upload is all `is_dynamic` lights regardless.
- `filter_dynamic_lights` and `filter_entity_shadow_candidates` (`renderer_lighting.rs:87,118`) are currently byte-identical filters.
- E10 Task 7 static depth cache was CUT — every occupied slot/face re-renders full world depth every frame (`renderer_shadow_passes.rs:220-247` spot, `:329-365` cube). Cube occupied faces must always be initialized (`cube_face_needs_clear` invariant; uninitialized depth reads as fully shadowed).
- `GpuLight` 64 B: type in vec4[0].w, spot slot vec4[3].z, cube slot vec4[3].w, sentinel `NO_SHADOW_SLOT`; color premultiplied by intensity CPU-side (`pack_light_with_slot`, `lib.rs:185-229`). No spare bits — routing by buffer membership, not flags. Weight-premultiply into color is the no-shader-change seam.

## Entity lighting (current state)

- Mesh group 2 live: `accumulate_dynamic_direct` (`skinned_mesh.wgsl:441-573`) — per-fragment, diffuse-only, spot + cube shadow attenuation via slots in the light record, scripted-animation aware. Filtering is CPU-side (buffer contents), no flag test in shader; loop bound `mesh_light_params.light_count` — independent of forward's bound (the count-split seam).
- Mesh shadow bindings group-2 b5–b8 alias the same pool textures/matrices forward binds at group 5 (`mesh_pass.rs:264-318`).
- `DirectShVolume` bake domain: `StaticBakedLights` minus animated minus sdf-typed (`direct_sh_bake.rs:94-109`; test `static_direct_filter_excludes_animated_dynamic_and_sdf`). SUMMED per probe — no runtime per-light exclusion exists (BC6H, pre-summed). Runtime knobs are whole-term only (`DynamicDirectParams`: scale/isolation/has_direct).
- Animated delta SH infra to mirror for direct deltas: `delta_sh_volumes.rs` (section 27, affinity CSR, `AFFINITY_FACTOR=4`, 64-probe f16 sub-blocks per (cell,light)); compose `sh_compose.rs` + `sh_compose.wgsl` (one thread per atlas texel, base + Σ curve-scaled deltas → total atlas). Compose currently dispatches full-atlas unconditionally when world renders.
- Instance cull: cell membership vs `VisibleCells` (`render-cpu/src/mesh_pass.rs:26-45`); shadow-caster cull is per-light cone/frustum in `plan_mesh_frame` (`mesh_instances.rs`), independent of main-view cull — the entity-presence gate should key on the caster-capable set.
- E17 kinematic movers plan to reuse the dynamic-object lighting model (SH atlases + dynamic loop) — receivers work automatically; occluder participation is E17's call.

## Forward/world path + budget

- Forward sampled textures: 14/16 with cube array, 13/16 without (`forward_pipeline_sampled_texture_request_matches_bgl_definitions`, `pipeline_budget_tests.rs:44`; single source `forward_pipeline_sampled_texture_count`). Headroom 2 → shadowmask +1 fits without lightmap array consolidation.
- `spec_lights` (group 2 b2): all baked-tier lights, 64 B (`SpecLight`), sdf flag in `color_and_pad.w`; chunk grid/offsets/indices b3–b5. Billboard binds the same set. VERTEX-stage storage pinned at 6 ≤ 8 for billboard.
- Per-light lightmap separability exists: `lightmap_layer.rs` (`LayerTexel`, per-light irradiance + weighted_dir, bit-for-bit recomposable) — the shadowmask bake's substrate. Per-light accumulate at `lightmap_bake.rs:1177-1188`, `light_texel_contribution` `:1286-1322`.
- Dynamic shadow receipt on world: multiply inside the world dynamic loop only (`forward.wgsl:961-993`), helpers in `shadow_sample.wgsl` (binding-agnostic by lexical name).
- SDF path is live (compile/load-gated by atlas presence, not runtime perf gate) but owner is leaning away from SDF — untouched by these plans.
- Only existing sticky/handoff pattern: fog hysteresis (`compute_active_mask_with_hysteresis`, `fog_pass.rs:841-866`) — the promotion-weight template. No lighting crossfade/LOD exists today.

## Plan-state coordination

- Roadmap: E10 mesh shadow casting / direct lighting / shadow receipt all `[x]` (roadmap.md:35-37). No epic homes this feature; nearest backlog line is "Moving-light shadow-depth invalidation" (~:341). M10--dynamic-mesh-shadows pins the dynamic-only entity-shadow constraint this feature deliberately inverts — soften its "physically impossible" wording in context/lib at promotion, not now.
- In-progress conflicts: `E19--render-cpu` relocates CPU packers/planners we touch (rebase hazard); `runtime-cell-spatial-contract` renames cell-locate APIs under the reachability gate; draft `single-source-animated-light-brightness` touches the same `update_dynamic_light_slots` eligibility seam.
- Stale docs noted in passing: `sdf-per-light-shadows/architecture.md` says 12-slot pool (source: 96); `map_data.rs` comments point at `in-progress/` for a plan now in `done/`.

## Oversized-file flags (split-before-extend candidates)

- `spot_shadow.rs` 1123 — extended by promotion → split task (Task 1).
- `renderer_shadow_passes.rs` 879 — depth-cache changes → new module, don't grow (pinned in Task 6).
- `mesh_pass.rs` 3309 — only bind-group swap here; E19 render-cpu is already carving it, do not split in this plan.
- `lightmap_bake.rs` 3818 — selection lives in a new module (pinned in Task 2); shadowmask bake reuses `lightmap_layer.rs` (1713).
- `forward.wgsl` 1055 — shadowmask plan adds a term; WGSL has no module split mechanism beyond helper concatenation; keep the union term in a shared helper file.
