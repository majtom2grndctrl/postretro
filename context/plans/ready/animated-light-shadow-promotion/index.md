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
   plus the same dim/short-range/decorative heuristic thresholds. The brightness the
   heuristic reads for an animated candidate is a **forward-lookahead window max** of its
   brightness curve over `[t_now, t_now + PROMOTE_SECONDS]` (the window length equals the
   `w` ramp time), not the instantaneous strobe value. This must hold for the per-frame
   suppression gate, not only the ranker: `visible_lights` feeds `gate_passed`, which
   drives the `w` promote/demote ramp, and today its brightness term is the instantaneous
   `effective_brightness` keyed on dynamic `level_lights` — a baked animated light is
   absent there and defaults to `1.0`, i.e. exempt from suppression only by index-space
   accident, so the driver must be fed a real value for animated candidates. Supply the
   window max: the bridge already holds each animated light's authored brightness curve (an
   `Option<Vec<f32>>` on `LightComponent.animation`) and the CPU Catmull-Rom sampler
   (`sample_brightness_at` / `sample_brightness_at_open`) it uses for dynamic-tier
   `effective_brightness`, so computing the window max reuses that sampler and curve — a bounded
   loop, no new evaluator. Producing and delivering the value is the new plumbing Task 1 owns:
   the bridge's `effective_brightness` is built only for `is_dynamic` lights, so a baked animated
   light has no entry there, and animated candidates instead need a per-candidate value fed to
   the gate through a distinct keyed channel (Task 1). The `PROMOTE_SECONDS` window length is
   shared from the renderer crate, where it is defined. The lookahead is what makes the crossfade lag-free: the
   gate opens `PROMOTE_SECONDS` before the curve would cross the threshold, exactly the time
   `w` needs to reach 1, so the self-shadow is already full-strength when the light is
   bright. The no-pop criterion does not depend on holding `w` fixed: the receiver term
   `(1−w)·scale_j(t)·delta + w·scale_j(t)·runtime` carries the shared brightness `scale_j(t)`
   in both arms (Decision 2), so at low brightness the term is ≈0 for *any* `w` and a `w`
   change is invisible; a `w` change is visible only at high brightness, where the lookahead
   guarantees `w=1`. A light therefore releases its slot during a dark stretch longer than
   the window plus `STICKY_SECONDS` plus `DEMOTE_SECONDS` — the gate fails through
   `STICKY_SECONDS`, then `w` ramps to 0 over `DEMOTE_SECONDS`, and only at `w=0` is the slot
   released — invisibly, since `scale_j(t)` ≈ 0 throughout the dark, and reclaims it before the
   next peak whenever the shared pool has a free slot, so a mostly-dark slow pulse no longer holds
   a slot it is not using. The window `[t_now, t_now + PROMOTE_SECONDS]` is real-time; map it into the curve's `[0, 1)`
   domain as `PROMOTE_SECONDS / period_s` and sample it with the light's own mode
   (`sample_brightness_at` closed-loop, `sample_brightness_at_open` endpoint-clamped), so a window
   spanning at least one full period sees the whole curve and its max saturates to
   `max(brightness)`. At a degenerate period (`period_s ≤ 0`) the runtime freezes `scale_j(t)` at
   `brightness[0]` (the sampler at cycle 0), so the gate reads `brightness[0]` — exactly the value
   the light renders — neither over- nor under-promoting it. The "selection index" for the weight buffer is simply
   the `AnimatedBakedLights` index.

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
   influence, the budget evicts, or the window-max brightness gate stays closed past the release
   boundary (Decision 1, P6b), `w` ramps to 0 and the light reverts fully to the section-45
   delta — no pop, no self-shadow, identical to v1.

6. **Pool contention: animated candidates share the budget.** Promoting animated lights is
   the deliberate budget-sharing the v1 contract reserved for designed promotion. Animated
   candidates compete in the existing `MAX_PROMOTED_SPOT`/`MAX_PROMOTED_CUBE` pool against
   dynamic and static-promoted lights, ranked on the one shared `slot_score` (which carries
   no intensity term); an animated candidate's brightness enters only its membership gate, as
   the window max of Decision 1. A bright scripted strobe can therefore win a slot from a
   gameplay light — accepted for this plan. The gate/ranker seam is shaped so a
   reserved-dynamic-slots split or a per-map worldspawn KVP drops in later without re-deriving
   the candidate set, if a set-piece ever starves combat lighting.

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
   the full `(1−w)=1` delta, byte-identical to v1). The reserved forward tail lives in the `lights` storage buffer, which is runtime-sized, so it
   needs no fixed cap. The per-animated-light `(1−w)` weights, however, ride the binding-26
   uniform (boundary inventory), and a WGSL uniform cannot be runtime-sized: it is a fixed-length
   array bounded by a named cap `MAX_ANIMATED_BAKED_LIGHTS`. A map whose animated-baked-light
   count exceeds the cap leaves the surplus lights unpromotable — Pass B applies their full
   `(1−w)=1` delta (v1 behavior). Packing the descriptor in the renderer at promotion time (no
   idle slots) is a strict optimization of this same seam if a map's animated-light count ever
   pressures the `lights` buffer.

## Acceptance criteria

- [ ] `[golden]` A mover inside a promoted animated light's cone casts a self-shadow whose
      brightness/color matches the light's curve value at the captured instant (a forced-`w`
      differential still); off the pool it shows the v1 flat baked delta.
- [ ] `[unit]` A promoted animated light is counted exactly once at the CPU scale seam: the
      Pass B `(1−w)` compose weight and the forward `w` color multiplier are written from a single
      `state.weight`, so `(1−w) + w == 1` at every `w`; the shared-`scale_j(t)` half of the
      exactness — the identical curve sample in both arms — completes the count-once guarantee.
      (The visual sum-to-v1 radiance is a forced-`w` stills golden, not a CPU value: `scale_j(t)` is
      GPU-sampled and never appears CPU-side.)
- [ ] `[golden]` Forced-`w` stills capture the no-pop guarantee the single-instant harness
      cannot capture as a runtime ramp: at `w=0` the frame is byte-identical to v1 (delta only),
      and across forced `w` values a lit (non-self-shadowed) receiver texel holds constant radiance
      while self-shadowed texels darken with rising `w`.
- [ ] `[unit]` An animated candidate's promotion eligibility is gated on the forward-lookahead
      window max of its brightness curve (Decision 1), not its instantaneous strobe value — a
      strobe whose dark trough is shorter than the lookahead window plus `STICKY_SECONDS` does
      not thrash in and out of the pool frame-to-frame, and its `w` holds across the trough
      (the shared ranker `slot_score` carries no intensity term).
- [ ] `[unit]` Budget is respected: with more eligible animated + dynamic + static-promoted
      lights than slots, only the top `MAX_PROMOTED_SPOT`/`MAX_PROMOTED_CUBE` promote; the
      rest keep the v1 baked delta.
- [ ] `[manual GPU]` A promoted animated light reuses the promoted-depth-cache: world depth
      is rendered once on assignment and only entity occluders re-render per frame —
      verifiable via `POSTRETRO_GPU_TIMING` (no per-frame world-depth pass appears).
- [ ] `[golden]` + `[review]` Fixture: the spawner-test alarm light, promoted when the
      closet door enters its cone, casts a door self-shadow that reddens with the alarm curve —
      captured as forced-`w` promoted stills at the door's rest pose (the single-instant harness
      draws no motion), differential against the off-pool flat delta; world surfaces are unchanged
      (`[review]` grep gate: world stays `lm_anim`).
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
index (the runtime-only eligibility of Design decision 1). Thread the `AnimatedBakedLights`
roster and each light's `MapLight` record (fixed position, falloff range, rest direction — the
fields `candidate_slot_score` and the reachability gate read) into the builder alongside the
existing `is_dynamic` and `EntityShadowLights` sources; these baked lights appear in neither
existing source, so the builder takes an explicit new input for them. There is no ready-made
roster: assemble it from the animated baked lights (those carrying a section-45 delta), tagging
each with its `AnimatedBakedLights` index — the value Pass B's `affinity_lights` entries already
carry into `animated_light_scale` as `light_index` (which also indexes
`animation_descriptor_indices`). That one index keys the candidate tag, the `(1−w)` weight array
(Task 2), and the depth-cache selection index; do not conflate it with `MapLight.animated_slot`
or a descriptor-table position, or the weight buffer mis-keys silently. Then gate those candidates in
the driver — receiver-in-influence + portal-reachable + brightness/range/decorative
heuristics. Gate their eligibility on the forward-lookahead window max of the brightness
curve over `[t_now, t_now + PROMOTE_SECONDS]` — the window length equals the `w` ramp time,
mapped into the curve's `[0, 1)` domain as `PROMOTE_SECONDS / period_s` and sampled with the
light's own mode (`sample_brightness_at` closed-loop, `sample_brightness_at_open`
endpoint-clamped); at `period_s ≤ 0` read `brightness[0]` (Decision 1) — not the instantaneous
strobe value, by feeding that value into the
driver as a second brightness input keyed by `AnimatedBakedLights` index — a new
`update_dynamic_light_slots` parameter, NOT the existing `effective_brightness`, which is keyed
on the dynamic-tier `level_lights` array where a baked animated light has no entry and the
per-candidate suppression check defaults it to `1.0` (always exempt). The per-candidate loop
reads this keyed window-max array for animated candidates and `effective_brightness` for dynamic
candidates. Compute the window max in the bridge from the light's CPU-resident `brightness` curve
via the existing `sample_brightness_at` / `sample_brightness_at_open` sampler (`PROMOTE_SECONDS`
is today a private function-local const in the renderer's ramp code — hoist it to a crate-public
location the bridge can import), so a strobe does not thrash a
candidate in and out of eligibility frame-to-frame and the lookahead lets `w` reach full
strength before the light is bright. Ranking itself stays on the
existing `assign_slots_with_hysteresis` score (`slot_score`, range/distance) — leave that
shared formula untouched; it carries no intensity term and must not gain one, or dynamic
and static-promoted ranking drift with it. Candidacy shares the existing `MAX_PROMOTED_SPOT`/`MAX_PROMOTED_CUBE` budget
with dynamic and static-promoted lights, gated and ranked as one pool (Decision 6).

### Task 2: Runtime — inject the promoted animated light + `(1−w)` compose factor

The bridge emits a **forward** animation descriptor (`pack_forward_animation_descriptor`)
carrying the brightness/color curve with the cone held at the authored rest direction — the
injected `GpuLight`'s direction is the rest cone and the forward descriptor packs no direction
curve, so a direction-animated light does not sweep; unit-test the injected `GpuLight` direction
against rest for a direction-curve light, delivering the frozen-cone acceptance criterion — into a
reserved `lights`-buffer tail slot for every animated baked light each frame; the renderer's
`update_dynamic_light_slots` assigns the pool shadow slot and sets `w`, premultiplying the
`GpuLight` color by `w` (Decision 7). Unpromoted lights sit at `w=0`, contributing nothing. Add a per-animated-light `(1−w)` promotion weight, one per `AnimatedBakedLights` index,
delivered through the existing binding-26 uniform (Pass B is at its 8-storage-buffer maximum, so
no new storage buffer): the `(1−w)` array absorbs v1's `debug_override` uniform — replace
`animated_light_scale`'s single `debug_weight` scalar with `weight_array[light_index]`, and keep
the `enabled`/`light_index` single-light isolation fields as a dev-tools overlay composing on top
of the always-applied `(1−w)`. Extend v1's Pass B to multiply each animated light's delta add by
`(1−w)`. The dynamic loop's shadow attenuation reuses the spot/cube pool sampling. The `(1−w)`/`w` split is energy-exact by construction, with no CPU brightness plumbing: the
forward record's runtime radiance and v1's Pass B delta scale both GPU-sample the same
`anim_samples` curve through the same `curve_eval.wgsl` helper (`sample_curve_catmull_rom` /
`sample_color_catmull_rom`), so `scale_j(t)` is one value in both terms. The CPU
eligibility scalars — `effective_brightness` for dynamic-tier lights, the separate keyed
window-max channel for animated candidates (Decision 1, Task 1) — are shadow-slot eligibility
signals only; neither is the radiance source and neither feeds either term — do not route
radiance through them, and do not lean on `single-source-animated-light-brightness`, which
moves only the forward path to a CPU scalar and would break this equality.

### Task 3: Runtime — depth-cache reuse

Route promoted animated lights through the static promoted-depth-cache unchanged: because
position and rest direction are both fixed, cache the static-world depth on assignment and
redraw only entity occluders per frame. No direction-dependent path split — the rest-cone
freeze (Task 2) is what keeps the cached world depth valid every frame. When the depth
cache has no free layer for a promoted record, the drop path (mirroring
`apply_promoted_cache_layers`) removes that record for the frame; it MUST also zero the
animated `(1−w)` promotion-weight buffer — a buffer distinct from the static
`promoted_static_weights` that `apply_promoted_cache_layers` already zeroes by its
`EntityShadowLights` selection index — at that light's `AnimatedBakedLights` index in the same
pass. Otherwise Pass B keeps fading the delta by `(1−w)` with no runtime term to
replace it — an energy deficit on the receiver for that frame.

### Task 4: Fixture + docs

Extend the `animated-direct-sh-dynamic-receivers` fixture: promote the alarm light when the
closet door enters its cone, add a golden asserting the door self-shadow reddens with the curve —
captured as forced-`w` promoted stills at the door's rest pose (the single-instant harness draws
no motion), differential against the off-pool flat delta. Update `rendering_pipeline.md` §4 (the promotion paragraph now covers animated
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
- **P6 (Task 1):** a strobe whose dark trough is shorter than the lookahead window (Decision 1)
  plus `STICKY_SECONDS` holds `w` — gate brightness is the window max, not the instantaneous
  sample, so the window sees the next peak and `w` does not re-fade each cycle.
- **P6b (Task 1):** a pulse dark for longer than the lookahead window plus `STICKY_SECONDS` plus
  `DEMOTE_SECONDS` releases its slot during the dark and — when the shared pool has a free slot
  (Decision 6) — reclaims it within `PROMOTE_SECONDS` of the next peak. On gate-fail `w` holds
  through `STICKY_SECONDS`, then ramps to 0 over `DEMOTE_SECONDS`; only at `w=0` is the record
  dropped and the cache layer freed, so a dark stretch merely longer than the window plus
  `STICKY_SECONDS` starts the ramp but reclaims the slot before `w=0` and releases nothing. A
  released light drops out of the incumbent set, so its next-peak reclaim competes for a free or
  evictable slot and is not guaranteed under the shared budget; when it loses it stays on the baked
  delta (`w=0`). Because brightness `scale_j(t)` ≈ 0 whenever `w` changes here, the receiver term is
  unchanged either way (no visible pop) — the tightening that a mostly-dark slow pulse no longer
  holds an unused slot.
- **P7 (Task 1/3):** a level unload / receiver despawn resets the animated weight-state, the
  `(1−w)` buffer, and the cache layer — no stuck `w`, no leaked layer.
- **P8 (Task 1):** the ranker holds for N eligible animated lights at every N including 0 and
  N>`MAX_PROMOTED_SPOT`/`MAX_PROMOTED_CUBE`, with a deterministic tie-break on equal `slot_score`
  (ascending candidate index, per `assign_slots_with_hysteresis`).
- **P9 (Task 1):** at a degenerate period (`period_s ≤ 0`) the gate reads `brightness[0]` — the
  frozen value the runtime renders — so a frozen-dark light is not promoted and a frozen-bright one
  is promoted and held, gate matching render with no over- or under-promotion (Decision 1).
- **P10 (Task 1):** a released animated slot (dark stretch past the window plus `STICKY_SECONDS`
  plus `DEMOTE_SECONDS`) is won by a higher-scored dynamic or static-promoted light during the
  dark; at the next peak the animated light finds no free slot, stays at `w=0` (baked delta), and
  casts no self-shadow that peak — no pop, since the delta is the `w=0` state — reclaiming only on a
  later peak once a slot frees.
- **P11 (Task 1):** a degenerate-period (`period_s ≤ 0`) animated light gates on `brightness[0]`
  (P9): with `brightness[0]` below `BRIGHTNESS_SUPPRESSION_THRESHOLD` it is not promoted (no slot
  held for a light that renders ≈0); with `brightness[0]` above it, it promotes and holds a slot at
  constant `w`, like a static promoted light.
- **P12 (Task 1):** a bright peak narrower than one frame interval — the CPU window-max scans the
  sample curve and promotes (`w` ramps up), while the GPU `scale_j(t)` sampled at frame times may
  stay at the trough, so the receiver renders no self-shadow that cycle and `w` releases after the
  following dark per P6b; the gate-scans-curve vs render-samples-at-frame-times asymmetry is benign
  (no pop, `scale_j(t)` ≈ 0 while `w` moves).
- **P13 (Task 2):** N animated candidates cross the gate in one tick (every N including 0,
  N>`MAX_PROMOTED_SPOT`/`MAX_PROMOTED_CUBE`, and N>`MAX_ANIMATED_BAKED_LIGHTS`): each light whose
  `AnimatedBakedLights` index is below `MAX_ANIMATED_BAKED_LIGHTS` has its `(1−w)` buffer entry and
  forward `color×w` written from its own `state.weight` keyed on that index, with no `w` bleed
  across lights and the per-light writes staying index-parallel under batching (extends P1 to the
  batch); a light whose index reaches `MAX_ANIMATED_BAKED_LIGHTS` gets no `(1−w)` entry and stays on
  the full `(1−w)=1` delta (Decision 7).

## Boundary inventory

| Name | Rust | Wire / serde | WGSL | FGD KVP |
|---|---|---|---|---|
| Animated promotion record | `PromotedStaticLightRecord` populated for an animated candidate — `weight`, `slot`, `pool_kind`, plus the `global_light_index`/`selection_index` the depth-cache `CacheKey` keys on — with its `AnimatedBakedLights` index as the selection index | n/a | n/a | n/a |
| Promotion weight (compose) | per-animated-light `(1−w)` weight, one per `AnimatedBakedLights` index, capped at `MAX_ANIMATED_BAKED_LIGHTS` | n/a | Pass B `animated_light_scale` weight factor — generalizes v1's single-light dev-tools `debug_override.weight` uniform (binding 26) into an always-applied per-light `(1−w)` array; Pass B already binds its 8-storage-buffer maximum, so the weights ride the binding-26 uniform as a fixed-length array (a WGSL uniform cannot be runtime-sized) rather than a new storage buffer | n/a |
| Runtime record | `pack_forward_animation_descriptor` (brightness/color, rest cone) + `GpuLight` (color × `w`, pool slot) — bridge-emitted for every animated baked light, `w=0` unpromoted; renderer assigns slot + `w` (Decision 7) | n/a | dynamic-direct `lights` loop | n/a |
| Budget | shared `MAX_PROMOTED_SPOT` / `MAX_PROMOTED_CUBE`, all tiers ranked on `slot_score` (no intensity term); animated candidates gated on the window-max brightness of Decision 1 (Decision 6) | n/a | n/a | reserved-slot / KVP variant (future) |

## Open questions

None. (The gate brightness signal is specified in Decision 1: a forward-lookahead window max
of the brightness curve.)
