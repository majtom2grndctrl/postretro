# Handoff — baked-light entity shadows & entity lighting (post-E10)

> **Status:** pre-spec thinking. Not a spec yet. This is a context handoff to a future you, capturing the *what* and the *why* of the plans discussed after `M10--dynamic-mesh-shadows`, in enough detail to reconstruct the reasoning without re-deriving it. Written second-person, you → future-you.
>
> **Origin:** a working session reviewing the E10 dynamic-mesh-shadows branch (`claude/dynamic-mesh-shadows-jvl96j`). The review and its fix-ups are committed. These are the *forward* plans the conversation surfaced.

## The mental model you arrived at

There are **two orthogonal capabilities** an entity (enemy mesh) can have with respect to a light, and E10 only built one of them:

1. **Entity → world shadow** (enemy throws a shadow on the floor) — **shipped by E10**, dynamic lights only.
2. **Light → entity illumination** (the light brightens the enemy's surface) — **not built.** Entities are lit *only* by baked SH probes today; a dynamic light casts the enemy's shadow but does **not** light the enemy (group 2 in the mesh shader is unallocated). This is the incoherence you kept pointing at: a shadow from a light that doesn't appear to light the creature.
3. **World/entity → entity shadow receipt** (geometry or other enemies shadow *onto* this enemy, crisply) — **not built.** Today entities get only the soft, probe-coarse, occlusion-tested SH-direct approximation of world→entity shadow.

The single invariant that governs everything below — **no double-count**: a light's direct contribution must live in *exactly one* place (summed lightmap **or** a separable runtime term). Only a light with a *separable runtime term* can be per-light shadowed at runtime, because you can only multiply a shadow factor onto a term you can still isolate. The summed lightmap has no such term. This is why E10 says baked lights "can't" cast entity shadows — and why that wording is an overstatement (see below).

## Plans to write, in dependency order

### 1. Dynamic mesh direct lighting (group 2) — ALREADY on the roadmap
- **Where:** `context/plans/roadmap.md:207` ("Dynamic mesh direct lighting").
- **What:** wire the Epic-5 flat per-fragment runtime-light loop into the skinned-mesh shader, filling the reserved group-2 slot (`render/mesh_pass.rs` group 2 = "SH ambient + dynamic direct"), so dynamic lights illuminate entities on top of their baked SH indirect.
- **Why it matters here:** it is the *prerequisite* for #2. Until group 2 exists there is no runtime per-light term on the entity to attenuate, so entities cannot *receive* crisp shadows. It also closes capability (2) above.

### 2. Dynamic mesh shadow receipt — ADDED to the roadmap this session
- **Where:** `context/plans/roadmap.md`, the bullet you inserted directly after the direct-lighting line (commit `95f1adf`).
- **What:** once group 2 opens the per-light term, attenuate it per dynamic light by sampling that light's existing shadow map — E10's spot 2D-array (`lighting/spot_shadow.rs`) and point cube-array (`lighting/cube_shadow.rs`) pools — but consumed from the **entity** shader side rather than only the world shader. Gives crisp **world→entity** and **entity→entity** shadows.
- **Why:** purely additive on top of group 2 + E10's pools; depends on group 2 (nothing to attenuate before then). This was the genuinely *unhomed* half — the illumination half already had `roadmap.md:207`; the receipt half did not, so you added it.

### 3. Baked-light entity-shadow casting — the big exploration (NOT yet a roadmap line or spec)

This is the substantial design conversation. The question: *can a **baked** light cast entity shadows, via a KVP, instead of forcing every entity-shadow author onto a dynamic light?*

**Verdict you reached:** E10's "physically impossible" framing is an **overstatement**. The precise, defensible claim is narrower: *"A `static_light_map` light, as stored, has no separable runtime term, so it can't be per-light shadowed without changing its storage."* The engine already ships a counterexample — the `sdf` shadow type is a fixed (baked-tier) light whose **direct term is excluded from the lightmap and evaluated as a separable runtime term × visibility factor** (`forward.wgsl` ~960-977; sdf lights filtered out of the lightmap set at `lightmap_bake.rs:340-346`). So separability for a fixed light is already a solved pattern.

Three candidate techniques emerged. Two are live; one is rejected.

#### Path A hybrid — separable runtime *direct* term (low-effort, budget-free)
- **What:** route the opted-in fixed light's **direct** term out of the frozen lightmap onto the **existing** separable runtime path (the `sdf` `spec_lights` term, or the animated-lightmap weight-map term), then multiply it by the entity-shadow factor sampled from E10's already-bound pools. Keep the light's **indirect/GI bounce baked** (SH + indirect lightmap).
- **Why valuable:** **zero new sampled textures** (reuses sdf term + already-bound E10 pools), near-zero new technique (composes two shipped subsystems). Still richer than a plain dynamic light because it contributes **baked indirect bounce** to the world and to entities (via SH). Authoring spelling that falls out naturally: `_shadow_type sdf` + `_cast_entity_shadows 1` on a fixed light.
- **Why it's limited (your initial skepticism, then the user's good reframe):** its defining move — making the *direct* term runtime-separable — is the essence of what makes a dynamic light dynamic. So on the *direct*-lighting axis it's "basically a dynamic light." You first said "probably drop it." The user reframed it correctly: as a **budget-free hybrid whose value is the baked indirect richness at ~zero cost**, it's a legitimate low-effort stepping stone, not a throwaway. Both framings are true — judge it as "dynamic light + baked indirect, free," not as "the fullest baked light."
- **Constraint:** must be a **fixed coordinate** — you're baking indirect around it; moving it needs a re-bake.

#### Shadowmask hybrid — baked *direct + indirect*, union with runtime entity shadow (highest fidelity)
- **This is the user's own proposal, and it is exactly Unity's Shadowmask mode.** Keep the light fully baked (direct + indirect, lightmap + SH, like a normal light today). Add a **separate per-light baked occlusion** term. At runtime combine the baked shadow and the entity shadow-map factor via **union** — `max(occlusion)` / `min(visibility)`, **not** a multiply — which is the canonical fix for double-darkening in the overlap region.
- **Why it's the only truly distinct option:** it's the one approach that gives **fully baked-quality DIRECT lighting** (GI, soft penumbrae) on the world *and* lights the entity (via baked SH) *and* a coherent entity shadow. That direct-lighting fidelity is the one thing a dynamic light fundamentally cannot produce.
- **The sampled-texture budget objection — and its resolution (important, don't lose this):** forward sits at ~13-14/16 sampled textures (Metal hard cap 16); a naive shadowmask is +1. But you confirmed a **reasonable** way to add it without growing — even *shrinking* — the binding count: **array consolidation.** Group 4 (lightmap) currently holds **4 sampled textures in 2 format families**:
  - Rgba16Float: `lightmap_irradiance` (b0) + `animated_lm_atlas` (b3)
  - Rgba8Unorm: `lightmap_direction` (b1) + `animated_lm_direction` (b5)

  A `texture_2d_array` is **one** binding regardless of layer count, and array layers only need matching format + dimensions. Consolidate each format pair into one array (4 textures → 2 array bindings, freeing ~2 slots), then add the occlusion mask as another **Rgba8Unorm layer for free**. (Verify-in-prototype: that all four atlases share dimensions — the code comments in `render/animated_lightmap.rs` ~204/1215 strongly imply they're created at identical dims.) **Bank this insight regardless of which hybrid ships — it's the general move that keeps any future lightmap-space data off the sampled-texture wall.**
- **The real remaining costs (none of them the texture budget):**
  1. The array-consolidation **refactor** — bake output layout, the PRL lightmap section, GPU upload, and shader sampling all move to array-indexed (`textureSampleLevel(arr, samp, uv, layer)`).
  2. The **directional-lightmap union correctness** — subtracting/unioning a per-light term against the bumped-Lambert directional reconstruction (`forward.wgsl` ~887-907) is fiddly. **This is the prototype gate** for the whole technique.
  3. The **4-shadowmask-lights-per-texel** channel limit (RGBA, graph-colored) — fine for fixed hero lights (<4 overlapping is the norm).

#### Subtractive — REJECTED
- Composite the entity shadow as a darkening multiply toward an ambient/shadow-color floor. Cheapest, no new data, but the classic **double-darkening** in overlap regions + shadow-color hand-tuning. The user explicitly wants the *union* (no double-darkening), so this is out.

### The decision frame you kept returning to (write this into any spec's rationale)

- **"Just use a dynamic light" is the baseline.** E10 already casts entity shadows from dynamic lights today.
- **The expensive part of *any* crisp entity shadow** — rendering the moving entity into a depth map from the light and sampling it — **is identical across all approaches.** So the hybrids are **fidelity plays, not performance wins.** A hybrid is *not* more computationally efficient than a dynamic spotlight; its only runtime saving (baked direct + no per-frame world-depth re-render) is small, and the world-depth re-render is *already* eliminable for a fixed dynamic light via E10 Task 7's static-depth cache.
- **So the hybrids earn their keep only for FIXED lights where you want baked-quality world lighting (indirect for Path A; direct+indirect for Shadowmask) AND the entity grounded.** The motivating scenario: a fixed hero/set-piece light (e.g. a ceiling lamp) where the room must read as fully baked and the same light should coherently light *and* shadow enemies under it. If you don't need that, a dynamic light wins on simplicity.

### Recommended sequencing for the baked-shadow work (two stages)

1. **Stage 1 — Path A hybrid.** Budget-free, reuses sdf term + E10 pools, no texture work. Proves the authoring model (`_shadow_type sdf` + `_cast_entity_shadows` on a fixed light) and the fixed-light entity-shadow plumbing.
2. **Stage 2 — Shadowmask hybrid + lightmap-array-consolidation.** The fidelity follow-on (baked *direct*). Do the array consolidation as part of it — it pays for itself in reclaimed binding slots. The union-vs-directional-lightmap correctness is the explicit prototype gate.

## Open questions to resolve before writing the spec(s)
(from the cited feasibility research; these gate scope)
1. Is the acceptable authoring model **"fixed light + `_shadow_type sdf` + `_cast_entity_shadows`"** (Path A), or do you specifically need a literal `static_light_map` light (direct fully baked) to also cast entity shadows (forces Shadowmask)?
2. How many fixed lights per map realistically want entity shadows? (<4 makes the shadowmask channel limit moot.)
3. Should the E10 "physically impossible" wording be **corrected** (it's an overstatement) or left as a deliberate simplification steering authors to dynamic lights?
4. What is the concrete scenario where a *fixed* light is required and a dynamic light won't do? This decides whether the baked path is worth building at all vs. documenting "use a dynamic light."

## Key code anchors (so future-you can re-find the reasoning)
- `crates/postretro/src/lighting/mod.rs:60` — `entity_occluder_eligible = casts_entity_shadows && is_dynamic` (the E10 gate).
- `crates/postretro/src/shaders/forward.wgsl:882` — `sample_lightmap_irradiance`, the single **summed** lightmap sample (the no-separable-term blocker).
- `forward.wgsl` ~960-977 — the `sdf` **separable runtime direct term × visibility** (Path A's reuse target and the live counterexample to "physically impossible").
- `forward.wgsl:197-220` — group 4 lightmap bindings + formats (the array-consolidation candidates).
- `forward.wgsl` ~887-907 — bumped-Lambert directional reconstruction (the Shadowmask union-correctness hazard).
- `crates/level-compiler/src/lightmap_bake.rs:340-346` — sdf lights excluded from the lightmap set (disjoint-set / no-double-count).
- `lightmap_bake.rs` ~810-826 (accumulate `+=`), ~919-955 (`light_texel_contribution` / per-light layer separability — proves a per-light occlusion mask is *bakeable*).
- `crates/postretro/src/render/animated_lightmap.rs` ~204/312/342/1215 — animated atlas formats (Rgba16Float irradiance, Rgba8Unorm direction) + the "same dimensions" comments.
- `crates/level-format/src/direct_sh_volume.rs` (id 35) — baked static-direct SH for dynamic objects: *how baked lights already light entities* (so a baked hybrid lights the enemy for free; a dynamic light does not, pre-group-2).
- E10 pools: `lighting/spot_shadow.rs`, `lighting/cube_shadow.rs`; the static-depth cache is E10 Task 7.
- Sampled-texture guard: `forward_pipeline_sampled_texture_request_matches_bgl_definitions` (the ~13-14/16 pin).

## Gotcha banked from the regression hunt (cube faces are entity-only)
The E10 cube point-shadow faces hold **entity occluders only — no world geometry baseline**. Consequence learned the hard way: a slot's occupied faces must be **cleared (to far = 1.0) every frame independent of whether any occluder is drawn** — otherwise an eligible point light with no skinned mesh in its PVS samples uninitialized cube depth (~0.0), which under `CompareFunction::Less` reads as fully shadowed and zeroes the light's world contribution (view-dependent, because cube slots are only assigned to PVS-visible lights). This is the same seam the deferred **world-self-shadow-under-point-lights** open question would later fill (adding a world-geometry baseline to the faces, like the spot path already has). When that work happens, the "always clear" invariant and the entity-only assumption are the things to revisit.

## Open threads NOT part of these plans (don't conflate)
- **Cleanup:** the E10 plan + FGD "physically impossible" wording should be softened to the precise claim (open question #3). `roadmap.md:206` still calls E10 the "12-light pixmap shadow pool" — stale (now 96-slot 2D-array + cube-array); offered to refresh, not done.
- **Unrelated regression (separate investigation):** scripted animated *point* lights (`arena_wave_2`, next to the fog/smoke room in `campaign-test.map`) — only the first in the sequence fires. Two deep traces exonerated the rendering/data path (byte-identical to `origin/main`); narrowed to runtime descriptor *upload* / sequence *dispatch* vs. GPU read. Pending a CPU descriptor-dump (log `period`/`phase`/`brightness_count` for `level_lights` indices 23-29) the user runs. This is a bug, not a plan — kept here only so you don't rediscover it cold.
