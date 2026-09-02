# Animated-light shadow-map promotion

## Goal

Give a script-animated baked light true runtime self-shadowing on moving receivers.
The v1 feature (`animated-direct-sh-dynamic-receivers`) lit movers/meshes/billboards
under an animated light through a baked static-occlusion SH delta (section 45) — no
receiver self-shadow, and direction animation frozen at rest. This follow-on promotes
an animated baked light into the runtime spot/cube shadow pool near a moving receiver,
crossfading from the baked delta to a shadow-mapped runtime term — the exact inverse of
`static-light-entity-shadows`, which promoted *static* lights by subtracting against the
`DirectShVolume` delta (id 41); here the subtraction is against v1's *additive* animated
delta (id 45). As a bonus, the runtime term has a real cone, so promotion is also the
escalation path for **direction-animated** mover direct that v1 deferred.

## Dependency

**Blocks on `animated-direct-sh-dynamic-receivers` shipping.** This plan extends v1's
section 45, its Pass B compose, and its CPU scale seam. It cannot start until v1 is
merged. It also consumes the `static-light-entity-shadows` pool machinery (ranker,
promoted-depth-cache, weight ramp, budget constants) unchanged where possible.

## Scope

### In scope

- Runtime promotion of an animated baked light into the spot/cube shadow pool when a
  shadow-relevant moving receiver (mover or skinned mesh) intersects its influence and
  it is portal-reachable — mirroring `selected_static_light_has_shadow_entity`.
- A crossfade weight `w` per promoted animated light: receiver term is
  `(1−w) × section-45 baked delta + w × runtime shadowed term`, no double-count.
- The runtime term: inject the promoted animated light into the dynamic-direct `lights`
  buffer with a **forward** animation descriptor (runtime-evaluated curve) and a pool
  shadow slot, color premultiplied by `w` — the light currently never enters that buffer.
- Pass B (v1) gains a per-animated-light `(1−w)` promotion-weight factor so the baked
  delta fades as the runtime term rises.
- Direction-animated promoted lights: the runtime cone tracks the live direction curve
  (closes the v1 direction limitation *for promoted lights only*).
- Depth-cache split: brightness/color-only promoted animated lights reuse the static
  promoted-depth-cache (fixed position + direction → cached world depth valid);
  direction-animated promoted lights re-render world depth per frame (frustum moves).

### Out of scope

- **World-surface self-shadowing under animated lights.** World stays `lm_anim`; only
  moving receivers get the runtime term (same boundary static promotion holds).
- **Billboards as promotion receivers.** They evaluate per-vertex unshadowed, as under
  static promotion; they keep the v1 baked delta only.
- **Raising the pool budget by default.** Budget contention policy is the central open
  question; the default sketch shares the existing `MAX_PROMOTED_SPOT`/`MAX_PROMOTED_CUBE`.
- **Compile-time animated-promotion selection list.** Every animated baked light already
  has a section-45 delta, so eligibility is runtime-only — no `EntityShadowLights` analog.

## Design decisions

1. **No compile-time selection; runtime eligibility only.** Static promotion needs a
   compile-time `EntityShadowLights` list because not every static light has a
   `DirectShVolume` delta. Every animated baked light already carries a section-45 delta,
   so any of them is a candidate. Eligibility is decided at runtime by the same gates
   static promotion uses at runtime — receiver-in-influence + portal-reachable + budget —
   plus the same dim/short-range/decorative heuristic thresholds, evaluated on the light's
   **authored (peak)** intensity, not its instantaneous strobe value. The "selection
   index" for the weight buffer is simply the `AnimatedBakedLights` index.

2. **The subtraction seam is v1's additive delta, scaled down.** v1 Pass B writes
   `Σ_anim(scale_j(t) × delta_j)`. For a promoted animated light at weight `w`, Pass B
   writes `(1−w) × scale_j(t) × delta_j`; the dynamic-direct loop adds
   `w × scale_j(t) × runtime_shadowed_j`. Both evaluate the *same* curve (single-sourced
   per `single-source-animated-light-brightness`), so energy holds and no receiver sums
   the light twice. This is the mirror of static promotion's
   `(1−w)·baked_SH + w·runtime` on the additive side.

3. **The runtime term needs a forward descriptor.** A baked animated light today has only
   a compose descriptor (`pack_compose_animation_descriptor`) and never enters the runtime
   `lights` buffer. Promotion injects it with a **forward** descriptor
   (`pack_forward_animation_descriptor`) plus a `GpuLight` record carrying a pool shadow
   slot, color premultiplied by `w`. The dynamic loop then evaluates its animated
   color/brightness/direction at runtime and shadow-maps it, exactly like a dynamic-tier
   light — but rationed through the promotion budget, not authored as dynamic.

4. **Direction animation resolves here.** Because the runtime cone is live, a promoted
   direction-animated light casts a correctly-swept shadow and lights the mover from the
   live direction — the escalation v1 named. Off the pool (unpromoted), it still reverts
   to the v1 rest-direction baked delta. So direction animation degrades gracefully with
   `w`, never popping.

5. **Depth-cache split by whether direction animates.** A promoted animated light's
   position is fixed, so a brightness/color-only light's static-world shadow depth is
   cacheable exactly like a static promoted light (only entity occluders re-render). A
   direction-animated light's frustum moves each frame, invalidating the cache, so it
   re-renders world depth per frame (dynamic-tier cost) — into its cache layer, never
   the pool slot: under `promoted-shadow-entity-only-depth` a promoted pool slot holds
   entity depth only and the cache is the sampled world-depth source. The compiler can tag
   `has_direction_curve` per animated light so the runtime picks the path without probing
   the curve.

6. **Crossfade lifecycle reuses static promotion's ramp.** Same `PROMOTE_SECONDS` /
   `DEMOTE_SECONDS` / `STICKY_SECONDS` / `EVICTION_MARGIN`. When a receiver leaves the
   influence or the budget evicts, `w` ramps to 0 and the light reverts fully to the
   section-45 delta — no pop, no self-shadow, identical to v1.

## Acceptance criteria

- [ ] `[golden]` A mover inside a promoted animated light's cone casts a self-shadow that
      tracks the animated brightness/color; off the pool it shows the v1 flat baked delta.
- [ ] `[unit]` A promoted animated light is counted exactly once: Pass B applies
      `(1−w)` to its delta and the dynamic loop applies `w` to its runtime term, summing to
      the unpromoted radiance at any `w` (energy-conservation test at the CPU scale seam).
- [ ] `[golden]` Crossfade shows no brightness pop across promote (`w: 0→1`), evict, and
      demote (`w: 1→0`); at `w=0` the frame is identical to v1 (delta only).
- [ ] `[golden]` A direction-animated promoted light casts a swept shadow following the
      direction curve; unpromoted it uses the rest-direction delta (v1 limitation restored).
- [ ] `[unit]` The pool ranker orders an animated candidate by its authored (peak)
      intensity, not its instantaneous strobe value — a strobing light does not thrash in
      and out of a slot frame-to-frame.
- [ ] `[unit]` Budget is respected: with more eligible animated + dynamic + static-promoted
      lights than slots, only the top `MAX_PROMOTED_SPOT`/`MAX_PROMOTED_CUBE` promote; the
      rest keep the v1 baked delta.
- [ ] `[manual GPU]` A brightness/color-only promoted animated light reuses the
      promoted-depth-cache (world depth rendered once on assignment); a direction-animated
      one re-renders world depth per frame — verifiable via `POSTRETRO_GPU_TIMING`.
- [ ] `[golden]` + `[review]` Fixture: the spawner-test alarm light, promoted when the
      closet door enters its cone, casts a moving door self-shadow that reddens with the
      alarm curve; world surfaces are unchanged.

## Tasks

### Task 1: Compiler — `has_direction_curve` tag

Tag each `AnimatedBakedLights` entry with whether its animation carries a direction curve
(`animation.direction.is_some()`), emitted alongside section 45 (or in the animation
descriptor metadata the runtime already loads). The runtime uses it to pick the
depth-cache-vs-per-frame path (Decision 5) without inspecting the curve. Wire into the v1
section-45 bake.

### Task 2: Runtime — animated candidate eligibility + ranking

Extend the promotion driver (`update_dynamic_light_slots` /
`selected_static_light_has_shadow_entity`) to admit animated baked lights as pool
candidates: receiver-in-influence + portal-reachable + peak-intensity/range/decorative
heuristics. Rank them in the existing `assign_slots_with_hysteresis` on their **authored
peak** intensity (a new stable-intensity input, so a strobe does not reorder the ranking
each frame). Candidacy shares the existing `MAX_PROMOTED_SPOT`/`MAX_PROMOTED_CUBE` budget
with dynamic and static-promoted lights (see Open questions for the contention policy).

### Task 3: Runtime — inject the promoted animated light + `(1−w)` compose factor

For each promoted animated light: build a `GpuLight` record with a pool slot and a
**forward** animation descriptor (`pack_forward_animation_descriptor`), color premultiplied
by `w`, appended to the runtime `lights` buffer (the light does not enter it when
unpromoted). Add a per-animated-light promotion-weight buffer (parallel to
`selection_weights`) and extend v1's Pass B to multiply each animated light's delta add by
`(1−w)`. The dynamic loop's shadow attenuation reuses the spot/cube pool sampling. Ensure
brightness is single-sourced (the CPU `effective_brightness` feeds both the forward record
and the Pass B scale) so the `(1−w)`/`w` split is energy-exact.

### Task 4: Runtime — depth-cache split

Route brightness/color-only promoted animated lights through the static
promoted-depth-cache (fixed position + direction → cache world depth on assignment, redraw
only entity occluders). Route direction-animated ones (Task 1 tag) through per-frame world
depth (frustum moves). Both draw entity occluders each frame.

### Task 5: Fixture + docs

Extend the `animated-direct-sh-dynamic-receivers` fixture: promote the alarm light when the
closet door enters its cone, add a golden asserting the moving door self-shadow reddens with
the curve. Update `rendering_pipeline.md` §4 (the promotion paragraph now covers animated
lights; the receiver matrix's animated column gains a promoted tier) and the FGD comment. If
the pool-contention policy lands as a worldspawn KVP, document it.

## Sequencing

**Phase 1 (sequential):** Task 1 — the direction tag feeds Task 4's path split.
**Phase 2 (concurrent):** Task 2 (eligibility/ranking), Task 3 (injection + compose factor)
— Task 3 consumes the candidate set from Task 2's driver, but both develop against the
promotion-record shape; sequence Task 2 before Task 3 if the record type changes.
**Phase 3 (sequential):** Task 4 (depth-cache split) consumes Task 3's promoted records and
Task 1's tag.
**Phase 4 (sequential):** Task 5 — consumes the shipped runtime behavior.

## Boundary inventory

| Name | Rust | Wire / serde | WGSL | FGD KVP |
|---|---|---|---|---|
| Direction tag | `has_direction_curve` per `AnimatedBakedLights` entry | section-45 metadata (or descriptor) | consumed CPU-side | n/a |
| Animated promotion record | mirror `PromotedStaticLightRecord` (`weight`, `slot`, `pool_kind`) keyed by `AnimatedBakedLights` index | n/a | n/a | n/a |
| Promotion weight (compose) | per-animated-light `(1−w)` buffer (parallel to `selection_weights`) | n/a | Pass B storage buffer | n/a |
| Runtime record | `pack_forward_animation_descriptor` + `GpuLight` (color × `w`, pool slot) | n/a | dynamic-direct `lights` loop | n/a |
| Budget | shared `MAX_PROMOTED_SPOT` / `MAX_PROMOTED_CUBE` | n/a | n/a | pool-policy KVP (open) |

## Open questions

- **Pool-contention policy — the one decision AI speed cannot make.** Promoting animated
  lights breaks the v1 contract's budget separation "unless promotion is deliberately
  designed" — this is that deliberate design. During a scripted set-piece, should a pulsing
  alarm be allowed to evict a static key light or a dynamic muzzle flash from the 8-spot
  pool? Options: (a) share the budget, rank all tiers together on stable intensity
  (default sketch — simplest, but a strobe can starve gameplay lights); (b) reserve N slots
  for authored-dynamic lights so scripted animation never evicts combat lighting;
  (c) a worldspawn KVP letting the author pick per map. This is a gameplay-feel decision for
  the game author, not an engine default — needs the owner's call before Task 2 locks the
  ranker inputs. Recommendation: ship (a) with the ranker seam shaped so (b)/(c) drop in.
- **Ranking a strobe fairly.** Peak intensity avoids frame-thrash, but a light authored
  bright-but-usually-dark (a slow pulse) would hold a slot it rarely uses. Consider ranking
  on a short-window *max* of the curve rather than the authored peak. Decide with (a) above.
- **Direction-animated depth cost.** Per-frame world depth for a swept promoted light is
  dynamic-tier cost against the compat-floor GPU. If measured too costly, cap the number of
  concurrently-promoted direction-animated lights below the general budget.
