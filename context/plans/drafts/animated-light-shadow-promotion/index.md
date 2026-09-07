# Animated-light shadow-map promotion

## Goal

Give a script-animated baked light true runtime self-shadowing on moving receivers.
The v1 feature (`animated-direct-sh-dynamic-receivers`) lit movers/meshes/billboards
under an animated light through a baked static-occlusion SH delta (section 45) — but the
delta cannot encode a receiver's own geometry, so movers cast no self-shadow. This
follow-on promotes an animated baked light into the runtime spot/cube shadow pool near a
moving receiver, crossfading from the baked delta to a shadow-mapped runtime term — the
self-shadow escalation v1 named as its promotion tier. It is the exact inverse of
`static-light-entity-shadows`, which promoted *static* lights by subtracting against the
`DirectShDeltaVolumes` delta (id 41); here the subtraction is against v1's *additive* animated
delta (id 45). Promotion crossfades **brightness and color only**; the runtime cone stays
at the light's authored **rest direction**, matching the baked delta — a cone that must
sweep across movers with a live direction remains authored `light_dynamic_spot`, per v1's
decision (its Out of scope, grounded in the `index.md` §2 static-vs-dynamic invariant).

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
- The runtime term: the bridge emits a **forward** animation descriptor
  (`pack_forward_animation_descriptor`, runtime-evaluated brightness/color curve, direction
  frozen at the authored rest cone) into the dynamic-direct `lights` buffer for every animated
  baked light each frame, color premultiplied by `w`; the renderer assigns a pool shadow slot
  and sets `w` at promotion. Unpromoted, `w=0` — the record contributes nothing and Pass B
  applies the full delta (Decision 7). Today such a light never enters the `lights` buffer.
- Pass B (v1) gains a per-animated-light `(1−w)` promotion-weight factor so the baked
  delta fades as the runtime term rises.
- Runtime shadow depth reuses the static promoted-depth-cache: because a promoted light's
  position and (rest) direction are both fixed, its static-world depth is rendered once on
  assignment; only entity occluders re-render per frame — identical to static promotion.

### Out of scope

- **World-surface self-shadowing under animated lights.** World stays `lm_anim`; only
  moving receivers get the runtime term (same boundary static promotion holds).
- **Billboards as promotion receivers.** They evaluate per-vertex unshadowed, as under
  static promotion; they keep the v1 baked delta only.
- **Direction sweep on promoted lights.** The runtime cone stays at the authored rest
  direction, matching the baked delta. A light whose animation carries a direction curve
  is promoted for its brightness/color self-shadow with that curve frozen at rest, exactly
  as v1 freezes it — a swept cone over movers is authored `light_dynamic_spot`, the
  dynamic tier's job by v1's design (its Out of scope; `index.md` §2 invariant).
- **Raising the pool budget by default.** Animated candidates *share* the existing
  `MAX_PROMOTED_SPOT`/`MAX_PROMOTED_CUBE` budget rather than enlarging it (Decision 6);
  reserved-slot and worldspawn-KVP contention variants are future options, not this plan.
- **Compile-time animated-promotion selection list.** Every animated baked light already
  has a section-45 delta, so eligibility is runtime-only — no `EntityShadowLights` analog.

## Design decisions

1. **No compile-time selection; runtime eligibility only.** Static promotion needs a
   compile-time `EntityShadowLights` list because not every static light has a
   `DirectShVolume` delta. Every animated baked light already carries a section-45 delta,
   so any of them is a candidate. Eligibility is decided at runtime by the same gates
   static promotion uses at runtime — receiver-in-influence + portal-reachable + budget —
   plus the same dim/short-range/decorative heuristic thresholds, evaluated on the light's
   **authored (peak)** intensity, not its instantaneous strobe value. This must hold for the
   per-frame suppression gate, not only the ranker: `visible_lights` feeds `gate_passed`,
   which drives the `w` promote/demote ramp, and today its brightness term is the
   instantaneous `effective_brightness` keyed on dynamic `level_lights` — a baked animated
   light is absent there and defaults to `1.0`, i.e. exempt from instantaneous suppression
   only by index-space accident. Pin the gate brightness for animated candidates to the
   constant authored peak, so a strobe whose dark trough exceeds `STICKY_SECONDS` does not
   pump `w` and re-fade the self-shadow each cycle (the no-pop crossfade criterion). The "selection
   index" for the weight buffer is simply the `AnimatedBakedLights` index.

2. **The subtraction seam is v1's additive delta, scaled down.** v1 Pass B writes
   `Σ_anim(scale_j(t) × delta_j)`. For a promoted animated light at weight `w`, Pass B
   writes `(1−w) × scale_j(t) × delta_j`; the dynamic-direct loop adds
   `w × scale_j(t) × runtime_shadowed_j`. Both the forward runtime term and Pass B's delta scale GPU-sample the *same*
   `anim_samples` curve through the shared `curve_eval.wgsl` helper (`sample_curve_catmull_rom`
   / `sample_color_catmull_rom`) at the *same* rest direction, so `scale_j(t)` is one number
   in both terms by construction — energy holds and no receiver sums the light twice. This is the mirror of static
   promotion's `(1−w)·baked_SH + w·runtime` on the additive side.

3. **The runtime term needs a forward descriptor, direction frozen at rest.** A baked
   animated light today has only a compose descriptor (`pack_compose_animation_descriptor`)
   and never enters the runtime `lights` buffer. Promotion injects it with a **forward**
   descriptor (`pack_forward_animation_descriptor`) plus a `GpuLight` record carrying a
   pool shadow slot, color premultiplied by `w`. The dynamic loop then evaluates its
   animated color/brightness at runtime and shadow-maps it, exactly like a dynamic-tier
   light — but with its cone held at the authored rest direction (so the runtime term
   encodes the *same* illumination the baked delta does, just crisply self-shadowed), and
   rationed through the promotion budget, not authored as dynamic. Any direction curve on
   the light is left unevaluated by the forward record, the same freeze v1 applies.

4. **Shadowed exactly like a static promoted light.** A promoted animated light's position
   is fixed and its cone is held at rest, so its frustum over the static world never moves:
   world shadow depth is cacheable exactly like a static promoted light — rendered once on
   assignment, with only entity occluders re-rendered per frame. Under
   `promoted-shadow-entity-only-depth` the pool slot holds entity depth only and the cache
   is the sampled world-depth source. No per-frame world redraw and no direction-dependent
   path split: freezing the cone at rest is what makes the static depth cache always valid.

5. **Crossfade lifecycle reuses static promotion's ramp.** Same `PROMOTE_SECONDS` /
   `DEMOTE_SECONDS` / `STICKY_SECONDS` / `EVICTION_MARGIN`. When a receiver leaves the
   influence or the budget evicts, `w` ramps to 0 and the light reverts fully to the
   section-45 delta — no pop, no self-shadow, identical to v1.

6. **Pool contention: animated candidates share the budget.** Promoting animated lights is
   the deliberate budget-sharing the v1 contract reserved for designed promotion. Animated
   candidates compete in the existing `MAX_PROMOTED_SPOT`/`MAX_PROMOTED_CUBE` pool against
   dynamic and static-promoted lights: all tiers are gated on stable **authored-peak**
   intensity (Decision 1) and ranked on the one shared `slot_score`. A bright scripted strobe
   can therefore win a slot from a gameplay light — accepted for this plan. The gate/ranker
   seam is shaped so a reserved-dynamic-slots split or a per-map worldspawn KVP drops in later
   without re-deriving the candidate set, if a set-piece ever starves combat lighting.

7. **The forward record is bridge-emitted for every animated baked light; promotion sets `w`.**
   `pack_forward_animation_descriptor` needs the `LightComponent` and its `anim_samples`
   offsets, which live in the Game-logic bridge; promotion is decided a stage later in the
   renderer's `update_dynamic_light_slots`, which holds neither. So the bridge emits a forward
   record and reserves a `lights`-buffer tail slot for **every** animated baked light each
   frame, and the renderer only assigns the pool slot and sets `w`. This is lag-free — the
   record already exists when promotion decides, so the crossfade never waits a frame — and it
   keeps all forward-record packing in one place (the bridge), leaving the renderer's role
   identical to how it already patches dynamic-light slots. Cost: one tail slot per animated
   baked light per frame, `w=0` when unpromoted (a zero-contribution multiply; Pass B applies
   the full `(1−w)=1` delta, byte-identical to v1). The count of animated baked lights is
   small, so the cost is bounded; packing the descriptor in the renderer at promotion time
   (no idle slots) is a strict optimization of this same seam if a map's animated-light count
   ever pressures the `lights` buffer.

## Acceptance criteria

- [ ] `[golden]` A mover inside a promoted animated light's cone casts a self-shadow that
      tracks the animated brightness/color; off the pool it shows the v1 flat baked delta.
- [ ] `[unit]` A promoted animated light is counted exactly once: Pass B applies
      `(1−w)` to its delta and the dynamic loop applies `w` to its runtime term, summing to
      the unpromoted radiance at any `w` (energy-conservation test at the CPU scale seam).
- [ ] `[golden]` Crossfade shows no brightness pop across promote (`w: 0→1`), evict, and
      demote (`w: 1→0`); at `w=0` the frame is identical to v1 (delta only).
- [ ] `[unit]` An animated candidate's promotion eligibility is gated on its authored (peak)
      intensity, not its instantaneous strobe value — a strobing light does not thrash in and
      out of the pool frame-to-frame (the shared ranker `slot_score` carries no intensity
      term; the gate holds `w` across a dark trough shorter than `STICKY_SECONDS`).
- [ ] `[unit]` Budget is respected: with more eligible animated + dynamic + static-promoted
      lights than slots, only the top `MAX_PROMOTED_SPOT`/`MAX_PROMOTED_CUBE` promote; the
      rest keep the v1 baked delta.
- [ ] `[manual GPU]` A promoted animated light reuses the promoted-depth-cache: world depth
      is rendered once on assignment and only entity occluders re-render per frame —
      verifiable via `POSTRETRO_GPU_TIMING` (no per-frame world-depth pass appears).
- [ ] `[golden]` + `[review]` Fixture: the spawner-test alarm light, promoted when the
      closet door enters its cone, casts a moving door self-shadow that reddens with the
      alarm curve; world surfaces are unchanged.
- [ ] `[unit]` A promoted animated light that carries a direction curve is injected with
      its cone at the authored rest direction: the forward record evaluates no direction
      curve, so the promoted runtime cone does not sweep — the frozen-cone invariant that
      keeps the static depth cache valid (Decision 4).

## Tasks

### Task 1: Runtime — animated candidate eligibility + ranking

Extend the promotion driver (`update_dynamic_light_slots` /
`selected_static_light_has_shadow_entity`) to admit animated baked lights as pool
candidates. The runtime candidate set (`shadow_candidate_lights`) is built at init by
`filter_entity_shadow_candidates_with_selection` from `is_dynamic` lights plus the
compile-time `EntityShadowLights` section — an animated baked light is neither, so it
never reaches the driver today. Add a third candidate source in that builder: every
animated baked light carrying a section-45 delta, tagged with its `AnimatedBakedLights`
index (the runtime-only eligibility of Design decision 1). Then gate those candidates in
the driver — receiver-in-influence + portal-reachable + peak-intensity/range/decorative
heuristics. Gate their eligibility on **authored peak** intensity, not the instantaneous strobe value,
by feeding that stable value into the driver's brightness-suppression check (the
`effective_brightness` threshold in `update_dynamic_light_slots`), so a strobe does not
thrash a candidate in and out of eligibility frame-to-frame. Ranking itself stays on the
existing `assign_slots_with_hysteresis` score (`slot_score`, range/distance) — leave that
shared formula untouched; it carries no intensity term and must not gain one, or dynamic
and static-promoted ranking drift with it. Candidacy shares the existing `MAX_PROMOTED_SPOT`/`MAX_PROMOTED_CUBE` budget
with dynamic and static-promoted lights, gated and ranked as one pool (Decision 6).

### Task 2: Runtime — inject the promoted animated light + `(1−w)` compose factor

The bridge emits a **forward** animation descriptor (`pack_forward_animation_descriptor`)
carrying the brightness/color curve with the cone held at the authored rest direction, into a
reserved `lights`-buffer tail slot for every animated baked light each frame; the renderer's
`update_dynamic_light_slots` assigns the pool shadow slot and sets `w`, premultiplying the
`GpuLight` color by `w` (Decision 7). Unpromoted lights sit at `w=0`, contributing nothing. Add a per-animated-light `(1−w)` promotion weight, one per `AnimatedBakedLights` index
(the boundary inventory pins how Pass B receives it without a new storage buffer), and extend
v1's Pass B to multiply each animated light's delta add by `(1−w)`. The dynamic loop's shadow attenuation reuses the spot/cube pool sampling. The `(1−w)`/`w` split is energy-exact by construction, with no CPU brightness plumbing: the
forward record's runtime radiance and v1's Pass B delta scale both GPU-sample the same
`anim_samples` curve through the same `curve_eval.wgsl` helper (`sample_curve_catmull_rom` /
`sample_color_catmull_rom`), so `scale_j(t)` is one value in both terms. The CPU
`effective_brightness` scalar is the shadow-slot eligibility signal (Decision 1), produced
only for dynamic-tier lights; it is not the radiance source and feeds neither term — do not
route radiance through it, and do not lean on `single-source-animated-light-brightness`,
which moves only the forward path to a CPU scalar and would break this equality.

### Task 3: Runtime — depth-cache reuse

Route promoted animated lights through the static promoted-depth-cache unchanged: because
position and rest direction are both fixed, cache the static-world depth on assignment and
redraw only entity occluders per frame. No direction-dependent path split — the rest-cone
freeze (Task 2) is what keeps the cached world depth valid every frame. When the depth
cache has no free layer for a promoted record, the drop path (mirroring
`apply_promoted_cache_layers`) removes that record for the frame; it MUST also zero the
animated `(1−w)` promotion-weight buffer at that light's `AnimatedBakedLights` index in the
same pass. Otherwise Pass B keeps fading the delta by `(1−w)` with no runtime term to
replace it — an energy deficit on the receiver for that frame.

### Task 4: Fixture + docs

Extend the `animated-direct-sh-dynamic-receivers` fixture: promote the alarm light when the
closet door enters its cone, add a golden asserting the moving door self-shadow reddens with
the curve. Update `rendering_pipeline.md` §4 (the promotion paragraph now covers animated
lights; the receiver matrix's animated column gains a promoted tier) and the FGD comment.
Document the shared-budget contention policy (Decision 6); a reserved-slot or worldspawn-KVP
variant, if a later plan adds one, is documented then.

## Sequencing

**Phase 1 (concurrent):** Task 1 (eligibility/ranking) and Task 2 (injection + compose
factor) — Task 2 consumes the candidate set from Task 1's driver, but both develop against
the promotion-record shape; sequence Task 1 before Task 2 if the record type changes.
**Phase 2 (sequential):** Task 3 (depth-cache reuse) consumes Task 2's promoted records.
**Phase 3 (sequential):** Task 4 — consumes the shipped runtime behavior.

## Ordering pins

Test-writable orderings this spec asserts and must state (ids provisional):

- **P1 (Task 2):** the `(1−w)` buffer and the forward `color×w` are written from one
  `state.weight` before Pass B and the forward pass encode — both read the same `w`.
- **P2 (Task 2):** `scale_compose(t) == scale_forward(t)` at every `w` (the energy-conservation
  precondition behind the exactness test).
- **P3 (Task 3):** a dropped-record frame (no free cache layer) zeroes the animated `(1−w)`
  buffer at that light's index — no `(1−w)`-only deficit.
- **P4 (Task 3):** an assignment-frame slot renders its world depth before the forward samples it.
- **P5 (Task 2):** at `w==0` the reserved forward tail record contributes nothing (`color×0`)
  and Pass B applies the full `(1−w)=1` delta; frame is byte-identical to v1.
- **P6 (Task 1):** a slow strobe (dark trough > `STICKY_SECONDS`) holds `w` — gate brightness
  is authored peak, not instantaneous.
- **P7 (Task 1/3):** a level unload / receiver despawn resets the animated weight-state, the
  `(1−w)` buffer, and the cache layer — no stuck `w`, no leaked layer.
- **P8 (Task 1):** the ranker holds for N eligible animated lights at every N including 0 and
  N>cap, with a deterministic equal-peak tie-break.
- **P9 (Task 1):** the peak-intensity ranker input is well-defined at degenerate periods
  (`period_s ≤ 0`).

## Boundary inventory

| Name | Rust | Wire / serde | WGSL | FGD KVP |
|---|---|---|---|---|
| Animated promotion record | `PromotedStaticLightRecord` populated for an animated candidate — `weight`, `slot`, `pool_kind`, plus the `global_light_index`/`selection_index` the depth-cache `CacheKey` keys on — with its `AnimatedBakedLights` index as the selection index | n/a | n/a | n/a |
| Promotion weight (compose) | per-animated-light `(1−w)` weight, one per `AnimatedBakedLights` index | n/a | Pass B `animated_light_scale` weight factor — generalizes the existing `debug_override.weight` multiply; Pass B already binds its 8-storage-buffer maximum, so deliver the per-light weights without adding a storage buffer (the binding-26 uniform slot already carries this weight) | n/a |
| Runtime record | `pack_forward_animation_descriptor` (brightness/color, rest cone) + `GpuLight` (color × `w`, pool slot) — bridge-emitted for every animated baked light, `w=0` unpromoted; renderer assigns slot + `w` (Decision 7) | n/a | dynamic-direct `lights` loop | n/a |
| Budget | shared `MAX_PROMOTED_SPOT` / `MAX_PROMOTED_CUBE`, all tiers gated on authored-peak and ranked on `slot_score` (Decision 6) | n/a | n/a | reserved-slot / KVP variant (future) |

## Open questions

- **Gating a strobe fairly (refinement, non-blocking).** Decision 6 gates eligibility on the
  authored peak, which avoids frame-thrash but lets a light authored bright-but-usually-dark
  (a slow pulse) stay eligible for a slot it rarely uses. A short-window *max* of the curve,
  rather than the authored peak, would tighten this; it is a drop-in change to the same gate
  input and can wait for a measured case.
