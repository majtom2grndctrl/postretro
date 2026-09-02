# dynamic-shadow-world-depth-cache

Brief · reads: `context/lib/rendering_pipeline.md` §4 · read at 64cb33a

## Problem

A performance analysis (developer) found that the dynamic-tier shadow path is the
largest per-frame item in the renderer's cost model: whole passes, not taps. Every
occupied dynamic-tier spot/cube slot re-rasterizes static world depth each frame — a
cull dispatch then a world depth draw, six faces for a point light — even though the
depth is bit-identical frame to frame. It is invariant because the shadow projection is
built from `shadow_candidate_lights`, frozen at level install; the light bridge's
per-frame poses never reach the projection builders. The re-raster is redundant because
a dynamic slot keeps world and entity depth together in one per-frame-cleared pool view
(`SpotShadowPool`/`CubeShadowPool`) with no persistent world store to reuse. campaign-test
occupies ~18–19 such fixture slots. When this is done, an occupied dynamic-tier slot whose
occupant is unchanged since last frame skips both its world-depth draw and its cull
dispatch, reusing a persistent cached world depth; the world draw runs only on the frame
the slot's occupant actually changes, while entity occluders still render every frame.

## Decisions

- **Warm-skip via a capped pool of persistent world-depth layers.** A new renderer module
  mirrors the warm/cold state machine of `promoted_depth_cache.rs` (`LayerState`,
  `plan_frame`, `assign_layer`, `mark_*_world_rendered`, with `retain_active_layers` running
  before `assign_layer`): a warming slot renders world depth once, then skips the world draw
  and the cone/face cull dispatch until invalidated. Parallel to the promoted cache, not a
  reuse — distinct textures, distinct layer namespace. Cache state clears on level reload and
  frees on an empty candidate set (mirror the install handling in `renderer_resources.rs`);
  textures recreate on device loss. (Empty-candidate free and device-loss recreate are mirror
  lifecycle, covered by the reload family, not separately witnessed.)
- **The cached world depth is FULL and consumed by BOTH receiver classes.** The promoted
  cache stores world depth but is sampled only by entity receivers (`skinned_mesh.wgsl`/
  `kinematic_brush.wgsl`, two-map `min`-combine) because a promoted-static light's
  world-on-world shadowing is baked into the lightmap; a dynamic-tier light bakes nothing
  (`rendering_pipeline.md` §4), so the dynamic cache must *additionally* be sampled by the
  forward *world* loop (`forward.wgsl`, today single-map `sample_spot_shadow`/
  `sample_point_shadow`). Both the world loop and the entity dynamic loops move onto a two-map
  cache+pool sample — reusing the `min`-combine *pattern* of `shadow_sample_static_cache.wgsl`,
  not the file verbatim: WGSL cannot pass textures as parameters, so the dynamic path needs its
  own texture binding and sampler variant, and the entity loops branch on the existing
  `i >= dynamic_light_count` split (promoted lights keep the promoted cache; dynamic lights get
  the dynamic cache). The pool slot then holds per-frame entity depth only. The per-slot
  `cache_layer` is a full per-frame sweep — every occupied slot holding a layer points at it,
  every other slot (unoccupied, or occupied but unclaimed) is -1, and -1 degrades the two-map
  sample to pool-only, so no stale channel survives a slot move and an unclaimed occupied slot
  keeps correct world+entity shadow. Both shadow directions must be witnessed: dropping the
  entity loops would silently lose world-on-entity, and a world loop that reads only the cache
  would lose entity-on-world.
- **Invalidate on occupant identity; store the projection matrix as a defensive guard.** The
  live key is the occupant's stable identity (`shadow_candidate_source_indices[candidate_index]`);
  a re-tenanted slot re-renders. Additionally store and compare the layer's light-space
  view-projection matrix (`light_space_matrix`/`cube_face_matrices` — the full `proj * view`,
  which embeds pose and range, plus cone angle for spot; not the far-plane scalar) so the
  cache stays correct the day shadow projections become live (a mover-shadow fix, or
  `drafts/animated-light-shadow-promotion`). Under today's freeze the matrix never varies for
  a fixed occupant, so this reduces to identity in practice; the matrix-change ACs are
  exercised synthetically. The key is identity + matrix only — deliberately NOT the promoted
  `CacheKey`, which includes `slot`: a dynamic light that changes pool slot under re-ranking
  keeps its layer, so the cache follows the light, not the slot.
- **Dynamic records stay in the dynamic prefix; never routed through
  `PromotedStaticLightRecord`.** `entity_shadow_select` already hard-excludes dynamic lights,
  and the promoted world-receiver path subtracts against a baked static term a dynamic light
  lacks; a light in both the dynamic prefix and the promoted tail is the double-count the
  engine invariant forbids (`context/lib/index.md` §2). The cache attaches to the existing
  dynamic slot; it does not promote.
- **Separate, capped spot and cube budgets; layers claimed demand-driven by
  warm-eligibility.** Distinct caps for the spot and cube caches (cube charged six face
  layers per slot), each well below its pool's size (`SHADOW_POOL_SIZE` is the spot pool
  only). Layers are claimed in ascending candidate-index order into the lowest free layer
  (mirroring `assign_layer`), with no eviction — a deterministic, frame-stable winner set; a
  slot holding a layer keeps it, a departed slot's layer is freed before new claims. Cube
  claims are atomic six-face units, never a partial 3-of-6. A warm-eligible slot that fails to
  claim falls back to the full world+entity pool render with `cache_layer = -1` — the
  unchanged single-map path — so it never loses world shadow.
- **Non-goal — the frozen mover-shadow gap.** Mover-attached dynamic lights' shadow
  projections are frozen at install today (their forward lighting follows the mover, their
  shadow does not). This brief keys off that frozen pose and is visually perf-neutral; it
  does not make mover shadows track. Warrant: a reader of the Problem (which names the
  freeze) might expect it. Live shadow poses are owned elsewhere
  (`drafts/animated-light-shadow-promotion` and a future mover-shadow fix).
- **Non-goal — runtime-spawned lights.** Projectile, plasma, and impact-flash lights are
  spawned `casts_entity_shadows: false` and never enter the frozen shadow-candidate set, so
  they cast no pool shadow and stress the forward light buffer, not this pool. Warrant:
  multi-player plasma churn is a natural worry about "the shadow pool"; it does not touch it.
- **Non-goal — right-sizing the pool's eager allocation.** The spot pool eagerly allocates 96
  slots (~384 MiB) against a ~19-slot ceiling; that waste is a separate, larger door
  (`context/research/baked-promoted-depth.md`), not this brief.

## Acceptance

Ordering rows below reference the constructible sequences in `research.md` § Ordering pins.

### Automated
- [ ] Spot cold frame: world depth and its cull dispatch render and fill the layer; the same
      frame's forward world sample reads the just-filled layer (world-on-world shadow present
      on the fill frame). [pin 1]
- [ ] Spot warm frame: occupant unchanged → world draw and cull dispatch skipped, skip
      counted (mirrors `cached_world_render_skips`); the layer is not rewritten, so the reused
      depth equals the cold frame's by construction. Pixel parity is the manual row. [pin 2]
- [ ] Cube: all six faces render on the cold frame, warm set only after the sixth; next frame
      skips all six, six per-face skips counted. [pin 11]
- [ ] Re-tenant: a slot taken by a different occupant re-renders that frame and never reads
      the prior occupant's depth, even when the freed layer index is reused. [pin 4/7]
- [ ] Identity follows the light: an occupant that keeps its identity but moves to a different
      pool slot stays warm; its cache follows the light, not the slot. [pin 6]
- [ ] Retained layer, vacated slot: a light keeps its layer across a pool-slot move while its
      old slot is re-tenanted or emptied the same frame — the new/old slot channels are swept
      so nothing samples the retained layer through the old slot. [pin 13]
- [ ] Projection change (synthetic; a no-op under today's frozen poses): a layer whose stored
      matrix differs re-renders; a slot fed a differing matrix every frame never warms.
- [ ] Spot budget exhausted: with more warm-eligible slots than the cap, exactly the cap claim
      layers (deterministic winners, stable across consecutive stable frames — no thrash); each
      unclaimed occupied slot renders world+entity into its pool with `cache_layer = -1`, and
      `cache_layer = -1` degrades the two-map sample to pool-only — the pre-change path by
      construction; no error. [pin 3]
- [ ] Cube budget exhausted: an unclaimed cube slot claims no partial face set and renders all
      six faces into its pool with `cache_layer = -1`. [pin 14]
- [ ] Budget covers occupancy: a constant assertion (in the style of
      `cache_budget_matches_promoted_budget_not_pool_size`) pins the spot and cube caps against
      the documented campaign-test occupancy (~18–19); the real coverage guard is the manual
      timing row below. No live occupancy probe exists, so the documented figure is the source.
- [ ] Departed/unoccupied slot: its forward-world `cache_layer` channel resets to -1; nothing
      samples the departed light's cached depth. [pin 5]
- [ ] Namespace isolation (wiring gate — a shader-source/BGL check, like the module's existing
      `include_str!` self-checks): a dynamic slot's world sample references the dynamic cache
      texture binding, never a promoted-cache layer. [pin 12]
- [ ] Level reload: cache state clears on install; a new-level occupant recycling a prior
      source index with an identical frozen matrix renders cold on frame 1 — no cross-level
      depth bleed. [pin 8]
- [ ] Empty geometry: a claimed layer clears to the far plane, marks warm, draws nothing; the
      forward min-combine reads unshadowed, with no uninitialized-depth sample. [pin 10]
- [ ] World-on-entity (wiring: the entity dynamic branch calls the two-map sampler with the
      slot's dynamic `cache_layer`): for a warm slot, a mover/skinned mesh under the fixed
      dynamic light samples static world depth from the cache. Pixel proof is the manual row. [pin 9]
- [ ] Entity-on-world (wiring: the forward world loop min-combines cache and pool; the entity
      draw into the pool is ungated by warm): entity occluders render every frame and the world
      loop's pool half remains. Pixel proof is the manual row. [pin 15]
- [ ] Dynamic-tier lights never appear in the promoted tail (review/unit gate for the
      no-double-count decision — a dynamic candidate never yields a `PromotedStaticLightRecord`).

### Manual-visual
- [ ] Under a fixed dynamic-tier fixture in campaign-test, world-on-world, world-on-entity, and
      entity-on-world shadows are identical to the pre-change single-map baseline render at full
      cache resolution, including across a frame where an entity crosses the cone — no
      over-brightening (no double-count), no lost world self-shadow. Baseline captured via the
      `--capture` scene path before the change lands (human A/B; no automated world-scene
      golden). Any resolution reduction from the open question is judged "no visible
      degradation," not bit-identical.
- [ ] `POSTRETRO_GPU_TIMING` shows dynamic shadow world-depth cost drop for every occupied
      fixture slot after its warm-up frame.

## Path

Non-binding.
- Seams: `promoted_depth_cache.rs` (warm/cold pattern); the dynamic branches in
  `record_spot_shadow_depth`/`record_cube_shadow_depth`; `update_dynamic_light_slots`/
  `update_cube_light_slots` (the frozen matrix source and stable identity); `forward.wgsl`
  dynamic world loop plus the `skinned_mesh.wgsl`/`kinematic_brush.wgsl` dynamic loops, and
  `shadow_sample_static_cache.wgsl` (`min`-combine, today entity-only);
  `drafts/animated-light-shadow-promotion` Decision 5 (the depth-cache-vs-per-frame split for
  static promoted lights — the mirror case).
- Deliberate divergence from the mirror: the dynamic `CacheKey` drops `slot` (identity +
  matrix only), so `slot_reassignment_invalidates_cache_layer` is a *counter*-example here —
  the dynamic path needs a `slot_reassignment_retains_cache_layer` test (pin 6). Otherwise
  mirror `plan_frame` ordering exactly: `retain_active_layers` before `assign_layer`; per-slot
  (not per-face) cube plan read; warm marked only after the sixth face.
- `cache_layer` carrier (delegated design): a new per-slot channel swept every frame in the
  slot-update functions (-1 for unoccupied/unclaimed). It lives in its own per-slot uniform
  (mirroring `light_space_matrices`), NOT inside `MeshLightParams`/`KinematicLightParams` —
  those are fixed-size std140 structs with layout-guard tests. Bind the dynamic cache texture
  in its OWN bind group, not by extending group 5: group 5's BGL is shared by the forward pass,
  the fog raymarch, and (on no-cube-array adapters) the `strip_point_shadow_cube` variant, so an
  added entry there ripples layout-identical declarations into all three. This is the riskiest
  new surface: the promoted path carries its layer only in the promoted metadata tail, which
  dynamic-prefix records lack.
- New module's `plan_frame` input is synthesized (not a `PromotedStaticLightRecord` list): the
  occupant set comes from `spot_shadow_pool.slot_assignment` + `shadow_candidate_source_indices`
  + the per-candidate matrices already built in the slot-update functions. "Mirror the promoted
  machinery" is about the warm/cold state machine, not the driving data path, which is new.
- Shape: persistent full-res per-slot world layer + per-frame entity depth in the pool slot,
  world and entity loops on the two-map sample, keyed on identity. Rival A: warm-skip the
  whole slot without splitting — rejected, wins nothing when an in-cone entity occluder moves.
  Rival B: bake dynamic-tier world depth offline (`baked-promoted-depth.md`) — rejected, map-
  bounded storage against the tiny-footprint northstar, and stale if the freeze is ever lifted.
- First slice: one spot slot, one persistent world layer, forward dynamic world loop switched
  to the two-map cache+pool sample with the cache bound. Prove world-on-world, world-on-entity,
  and entity-on-world match the single-map baseline on a static fixture with a moving entity —
  this falsifies the riskiest assumption, that the forward *world* path can sample a separate
  cache correctly (new; the two-map path today serves only entity receivers). Cube, counters,
  budget, and invalidation follow.
- `renderer_shadow_passes.rs` is ~1200 lines; if the dynamic-branch edits grow it further,
  split behavior-preserving in its own commit first.
- At promotion, `rendering_pipeline.md` §4 (and steps 7–8) need the durable update: they
  describe dynamic slots as rendering world depth every frame and cache-reuse as
  promoted-only; §4's "cached world depth … count as one source" is written for the promoted
  slot and must extend to the dynamic slot's world-receiver sampling.

## Open questions
- Cache budget sizes — **delegated**: separate spot and cube caps, each sized from
  campaign-test's measured stable occupancy so the coverage AC holds; report the chosen
  constants in the plan of record.
- Cached world-map resolution — **delegated**: full pool resolution gives baseline parity;
  whether a lower dynamic-cache resolution reads acceptably (no visible degradation) for world
  receivers is measured and reported.
