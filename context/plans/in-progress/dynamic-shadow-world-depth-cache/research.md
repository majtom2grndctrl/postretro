# dynamic-shadow-world-depth-cache — research

Source read at 64cb33a. Findings that inform the brief but do not decide it.

## The cause, confirmed and reframed

The dynamic branch in `record_spot_shadow_depth`/`record_cube_shadow_depth`
(`crates/renderer/src/render/renderer_shadow_passes.rs`) draws world depth unconditionally
under `draw_world` for every occupied slot without a promoted plan, after its per-slot cone/
face cull dispatch — six faces per point light.

Reframe from the original suggestion: the redundancy is **universal across the dynamic
shadow pool**, not limited to authored "fixtures." The shadow projection matrices
(`slot_cone_matrices`, `face_matrices`) are built from `full.shadow_candidate_lights`, a
`Vec<MapLight>` written at exactly two install-time sites (`renderer_resources.rs`,
`renderer_full_init.rs`) and never again. `update_dynamic_light_slots`/`update_cube_light_slots`
read that frozen candidate and reproduce a bit-identical matrix every frame. The light
bridge's per-frame poses/ranges (`light_bridge.rs`: `follow_transform_position` →
`cached_follow_positions`, `eval_animated_radius` → `falloff_range`) flow only into the
forward `lights_buffer` and into slot eligibility/ranking — never into the projection
builders. In-code comment (renderer_light_slots): "a scripted sweep light's animated
position/cone is NOT reflected here."

## Two premises from the original suggestion that did not survive source

- **"Mover-attached lights would still re-render correctly."** They do not re-render moving
  shadows today — their projection is frozen at install pose (see above). kinematic-platform's
  three dynamic lights are all `carrier`-attached; their forward lighting follows the platform,
  their pool shadow does not. This is a latent gap, split out (brief Decision, non-goal).
- **"Impact flashes would still re-render."** Runtime-spawned lights are spawned
  `casts_entity_shadows: false` (`component_to_map_light`, light_bridge.rs) and are absent from
  the frozen candidate set, so they cast no pool shadow at all. Moot.

The correctness caveats the suggestion raised therefore mostly dissolve; the real invalidation
trigger is **slot re-tenanting** under hysteresis re-rank (`assign_slots_with_hysteresis`,
`EVICTION_MARGIN`), which the promoted cache already handles by keying its `CacheKey` on `slot`.

## Why this is not an extension of the promoted-depth cache

`plans/done/static-light-entity-shadows` Task 6 introduced the cache (world depth once per
assignment, entities per frame). `plans/done/promoted-shadow-entity-only-depth` made it
directly-sampled and **entity-only**: "a promoted static light's pool slot holds
dynamic-occluder depth only; the static world is never rendered into it… movers and skinned
meshes keep crisp static-world shadows by sampling the promoted depth cache directly."

That entity-only decision is causally tied to promoted lights being *static and baked*:
world-on-world shadowing lives in the lightmap (shadowmask union subtraction on world
receivers, `forward.wgsl`), so world receivers never sample the cache. A dynamic-tier light
has no baked visibility anywhere (`rendering_pipeline.md` §4). So its cached world depth must
be full and sampled by world receivers — a receiver path no committed spec contemplates.
Extending the promoted cache to dynamic lights was never named as deferral/non-goal/future/
rejected; the selector (`entity_shadow_select::is_promotable_base_light`) structurally forbids
dynamic lights from promotion. New territory.

Double-count hazard: the count-split light buffer keeps dynamic-tier records first, promoted
records appended; a light in both halves double-counts (`context/lib/index.md` §2). Hence the
brief's decision to keep the dynamic record where it is and attach the cache to its dynamic
slot, never to promote it.

## Forward sampling — the new work

Two-map `sample_*_with_static` (`shadow_sample_static_cache.wgsl`, `min(pool, static_world)`
per PCF tap) is used today only by entity receivers (`skinned_mesh.wgsl`, `kinematic_brush.wgsl`).
World receivers use single-map helpers (`shadow_sample.wgsl`); for promoted lights they use the
baked shadowmask, never the cache. So warm-skipping dynamic world depth requires the forward
dynamic *world* loop (`forward.wgsl`, the point/spot dynamic taps) to switch to the two-map
sample and receive a `cache_layer`, and the dynamic world-depth cache to be bound into the
forward world path. The promoted sampled views are fixed-size D2Array(8)/CubeArray(2) bindings;
the dynamic cache is a separate, separately-sized allocation.

## Content sizing

- campaign-test (`content/dev/maps/campaign-test.map`): 18 dynamic-tier lights, all fixed-pose
  fixtures (0 mover-attached, 0 impact). 17 are intensity-animated only (brightness/color; no
  position/direction/cone) via `content/dev/scripts/arena-lights.ts` — pose never moves. All 18
  have world depth that never changes → the full win case.
- kinematic-platform (`content/dev/maps/kinematic-platform.map`): 3 dynamic-tier lights, all
  `carrier`-attached movers → the correctness case (they legitimately would re-render, if their
  shadow poses tracked — which today they do not; see above).

## The frozen mover-shadow gap (Decision 6 refers here)

Independent of this perf work: a mover-attached or animated-range dynamic light's shadow-map
projection is frozen at install pose/range while its forward lighting tracks the live pose. The
cube far-plane can desync from the live GPU range for an animated-range light that wins a cube
slot (`cube_shadow.rs` comment warns of this; `eval_animated_radius` now animates range into the
forward buffer, so the comment's "no path mutates range" premise is stale). Making shadow
projections track live poses is owned by `drafts/animated-light-shadow-promotion` (for animated
baked lights) and a future mover-shadow fix. This brief keys off the frozen pose and is
visually perf-neutral.

## Matrix key is inert under the freeze (why identity is the live key)

The projection matrix is built from the frozen static candidate (`light_space_matrix` in
`crates/lighting/src/lib.rs`, `cube_face_matrices` in `cube_shadow.rs`; debug note in
`renderer_light_slots.rs` confirms animated position/cone is not reflected). So for a fixed
occupant the matrix is bit-identical every frame regardless of runtime movement, and
matrix-change invalidation collapses into identity-change invalidation today. The matrix stays
in the key as a defensive, forward-compatible guard: the day a pose-to-projection path lands
(a mover-shadow fix, or `animated-light-shadow-promotion`'s direction-animated cone), a moving
projection invalidates automatically with no rework. The far plane carries only range
(`far = falloff_range`); pose lives in the `view` factor and cone angle in the `fov` — the full
`proj * view` product covers all three, so the compared key is the whole matrix, never a
far-plane scalar. The matrix-change ACs are therefore exercised by feeding differing matrices
synthetically; the real light path cannot trigger them while projections are frozen.

## World-on-entity would regress silently without the entity-loop switch

Today a dynamic slot's pool view holds world+entity depth merged, and entity receivers
(`skinned_mesh.wgsl`, `kinematic_brush.wgsl`) sample that one map — so a mover under a dynamic
light is shadowed by static world geometry. Warm-skip moves world depth into the cache and
leaves the pool slot entity-only. If only the forward *world* loop is switched to the two-map
sample and the entity loops are left on the single pool sample, entity receivers lose
world-on-entity occlusion while all world-path automated ACs still pass — the regression shows
only in the manual-visual check. Hence both loops must move onto the two-map sample and receive
the dynamic `cache_layer`, and the brief carries an explicit world-on-entity AC (pin 9).

## Ordering pins (temporal review)

The constructible sequences the brief's Acceptance rows reference. Each is writable as a test.
Rows 3, 5, 8, 12 correspond to the four ordering blockers most likely to be built wrong from
the Decisions alone.

| # | Scenario | Ordering (constructible sequence) | Expected outcome |
|---|----------|-----------------------------------|------------------|
| 1 | Cold→warm→sample, same frame | Frame N: plan assigns layer L to slot S (warm=false) → world drawn into dynamic cache layer L → mark_world_rendered(S); later same frame the forward world pass samples S | Forward world sample this frame reads the just-filled layer L (intra-encoder ordering); world-on-world shadow present on the fill frame. |
| 2 | Steady warm | Frame N fills S (as #1). Frame N+1: occupant unchanged | Plan returns needs_world_render=false; world draw AND cull dispatch skipped for S; skip counted; forward world sample still reads L; shadow identical to N. |
| 3 | Budget exhausted | Budget B; B+1 slots warm-eligible and stable; claim list built in stable order | Exactly B slots hold layers (deterministic winners); remainder render world into their pool slot with cache_layer=-1; no error; same B winners persist next stable frame (no thrash). |
| 4 | Slot re-tenanted, layer index reused | Frame N: light X (src=1) → pool slot 3, layer 0, warm. Frame N+1: X ineligible, light Y (src=2) → pool slot 3 | retain runs first, clears layer 0 (key {1,·} not active); assign gives Y layer 0 warm=false → world rendered for Y that frame; Y never reads X's depth. |
| 5 | Departed light, stale forward channel | Frame N: slot 3 warm, forward cache_layer channel = 0. Frame N+1: slot 3 occupant leaves; slot 3 unoccupied | Slot 3's forward-world cache_layer channel resets to -1; no forward sample resolves to layer 0; nothing samples the departed light's cached depth. |
| 6 | Same identity, new pool slot | Frame N: X (src=1) in pool slot 3, layer 0, warm. Frame N+1: ranking moves X to pool slot 5, identity+matrix unchanged | Key {1,M} unchanged → layer 0 retained, warm stays true → world skipped; forward world sample for X uses layer 0 (cache follows the light, not the slot). |
| 7 | Occupant change, matrix identical | Frame N: src=1 warm on layer 0. Frame N+1: same pool slot now src=2 whose frozen matrix equals M | Identity component differs → invalidate → world rendered for src=2 that frame; no reuse despite identical matrix. |
| 8 | Level reload, source-index recycled | Level A: layer 0 keyed {src=5, M} warm. Install level B; a B occupant also resolves to {src=5, M} | Dynamic cache state cleared at install → layer 0 cold → world rendered for B's occupant frame 1; no level-A depth bleed. |
| 9 | Entity moves through cone, warm slot | Slot S warm across N, N+1, N+2; an entity crosses the cone at N+1 | Every frame the pool entity pass runs (Clear(1.0) + entity draw) regardless of warm; world stays skipped; entity shadow tracks the mover; world-on-entity and world-on-world identical before/after (min-combine of cached world + per-frame entity). |
| 10 | Empty-geometry cold fill | Map with no world geometry (draw_world=false); slot S claims a layer | Cache pass runs Clear(1.0) (far plane = lit), draws nothing, marks warm; forward min-combine sees static_world=1.0 → world contributes no shadow; no sampling of uninitialized depth. |
| 11 | Cube six-face fill | Frame N: cube slot S cold; loop over its 6 contiguous face layers | All six faces render world this frame (plan read once per slot); warm marked only after face 5; frame N+1 skips all six; six per-face skips counted. |
| 12 | Namespace isolation | Promoted light on promoted layer 0 and a dynamic light on dynamic layer 0 coexist | Forward world sample for the dynamic slot binds the dynamic cache texture; never resolves to promoted-cache layer 0; the two cache_layer spaces do not alias. |
| 13 | Retained layer, vacated slot re-tenanted same frame (pin 4 × pin 6) | Frame N: X(src=1) pool slot 3, layer 0, warm; channel[3]=0. Frame N+1: ranking moves X to slot 5 (identity+matrix unchanged) AND Y(src=2) enters slot 3; Y fails to claim (budget full) or slot 3 left empty; channels rewritten by full sweep | Layer 0 retained by X; channel[5]=0 (X samples its retained depth at the new slot); channel[3]=-1 (X's old slot resets even though layer 0 is still live); Y (or the empty slot) never samples layer 0. |
| 14 | Cube budget exhausted, atomic six-face claim | Cube cap C; C+1 cube slots warm-eligible and stable; claim in candidate-index order | Exactly C cube slots hold a contiguous 6-face unit (deterministic winners); the (C+1)th claims none — no partial 3-of-6 — and renders all six faces into its pool slot with cache_layer=-1; no error; same C winners persist next stable frame. |
| 15 | Entity-on-world under a warm slot | Slot S warm across frames; a world surface lies behind a moving entity occluder in S's cone | The forward world loop min-combines the cache (world-on-world) and the pool (entity-on-world), so the world surface is shadowed by the moving entity every frame while world depth stays skipped. Guards the pool half of the two-map world sample. |

Rows 1–8, 11–12 are guarantees the Decisions imply but do not write down; rows 13–15 close
holes the round-1 rewrite introduced. Rows 3, 5, 8, 12, 13, 15 are the ones most likely to be
built wrong from the brief alone.

## `cache_layer = -1` has two meanings

Both readings must hold. For an *unoccupied or departed* slot (pin 5) it means "not sampled."
For an *occupied but unclaimed* slot (budget full, pins 3/14) it means "the pool holds
world+entity — the two-map forward loop samples the pool only," reproducing today's single-map
result exactly. A slot is never a partial split: either warm (world in cache, entity in pool,
both sampled) or full (world+entity in pool, cache_layer=-1, pool-only). The forward
`cache_layer` channel is a full per-frame sweep over all slots, so a layer retained at a
moved-away slot cannot leave a stale non-negative channel behind (pin 13).

## The dynamic key drops `slot` (deliberate divergence from the promoted mirror)

The promoted `CacheKey` is `{global_light_index, selection_index, slot}`, and its flagship test
`slot_reassignment_invalidates_cache_layer` asserts a slot move invalidates the layer. The
dynamic cache keys on identity + matrix only (no `slot`): a dynamic light re-ranked to a new
pool slot has unchanged world depth and keeps its layer (pin 6). So the promoted test is a
counter-example here, not a template; the dynamic path needs the inverse,
`slot_reassignment_retains_cache_layer`. This is the one place "mirror the promoted machinery"
must not be taken literally.
