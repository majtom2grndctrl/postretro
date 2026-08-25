# Cold-Bake Reaching-Light Cull — measurement spike

Build-to-learn. Deliverable is a decision, not a shippable feature.

## Goal

Answer one question: **in the cold base-indirect SH bake and the cold lightmap bake, what fraction of
`static_lights` actually reaches a given receiver, and is culling to that set worth a full
implementation spec?**

The cold base-indirect SH bake is ~83% of compile (212.7s of 257.5s on `campaign-test`); the cold
lightmap bake is ~13s. Both loop over the *entire* light set at every receiver and cast a shadow ray
per light before falloff/visibility zeroes the out-of-range ones — the shadow-ray-per-`(light,
receiver)` pair is the uncapped cost. The direct/delta/animated-direct SH bakes already avoid this via
a per-region reaching-light index (`affinity_grid` decompose: falloff-sphere AABB ∩ portal-reachability
flood, inverted to region→lights). The cold bakes never got it. This spike measures whether extending
the same cull to the cold bakes would meaningfully cut the 83%.

The measured finding is a **floor / mechanism bound**, not a production projection — see Prerequisites.

## Prerequisites

- **Runnable now — no blocking dependency.** The cull machinery ships (`affinity_grid` decompose +
  `ReachIndex` inversion, already consumed by the direct SH bake). The cold bakes exist. The only
  plumbing the spike needs is reaching the portal set inside the cold-bake context, which currently
  omits it — in scope for the spike.
- **Independent of the Cell-Visibility Relation spec.** This does not consume the baked `CellVisibility`
  section. It uses each light's own affinity flood. The relation's component array would supply only the
  portal-reachability half, which no-ops in a single-component map; the falloff-AABB half does the real
  culling and already exists. Do not couple the two.
- **Correctness constraints the spike must honor for its number to be valid** (a mismeasured bound
  teaches nothing):
  - Count reaching-lights at the **receiver's** cell — the ray's *hit point* for the indirect bake, the
    texel's world position for the lightmap — never the probe's cell. The bounce originates at the
    receiver; gating on the probe's cell would over- or under-count. (Valid because a straight bake ray
    cannot cross portal-disconnected components — portalization is complete.)
  - **Directional / sun lights are never culled.** They have no falloff sphere and their shadow ray can
    be occluded by geometry in another component; they contribute everywhere. The affinity decompose
    already keeps them in every AABB-overlapping cell — inherit that, do not special-case it away.
- **Synthetic fixtures only.** No real maps exist; `stress-warren` is a synthetic pressure probe. The
  measured reachable-fraction bounds the *mechanism*, not the shipping win — real-map validation is a
  separate gate before any production implementation. State this in the findings; do not project the
  fixture number onto real content.

## Non-goals

- Shipping the cull. The spike measures; a later implementation spec (if promoted) hardens and wires it.
- Any format, section, or runtime change. This is a compile-time bake-instrumentation experiment.
- Touching the direct / delta / animated-direct bakes — they already cull.
- A quality-settings or debug-slider surface.

## Acceptance criteria

Split by what they prove (per `context/lib/experimental_spikes.md`).

**Honesty gate** (automated — the experiment ran correctly):

- [ ] The instrumentation attributes each receiver to its correct cell (hit point for the indirect
  bake, texel world position for the lightmap) and reads the reaching-light set for *that* cell.
- [ ] Directional lights are counted as always-reaching (never filtered out).
- [ ] If the spike prototypes the actual cull (not required — see Deliverable), the bake output is
  byte-identical to the unculled baseline on the fixture (a culled light is provably zero-contribution;
  any drift means the cull is wrong).

**Measured finding** (measure-and-report — the number the spike exists to learn):

- [ ] On `stress-warren` at the shipping bake config, report the per-receiver reaching-light fraction
  (reaching-lights / total lights) for both cold bakes — distribution, not just a mean (min / median /
  p95 / max), so a few dense receivers do not hide a large tail.
- [ ] If the cull is prototyped, report wall-clock delta on both stages (baseline vs. culled) alongside
  the fraction. This is a recorded result feeding the recommendation, not a pass/fail threshold.

## Deliverable

A findings note: the reaching-light fraction distribution on `stress-warren` (and wall-clock delta if
the cull was prototyped), the correctness confirmation, and a recommendation — **promote to a full
implementation spec** (fraction small enough that culling is worth the hardening + real-map validation
gate), **defer** (fraction large on the synthetic map, so the win is unlikely to justify the work), or
**re-measure once a real map exists** (fixture is too unrepresentative to decide). The implementing
agent chooses whether counting alone answers the question or a prototype cull is needed to trust the
wall-clock; the question above is the contract, the method is theirs.
