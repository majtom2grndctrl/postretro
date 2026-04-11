# Phase 4 — FGD Light Entities

> **Status:** draft — unblocked; drafting in parallel with probe format research.
> **Phase:** 4 (Light Probes) — see `context/plans/roadmap.md`
> **Related:** `context/lib/build_pipeline.md` §Custom FGD · `context/lib/entity_model.md` · ericw-tools light entity reference · dmap light entity reference

---

## Goal

Define the Postretro light entity set in the custom FGD so mappers can place light sources in TrenchBroom. `prl-build` reads the entities out of `.map` files and exposes them as a typed in-memory list for downstream compiler stages to consume. Without this plan, the probe baker has nothing to bake from.

---

## Context

The current FGD defines `env_fog_volume`, `env_cubemap`, and `env_reverb_zone`. No `light`, `light_spot`, or `light_sun` entity exists — the project has been runnable without one because Phase 3 shipped flat uniform lighting with no baked light sources.

Phase 4 changes that. The probe baker evaluates lighting per probe sample point, which requires a set of light sources in the map. Light entity definitions live on the mapper side, so they land in the FGD and are parsed at compile time through the existing shambler pipeline. TrenchBroom already understands standard FGD light entity shapes; the work is deciding which shapes Postretro supports and which properties each carries.

ericw-tools and dmap are the reference points for entity shape. Postretro is free to diverge where the retro aesthetic or engine constraints warrant.

---

## Scope

### In scope

- Define the light entity set: at minimum `light` (omnidirectional point), `light_spot` (cone), and `light_sun` (directional). Final set determined during refinement.
- Property definitions per entity: color, intensity, range / falloff, cone angles (spot), direction vector (sun), optional style / animation hooks.
- FGD source edits adding the new entities with TrenchBroom-compatible metadata (bounding box, editor colors, icons where applicable).
- Compiler-side entity parsing: `prl-build` recognizes the new classnames during `.map` parse and produces a typed light-source list for downstream stages. No downstream stage consumes it yet — the baker plan picks up from here.
- Documentation edit to `context/lib/build_pipeline.md` §Custom FGD so the entity table reflects the new rows.

### Out of scope

- Probe baking itself. This plan produces light sources; the baker plan consumes them.
- Runtime rendering of lights as dynamic sources. Phase 4 is baked-only. Dynamic light entities are Phase 5.
- Light styles and animation at runtime (flickering neon, pulsing signs). If the FGD carries a style field, it is stored in the parsed representation but not evaluated yet.
- `env_projector` and texture-projecting lights. Out of scope for the validation experiment.
- IES profiles or real-world photometric data. The retro aesthetic doesn't need them.
- Area lights (rectangle, disk). Point / spot / sun cover the decision-gate evaluation.

---

## Key decisions to make during refinement

- Exact entity set — is `light_sun` needed for the decision gate, or are point and spot enough for the test maps?
- Falloff model — inverse-square, linear, or authored per-light? ericw-tools and Source disagree here; pick one and commit.
- Property naming — `_color` (ericw-tools convention) vs. a Postretro-native property name. Mapper ergonomics vs. freedom to diverge.
- Where the parsed light list lives in the compiler — a new module under `postretro-level-compiler/src/`, or reuse of an existing entity-parsing path.
- Whether entity validation (missing required properties, out-of-range values) errors at compile time or warns and uses defaults.

---

## Acceptance criteria

- FGD defines the agreed light entity set with TrenchBroom-compatible property metadata.
- TrenchBroom displays the entities, lets the mapper place them, and edits their properties.
- `prl-build` parses light entities out of a `.map` file and produces a typed in-memory list available to downstream compiler stages.
- `context/lib/build_pipeline.md` §Custom FGD reflects the new entity rows.
- At least one test `.map` contains each entity type and compiles without error.
- No runtime engine changes. This plan stops at the compiler's parsed representation.

---

## Open questions

- Exact entity set (see Key decisions).
- Falloff model (see Key decisions).
- Property naming — ericw-tools compatibility vs. Postretro-native.
