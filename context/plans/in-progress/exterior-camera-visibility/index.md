# Exterior Camera Visibility

> **Status:** ready
> **Depends on:** brush-volume BSP refactor (`context/plans/done/brush-volume-bsp/`) — exterior leaves must already be the structural signature this plan keys on.
> **Related:** `postretro/src/visibility.rs` · `postretro/src/render.rs` · `context/lib/build_pipeline.md` §Runtime visibility

---

## Goal

When the camera leaves the playable interior of a level (noclip, teleport, debug fly-through), the level interior remains visible from outside. Visibility rules **inside** the level are unchanged: portal traversal still seeds from the camera leaf and produces the same per-frame face set it does today.

---

## Background

PRL portal traversal seeds from the camera leaf and walks the connected component of the portal graph it can reach. A sealed level produces two disconnected components: **interior** (the playable empty space) and **exterior** (the void surrounding the level, flood-filled from a probe outside the world AABB). The compiler strips face data from exterior leaves so they contribute zero to the packed geometry; the leaves themselves remain in the BSP tree because point-in-leaf queries still need to land on them.

Together these mean three runtime cases for the static-world path:

| Camera location | Today's behavior | Correct? |
|---|---|---|
| Interior empty leaf | Portal walk reaches interior leaves; faces drawn. | Yes — the design. |
| Solid leaf (clipped through geometry) | Existing `SolidLeafFallback` branch draws every non-solid leaf with frustum cull. | Yes. |
| Exterior empty leaf | Portal walk reaches only exterior leaves; every reached leaf has `face_count == 0`; nothing draws. | **No** — the entire level vanishes. |

The third case is structurally identical to the second: the camera is somewhere with no useful local visibility data and the right answer is a frustum-only fallback over the level interior. It has one extra wrinkle: from outside, the inward-facing surfaces present their **back** side to the camera, and the rasterizer drops them under the current `cull_mode: Some(Face::Back)` setting on the static-world pipeline (`postretro/src/render.rs:567`).

---

## Approach

Two complementary changes. Both small, both file-isolated.

### A. Exterior camera fallback in the visibility module

`postretro/src/visibility.rs::determine_prl_visibility` already has the shape: a `SolidLeafFallback` branch (lines 544–583) that detects "camera in solid leaf" and switches to a frustum-only loop over every non-solid non-zero-face leaf. Add a sibling branch for the exterior case.

Detection signature: `!camera_leaf.is_solid && camera_leaf.face_count == 0`. This is the structural fingerprint left by the compiler's exterior strip — an empty leaf with no faces. No new metadata needs to ride alongside the BspLeaves section; the existing `face_count == 0` is the marker.

The branch body is a near-clone of `SolidLeafFallback`: clear scratch, iterate `world.leaves`, skip solid and zero-face entries, AABB-frustum-cull each surviving leaf, push its faces. Tag the result with a new `VisibilityPath::ExteriorCameraFallback` so the title bar and the `[Diagnostics]` per-frame log line distinguish it from solid-leaf and from normal portal traversal.

The two fallback branches are similar enough that a shared helper is tempting; resist that until both branches exist. Premature merging hides the difference in *why* each is taken — solid is "shouldn't be here, draw something so the level isn't a black void," exterior is "you've left the playable space deliberately, render the interior X-ray."

### B. Disable back-face culling on the static-world pipeline

Change `cull_mode` at `postretro/src/render.rs:567` from `Some(Face::Back)` to `None`. This is the `Textured Pipeline` at `render.rs:532`, used for all static-world draws at `render.rs:831`.

**BSP note.** The `Textured Pipeline` is also the pipeline the legacy `.bsp` loader draws through. The invariance argument below generalizes to any sealed level — BSP or PRL — so `cull_mode: None` is pixel-neutral from inside a BSP level too. The repo currently contains no `.bsp` assets (all maps are `.prl` compiled from `.map` sources), so empirical BSP verification is not part of Task 3; if a `.bsp` asset is ever loaded later, the invariance argument is what keeps the inside-the-level render correct.

**Inside-the-level invariance argument.** Every face emitted into an interior empty leaf has its outward normal pointing into that leaf's empty space — that is the structural contract of brush-side projection (`postretro-level-compiler/src/partition/face_extract.rs`). Pass 1 (`ClipSideByTree_r`) only accumulates a side's fragments into leaves on the side's front-facing half-space, so the polygon's outward normal always points toward the leaf interior it ends up in. The camera, when interior, sits in an empty leaf, and every face it sees from there is front-facing by construction. Removing back-face culling changes zero pixels rendered from any interior position in any sealed level.

The GPU cost of `cull_mode: None` is the per-vertex work the rasterizer would otherwise skip — measurable only when back-face culling drops a meaningful fraction of submitted geometry. From an interior camera in a sealed level, today's pipeline drops zero back faces, so removing the cull is GPU-free in that case. From an exterior camera, the rasterizer now processes both faces of every interior wall — a constant factor of two on the visible-from-outside set, which is the entire point of the change.

If a future change introduces inward-visible back faces (one-sided decals authored facing the wrong way, additive overlays, brush sides escaping containment dedup), the new visible artifacts would surface immediately from inside the level. That is an authoring bug worth catching, not a cost worth paying.

### Diagnostic loss

Today, "the entire level disappeared" is an unambiguous signal that the camera escaped the playable region. After this plan, the level keeps rendering and the user has to read the title bar / log to know they're outside. Two replacements absorb the lost signal:

- **Title bar tag.** The `path:` segment in the window title is driven by an exhaustive match on `VisibilityPath` at `postretro/src/main.rs:553-560`. The new variant must be added to that match in the same change — Task 1 covers it. The new label string is `exterior`.
- **Log line.** The per-frame diagnostic emit at `main.rs:565` is `log::debug!` under the `[Diagnostics]` tag and already names the path label. Once the match arm is added it will show `path:exterior` per frame at debug level. No new emit, no info-level log on entry. A camera leaving the playable region is a valid runtime case — spectators and debug fly-throughs do it routinely — not a warning or an error, and the existing debug line is the right visibility level for it.

These cost nothing and replace the implicit "screen is black" diagnostic with an explicit one at debug level.

---

## Out of scope

- Changing the portal traversal algorithm or cycle-prevention rules.
- Changing how exterior leaves are detected at compile time. The structural `face_count == 0 && !is_solid` signature must keep matching the compiler's strip.
- Re-introducing exterior face data into the PRL output. The strip stays.
- A separate "draw distance" cull. The frustum AABB cull on the fallback path already limits work to the on-screen subset; distance is implicit in frustum extent.
- Two-pipeline render with cull-mode toggle per frame. One pipeline change is the simpler answer and the inside-the-level invariance argument removes the reason to keep two.
- Indicator UI in the viewport (border tint, watermark) when the fallback path is active. Title bar is sufficient for the developers using this; viewport indicators are a UX concern that belongs in a player-facing diagnostics plan, not here.

---

## Tasks

### Task 1: Exterior camera fallback branch

**Crates:** `postretro` · **Files:** `src/visibility.rs`, `src/main.rs`

Add `ExteriorCameraFallback` to the `VisibilityPath` enum. In `determine_prl_visibility`, after the existing `in_solid` branch and before the `has_portals` branch, add a new branch keyed on `!camera_leaf.is_solid && camera_leaf.face_count == 0`. Body: clear scratch, frustum-cull every non-solid non-zero-face leaf, push faces, return `VisibleFaces::Culled` with a `VisibilityStats` carrying the new path tag. Do *not* emit a new log line on entry — a camera leaving the playable region is a valid runtime case (spectators, debug fly), not a warning. The existing `[Diagnostics]` debug emit at `main.rs:565` already names the path label per frame and is sufficient.

Then extend the exhaustive match on `VisibilityPath` at `postretro/src/main.rs:553-560`: add the arm `VisibilityPath::ExteriorCameraFallback => "exterior",`. Without this edit the crate will not compile — the match has no `_` arm.

**Acceptance criteria:**
- Unit test **A** — *entry detection*: constructs a `LevelWorld` with one interior leaf (faces, inside the frustum) and one exterior leaf (no faces, not solid), positions the camera in the exterior leaf, asserts `determine_prl_visibility` returns `Culled` with non-zero `drawn_faces` and `path == VisibilityPath::ExteriorCameraFallback`.
- Unit test **B** — *frustum cull on fallback*: same world shape plus a second interior leaf placed outside the view frustum; asserts only the in-frustum leaf's faces appear in the draw range list. This test is what distinguishes this branch from a "draw everything non-solid" fallback and mirrors the existing `SolidLeafFallback` test pattern.
- Unit test **C** — *interior camera invariance*: an interior camera in the same world returns `VisibilityPath::PrlPortal { .. }`, not the new variant. Guards against detection predicate drift.
- Existing PRL portal tests still pass with no changes.
- `VisibilityStats::pvs_reach` on the new path reports the same baseline shape as `SolidLeafFallback` (use `total_faces`, since neither path consults a PVS bitset).
- `cargo build -p postretro` succeeds, proving the `main.rs` match arm was added.

### Task 2: Static-world pipeline cull mode

**Crate:** `postretro` · **File:** `src/render.rs`

Change `cull_mode: Some(wgpu::Face::Back)` at line 567 to `cull_mode: None`. Add an inline comment that captures the invariance argument so a future reader editing this line sees what it costs. The comment must not link this plan file — it will rot once the plan moves to `done/`. Suggested wording: `// cull_mode: None — exterior cameras render interior walls from their back side; interior cameras see only front faces by brush-side construction, so this change is pixel-neutral from inside.`

**Acceptance criteria:**
- `cargo build -p postretro` succeeds.
- No test changes required — the change is a pipeline-state tweak with no observable effect from inside a sealed level.
- Manual: walk around the inside of `assets/maps/test-3.prl` and confirm visual parity with the pre-change render (baseline screenshot kept side-by-side during review).

### Task 3: Manual verification

Compile and load `test-3.prl`. Walk inside the level — verify visual parity with current behavior (no missing or duplicated surfaces, no z-fighting introduced by the cull change). Use the engine's noclip / fly path to step outside the level boundary — verify the interior remains visible from outside, with inward-facing surfaces visible as their back sides. Also view the level from several outside angles to check for z-fighting on thin walls or shared-plane brush sides, which is where the disabled back-face cull is most likely to surface a new artifact. Confirm the title bar shows `path:exterior` while outside and reverts to `path:prl-portal` on re-entry.

**Acceptance criteria:**
- Inside-the-level rendering matches the pre-change baseline.
- Outside-the-level rendering shows interior geometry instead of an empty void.
- Title bar reflects the path transition on entry/exit.
- No new z-fighting or duplicated-surface artifacts from inside or outside angles.

### Task 4: Documentation update

**File:** `context/lib/build_pipeline.md` §Runtime visibility

Add a sentence to the runtime visibility table noting that the runtime falls back to a frustum-only pass when the camera leaves the interior portal component. One line — this is a runtime branch documented at the table that already enumerates runtime paths, not a new section.

**Acceptance criteria:**
- The runtime visibility table or its surrounding prose names the exterior fallback as one of the runtime paths.
- No function names appear in the durable doc.

---

## Sequencing

Task 1 must land before Task 2. Task 2 alone disables back-face culling without adding the exterior detection that motivates it: the change costs a (small) amount of rasterizer work and delivers zero behavioral benefit until Task 1 is in place, and if a regression later breaks the exterior-leaf signature, the implicit "screen goes black" diagnostic is already gone. Land Task 1 first, Task 2 second, then Task 3 once both are in. Task 4 can run in parallel with Task 3.

```
Task 1 (visibility.rs + main.rs match) ──► Task 2 (render.rs cull) ──► Task 3 (manual verification)
Task 4 (docs) ───────────────────────────────────────────────────── parallel
```

---

## Risks and Mitigations

| Risk | Mitigation |
|---|---|
| Disabling back-face cull surfaces a previously-hidden authoring bug (e.g., a brush face emitted with the wrong orientation). | Manual verification in Task 3 covers this — any new visible artifact from inside the level is an immediate failure that blocks the change. The fix is to repair the orientation upstream, not to re-enable culling. |
| `face_count == 0 && !is_solid` matches a leaf that isn't structurally exterior — e.g., an interior leaf with no faces because all of its bounding brushes share planes with adjacent leaves. | Inspect the compiler output for any test map: count leaves matching the signature, confirm they correspond to exterior or genuinely-empty regions. If a false positive exists, the fallback still does the right thing for correctness — it draws the level interior with frustum culling. The concrete perf cost is bounded: every frame the camera spends inside the false-positive leaf runs an `O(world.leaves)` loop instead of portal traversal, the same per-frame cost `SolidLeafFallback` already pays and is already deemed acceptable. On a sealed test map today the compiler does not emit such leaves (exterior strip at `postretro-level-compiler/src/visibility/mod.rs:256-269` is the only producer of `face_count == 0 && !is_solid`), so this is a "watch for future regressions," not a known defect. |
| The frustum-only loop allocates per frame, regressing the zero-allocation contract `App::scratch_ranges` enforces for the portal path. | The new branch reuses the same `scratch: &mut Vec<DrawRange>` parameter and the same `scratch.clear()` + `push` pattern as `SolidLeafFallback`. Capacity is reclaimed by main.rs on the next frame the same way. |
| Future renderer work re-enables back-face culling without re-checking this plan's invariance argument. | The render.rs comment added in Task 2 names the exterior-camera dependency explicitly so a reader who edits the line sees what it costs. |

---

## Acceptance Criteria

The plan is done when all of the following hold:

1. `determine_prl_visibility` has an `ExteriorCameraFallback` branch and a corresponding `VisibilityPath` variant, and the exhaustive `main.rs` path-label match has a new arm for it.
2. The `Textured Pipeline` at `render.rs:532` uses `cull_mode: None`.
3. From inside `test-3.prl`, the rendered image is visually identical to the pre-change baseline.
4. From outside `test-3.prl` (noclip / fly), the level interior is visible.
5. The title bar `path:` segment shows `exterior` while outside the level and `prl-portal` on re-entry.
6. `cargo test --workspace` passes. The three new unit tests from Task 1 (entry detection, frustum cull on fallback, interior camera invariance) are included.
7. `context/lib/build_pipeline.md` §Runtime visibility names the exterior fallback path.

---

## Open Questions

- **Should the in-solid and exterior branches share a helper after both exist?** Defer to post-implementation. The two branches arose for different reasons and merging them now would erase the distinction in commit history. If a third frustum-only fallback shows up, that's the right time to refactor.
