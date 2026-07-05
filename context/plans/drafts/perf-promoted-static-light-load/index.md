# Promoted / Selected-Static Light Load — Findings

> **Status:** findings note (not a spec). Answers: does the entity-shadow merge make **static**
> lights a per-frame growth risk on big maps, in the same class as the dynamic forward loop
> (`perf-forward-light-cull`)? Short answer: **the per-fragment shading load is bounded (≤ 10); the
> only growth is CPU-side selection/promotion scan + load-time influence VRAM, both driven by an
> uncapped baked selection, both cheap in absolute terms.** A latent risk worth a documented cap,
> not a crawl.
> **Related:** `context/lib/rendering_pipeline.md` §4 ("Promoted static lights"), §7.1 steps 7–8 ·
> `context/lib/build_pipeline.md` §PRL section IDs (EntityShadowLights id 40) ·
> `context/plans/drafts/perf-forward-light-cull/` (the dynamic-tier sibling; the per-fragment tail
> analyzed here is why the promoted records are *out of scope* there) ·
> `context/plans/drafts/perf-shadow-caster-culling/` (where a selection cap, if ever wanted, belongs).

## Three load classes, three verdicts

A baked static light that illuminates a dynamic entity enters two runtime paths post-merge. Sizing
each against `selected_static_count` = `EntityShadowLights` (id 40) length = `N`:

### 1. Per-fragment forward / mesh / billboard tail — **BOUNDED at ≤ 10. Not a growth risk.**

`total_light_count = full.light_count + full.promoted_static_records.len()`
(`crates/renderer/src/render/renderer_light_slots.rs:851`). The forward
`shadowmask_union_subtraction` loops `promoted_count = total_light_count − light_count`
(`crates/renderer/src/shaders/forward.wgsl:741`); billboard (`billboard.wgsl:328`) and mesh
(`skinned_mesh.wgsl:443`, fed `total_light_count` at `renderer_render_frame.rs:384`) add the same
tail to their loops. So the per-fragment cost of this class is exactly `promoted_static_records.len()`.

**Arithmetic for the bound.** A `PromotedStaticLightRecord` is pushed only when a light is
`assigned` a shadow-pool slot (spot or cube slot ≠ `NO_SHADOW_SLOT`) **and** `weight > 0`
(`renderer_light_slots.rs:797`, `:813–824`). Slots are handed out by
`assign_shadow_pool_slots_with_promoted_static` under a `promoted_cap`: `MAX_PROMOTED_SPOT = 8` for
the spot pool (`renderer_light_slots.rs:137`) and `MAX_PROMOTED_CUBE = 2` for the cube pool (`:682`),
both from `renderer_types.rs:371–372`. A record cannot exist without a slot, so:

```
promoted_static_records.len() ≤ MAX_PROMOTED_SPOT + MAX_PROMOTED_CUBE = 8 + 2 = 10
```

independent of `N` and of the map-wide static-light count. **This is the crux.** The exact concern
that motivates `perf-forward-light-cull` — a per-fragment loop that scales with the *map-wide* light
count — does **not** apply to the promoted-static tail: the shadow-pool budget already caps it at 10,
several rooms' worth. No per-fragment growth to cull.

### 2. Per-frame CPU selection / promotion scan — **O(N) (and O(N × candidates), O(entities × N)). Grows with the baked selection, which is uncapped. Cheap in absolute terms today.**

- **Promotion driver.** `update_promoted_static_weights_and_records` loops all `N`
  `promoted_static_states` (`renderer_light_slots.rs:772`); for each it runs a `.position()` linear
  scan of `shadow_candidate_selection_indices` (length `dynamic + N`) — so
  O(N × (dynamic + N)) integer compares per frame.
- **Slot ranking.** `assign_shadow_pool_slots_with_promoted_static` iterates the full candidate list
  (`dynamic + N`) per pool (`renderer_light_slots.rs:896`), plus the hysteresis sort.
- **Mesh collector.** `selected_static_shadow_light_reaches_bounds`
  (`crates/postretro/src/scripting/systems/mesh_render.rs:351–364`) is called **per entity** each
  frame (`:270`) and iterates `world.entity_shadow_lights` (= `N`) doing a sphere-vs-AABB test each
  — O(entities × N) per frame.

**Is `N` bounded?** No. `select_entity_shadow_lights`
(`crates/level-compiler/src/entity_shadow_select.rs:32–72`) returns **every** eligible baked-tier
static light — a quality filter (min-intensity ratio `:43–50`, min range, non-decorative-spot
`:74–97`) with **no top-K cap and no count budget**; `light_indices` is the full eligible set,
sorted. The compiler even logs the raw count (`main.rs:911–914`) precisely because it is
open-ended. So an author who sprinkles many bright, wide-range point/spot fixtures (cheap to place
because baked) grows `N`, and these scans grow with it.

**But the absolute cost is small.** At a generous big-map estimate (`N = 100`, entities = 50,
candidates = 250): mesh collector ≈ 5 000 sphere-AABB tests/frame; promotion ≈ 100 × 250 = 25 000
integer `.position()` compares/frame; ranking ≈ a few hundred-element sorts. These are CPU
microseconds, not a crawl — and note the promotion driver already gates each candidate on the wide
reachable set (`visible_lights`, `renderer_light_slots.rs:87–127`) so off-view selections don't
promote. This is a **latent O(N) growth path**, not a measured bottleneck. Flag it; don't fix it
speculatively.

### 3. Load-time influence-buffer VRAM — **O(N). Memory, not a loop. Trivial.**

`influence_capacity_with_shadowmask_metadata(dynamic, N) = dynamic + N + N × 2`
(`crates/renderer/src/render/shadowmask.rs:17–25`) sizes the influence buffer once at level install
(`renderer_resources.rs:235–243`). Each record is 16 bytes, so `N = 100` adds `3 × 100 × 16` = 4.8
KiB — negligible. The per-frame *packed* metadata is bounded by `records.len()` (≤ 10,
`pack_forward_shadowmask_metadata`, `shadowmask.rs:56–95`), not `N`; only the buffer *capacity* is
O(N).

### Shadow-slot side — **BOUNDED (confirms the prior finding).**

Promoted-static shadow slots are capped at `MAX_PROMOTED_SPOT = 8` + `MAX_PROMOTED_CUBE = 2`
(`renderer_types.rs:371–372`) and their world depth is cached
(`crates/renderer/src/render/promoted_depth_cache.rs`), so the shadow path adds no per-frame
re-raster growth. See `perf-shadow-caster-culling` for the dynamic-slot cost that plan targets.

## Verdict: does the fix fold into `perf-forward-light-cull`? No.

The fold-in criterion was: **only if the same drawn-cell visible-set contribution cull applies.** It
does not, for two independent reasons:

1. **Nothing to cull in the forward class.** The per-fragment promoted tail is already bounded at
   ≤ 10 by the shadow-pool budget (class 1). `perf-forward-light-cull`'s mechanism removes
   zero-contribution iterations from a loop that scales with map-wide count; this loop doesn't scale
   with map-wide count, so the mechanism has no target here. The one-line out-of-scope note in that
   spec ("promoted tail already bounded by promotion budgets ≤ 8 + 2") is confirmed correct by the
   arithmetic above.
2. **The real growth is not a per-fragment contribution loop.** Class 2 is a CPU selection/promotion
   scan; class 3 is a load-time allocation. Neither is cullable by "which lights reach a drawn
   cell" — the promotion driver *already* applies a reachability gate, and the scan cost is in
   iterating the selection set itself, not in shading it. The right lever is a **cap on `N`** (or a
   spatial index for the scans), which is a selection/shadow-pool concern, not a forward-contribution
   cull. Forcing it into `perf-forward-light-cull` would conflate two different cost models.

So: **keep `perf-forward-light-cull` dynamic-tier-only.** This note documents that the promoted-static
forward load was assessed and found bounded.

## Recommendation (if action is ever warranted)

The only unbounded quantity is the baked selection `N`. If a big-map profile ever shows class 2 in a
CPU trace, the fix is a **compiler-side selection cap** — rank eligible lights by the same score the
runtime uses (`postretro_lighting::shadow_ranking::slot_score`) and keep the top-K, where K is a
small multiple of the runtime promoted budget (10) so the runtime always has enough candidates to
fill its slots but the per-frame scans stay bounded. This lands in
`crates/level-compiler/src/entity_shadow_select.rs:52–71` (add the sort-and-truncate after the
eligibility filter) and is a natural companion to the promoted-cap discussion in
`perf-shadow-caster-culling` (both are about the ≤ 10 promoted budget and what feeds it).

**It is not urgent.** At present scales the scans are microseconds and the VRAM is kilobytes. This
note's purpose is to record that the growth exists, is CPU/VRAM (not per-fragment), and has a clean
one-file fix if a map ever provokes it.

The top-K cap is the point fix. The *consolidated* fix is a shared static cell→light index
(`context/plans/drafts/cell-light-binning/`), which would turn these class-2 scans into cheap
per-cell gathers — but that is a streaming-era build gated on cell clustering, so prefer the top-K
cap if this is ever provoked before streaming lands.

## Cross-reference placement

If the maintainer wants a pointer from the shadow spec, add one bullet to
`context/plans/drafts/perf-shadow-caster-culling/index.md` under **`## Related work`** (after the
`perf-anti-penumbra-pvs` bullet, ~line 358):

> - **`context/plans/drafts/perf-promoted-static-light-load/`** — findings note: the promoted-static
>   *forward-entity* per-fragment load is bounded at ≤ 10 by the same `MAX_PROMOTED_SPOT + CUBE`
>   budget this plan caps. The only growth is the CPU selection/promotion scan over the uncapped
>   baked `EntityShadowLights` set; a top-K selection cap in `entity_shadow_select.rs` is the
>   companion lever if ever needed.

No change to engine code, tests, or the shadow spec was made.
