# glTF Skeleton + Clip Loading

> **Status:** draft.
> **Milestone:** 10 (Animated Enemies) — render foundation track. Follows the shipped *Mesh render pass*, *Dynamic mesh shadows / direct lighting / shadow receipt*.
> **Related:** `context/lib/rendering_pipeline.md` §9 · `context/plans/done/M10--gltf-mesh-loading/` · `context/plans/done/M10--model-pipeline-slice/findings.md` · sibling plan `M10--skinned-animation-runtime` (the consumer).

## Goal

Finish the roadmap's "glTF skeleton + clip loading" bullet. The loader already reads the complete joint hierarchy (topologically sorted), every inverse-bind matrix, and **all** animation clips — but the renderer cache keeps only the first clip, interpolation modes are discarded (LINEAR hardcoded), and nothing exercises a multi-clip asset. This plan makes the full clip set survive to runtime, queryable by name, with correct STEP handling — the data surface the *Skinned animation runtime* plan consumes.

## Scope

### In scope

- **Interpolation-mode capture.** Each keyframe track records its glTF sampler interpolation (LINEAR or STEP). The sampler honors STEP: a stepped channel holds each keyframe's value until the next keyframe time, no interpolation.
- **CUBICSPLINE degrade.** A cubic-spline channel loads as LINEAR with a load-time warning naming the clip and channel. The loader extracts the *value* element of each `[in-tangent, value, out-tangent]` output triple — keyframe values play, tangent data is discarded, never misread as values.
- **All clips through the renderer cache.** The model cache retains every loaded clip in glTF order (today it truncates to the first). Default draw behavior is unchanged: an entity with no animation state plays the first clip, looped, at its per-instance phase offset.
- **Clip lookup surface.** The cache answers, per model handle: clip name → clip, and the full clip metadata list (name, duration, index order). This is the seam the animation runtime plan's level-load wiring consumes; tests consume it now.
- **Multi-clip coverage.** A small hand-authored multi-clip glTF test fixture (no external `.bin` — data-URI buffer) plus unit tests for all of the above.

### Out of scope

- **Clip selection, state machine, blending, time-slicing** — sibling plan *M10--skinned-animation-runtime*.
- **True cubic-spline evaluation.** Degrade-to-LINEAR is the decision, not a stopgap; revisit only against a real asset need.
- **Morph targets, multiple skins, multi-mesh documents, embedded/`.glb` buffers** — existing loader non-goals, unchanged.
- **Game-side clip-metadata table.** The runtime plan owns it (it is that plan's consumer); this plan only exposes the renderer-side query.

## Acceptance criteria

- [ ] Loading a glTF with more than one animation yields all clips: each is retrievable by its authored name and reports its own duration (multi-clip fixture; the single-clip dev asset still loads exactly one).
- [ ] Looking up a clip name absent from a model returns nothing — no error, no panic.
- [ ] A STEP-interpolated channel sampled between two keyframes returns the earlier keyframe's value exactly; at and after a keyframe time it returns that keyframe's value. LINEAR channels still interpolate.
- [ ] A CUBICSPLINE channel loads with a load-time warning, and sampling it returns the keyframe *values* (a test pins a known value that differs from the adjacent tangent elements).
- [ ] A cubic channel whose output count is not 3× its keyframe count is skipped with a warning; the rest of the clip loads.
- [ ] With no animation state system present, render behavior is unchanged: the cache's first clip drives the palette, looped at the per-instance phase (the dev asset renders identically — manual-visual).
- [ ] `cargo test -p postretro` passes; `cargo clippy -p postretro -- -D warnings` clean.

## Tasks

### Task 1: Track interpolation mode + STEP sampling

Add a per-track interpolation mode to the model module's track type (the doc comment on `Track` in `model/skeleton.rs` already reserves this) and honor it in `model/anim.rs` sampling: STEP uses the lower keyframe of the located span with no fraction. In `model/gltf_loader.rs` `load_clip`, read each channel's sampler interpolation; map LINEAR and STEP directly; for CUBICSPLINE, warn (clip + channel kind), extract `values[3k + 1]` per keyframe, and store the track as LINEAR. Guard the cubic triple shape: if outputs ≠ 3 × inputs, warn and skip the channel (its joint holds rest pose, the existing absent-channel behavior). Unit tests on synthetic tracks for STEP hold semantics and the cubic value extraction.

### Task 2: All clips through the renderer cache + name lookup

Stop truncating clips at the cache boundary. `render/mod.rs` `load_skinned_model` currently keeps `clips.into_iter().next()`; pass the whole `Vec<AnimationClip>` through `MeshPass::insert_model` into `UploadedModel` (field `clip: Option<AnimationClip>` becomes the clip list). `plan_and_upload`'s default sampling path uses the first clip — behavior-preserving. Add cache queries on `MeshPass`: clip-by-name for a handle (first match wins on duplicate names — documented), and the metadata list (name + duration, glTF order). Callers today: tests and the existing first-clip default; the animation runtime plan is the named external consumer. Update the load-time log to name all parsed clips.

### Task 3: Multi-clip fixture + integration tests

Author a minimal multi-clip glTF fixture inside the crate (e.g. two joints; two named clips with distinct durations; one STEP channel; one CUBICSPLINE channel) using a base64 data-URI buffer so no sidecar `.bin` ships — `gltf::import_buffers` already resolves data URIs through the existing decode-free entry. Tests drive the full path: `load_model` over the fixture asserts clip count, names, durations, STEP/cubic behavior per the ACs; a cache-level test asserts lookup-by-name and the metadata list against an inserted model. The existing real-asset test (one clip, 26 joints) stays as-is.

## Sequencing

**Phase 1 (concurrent):** Task 1 (model module), Task 2 (renderer cache) — disjoint layers.
**Phase 2 (sequential):** Task 3 — its tests consume Task 1's modes and Task 2's queries.

## Rough sketch

- `model/skeleton.rs`: `Track<T>` gains a mode enum (`Linear` default, `Step`); `Default` keeps existing synthetic-test construction compiling.
- `model/anim.rs`: `sample_vec3_track` / `sample_quat_track` branch on mode after `locate_span` — STEP returns the `i0` value. `sample_clip`'s signature is unchanged.
- `model/gltf_loader.rs` `load_clip`: `channel.sampler().interpolation()` (`gltf::animation::Interpolation`). Cubic extraction applies to all three channel kinds (translation/rotation/scale).
- `render/mesh_pass.rs`: `UploadedModel.clip` → clip list; `insert_model` signature change (single caller: `load_skinned_model`); `plan_and_upload` reads `clips.first()`.
- Fixture lives under the `postretro` crate, path-resolved via `CARGO_MANIFEST_DIR` like the existing real-model test.

## Open questions

- Duplicate clip names in one document: first-match-wins is the proposed rule; flag at review if a warning on duplicates is wanted.
- Whether the STEP and CUBICSPLINE test channels share the multi-clip fixture or get a sibling fixture file — implementer's choice, both satisfy the ACs.
