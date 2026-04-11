# Exterior Camera Visibility

> **Status:** draft
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

`postretro/src/visibility.rs::determine_prl_visibility` already has the shape: a `SolidLeafFallback` branch (lines ~538–582) that detects "camera in solid leaf" and switches to a frustum-only loop over every non-solid non-zero-face leaf. Add a sibling branch for the exterior case.

Detection signature: `!camera_leaf.is_solid && camera_leaf.face_count == 0`. This is the structural fingerprint left by the compiler's exterior strip — an empty leaf with no faces. No new metadata needs to ride alongside the BspLeaves section; the existing `face_count == 0` is the marker.

The branch body is a near-clone of `SolidLeafFallback`: clear scratch, iterate `world.leaves`, skip solid and zero-face entries, AABB-frustum-cull each surviving leaf, push its faces. Tag the result with a new `VisibilityPath::ExteriorCameraFallback` so the title bar and `[Visibility]` log line distinguish it from solid-leaf and from normal portal traversal.

The two fallback branches are similar enough that a shared helper is tempting; resist that until both branches exist. Premature merging hides the difference in *why* each is taken — solid is "shouldn't be here, draw something so the level isn't a black void," exterior is "you've left the playable space deliberately, render the interior X-ray."

### B. Disable back-face culling on the static-world pipeline

Change `cull_mode` at `postretro/src/render.rs:567` from `Some(Face::Back)` to `None`.

**Inside-the-level invariance argument.** Every face emitted into an interior empty leaf has its outward normal pointing into that leaf's empty space — that is the structural contract of brush-side projection (`postretro-level-compiler/src/partition/face_extract.rs`). Pass 1 (`ClipSideByTree_r`) only accumulates a side's fragments into leaves on the side's front-facing half-space, so the polygon's outward normal always points toward the leaf interior it ends up in. The camera, when interior, sits in an empty leaf, and every face it sees from there is front-facing by construction. Removing back-face culling changes zero pixels rendered from any interior position in any sealed level.

The GPU cost of `cull_mode: None` is the per-vertex work the rasterizer would otherwise skip — measurable only when back-face culling drops a meaningful fraction of submitted geometry. From an interior camera in a sealed level, today's pipeline drops zero back faces, so removing the cull is GPU-free in that case. From an exterior camera, the rasterizer now processes both faces of every interior wall — a constant factor of two on the visible-from-outside set, which is the entire point of the change.

If a future change introduces inward-visible back faces (one-sided decals authored facing the wrong way, additive overlays, brush sides escaping containment dedup), the new visible artifacts would surface immediately from inside the level. That is an authoring bug worth catching, not a cost worth paying.

### Diagnostic loss

Today, "the entire level disappeared" is an unambiguous signal that the camera escaped the playable region. After this plan, the level keeps rendering and the user has to read the title bar / log to know they're outside. Two replacements absorb the lost signal:

- **Title bar tag.** The `path:` segment in the window title (already populated from `VisibilityPath`) gains an `ExteriorCameraFallback` value.
- **Log line.** The existing `[Visibility]` info-level emit names the path; an explicit `log::info!` on transition into and out of the new branch makes the boundary visible.

These cost nothing and replace the implicit "screen is black" diagnostic with an explicit one.

---

## Out of scope

- Changing the portal traversal algorithm or cycle-prevention rules.
- Changing how exterior leaves are detected at compile time. The structural `face_count == 0 && !is_solid` signature must keep matching the compiler's strip.
- Re-introducing exterior face data into the PRL output. The strip stays.
- A separate "draw distance" cull. The frustum AABB cull on the fallback path already limits work to the on-screen subset; distance is implicit in frustum extent.
- Two-pipeline render with cull-mode toggle per frame. One pipeline change is the simpler answer and the inside-the-level invariance argument removes the reason to keep two.
- BSP path (`.bsp` legacy loader). Has its own visibility flow; if this plan lands first, the BSP path is unchanged and may exhibit the same vanishing-when-outside symptom — separate follow-up if observed.
- Indicator UI in the viewport (border tint, watermark) when the fallback path is active. Title bar is sufficient for the developers using this; viewport indicators are a UX concern that belongs in a player-facing diagnostics plan, not here.

---

## Tasks

### Task 1: Exterior camera fallback branch

**Crate:** `postretro` · **File:** `src/visibility.rs`

Add `ExteriorCameraFallback` to the `VisibilityPath` enum. In `determine_prl_visibility`, after the existing `in_solid` branch and before the `has_portals` branch, add a new branch keyed on `!camera_leaf.is_solid && camera_leaf.face_count == 0`. Body: frustum-cull every non-solid non-zero-face leaf, push faces into scratch, return `VisibleFaces::Culled` with a `VisibilityStats` carrying the new path tag.

**Acceptance criteria:**
- A unit test constructs a `LevelWorld` with one interior leaf (faces) and one exterior leaf (no faces, not solid), positions the camera in the exterior leaf, and asserts `determine_prl_visibility` returns `Culled` with non-zero `drawn_faces` and `path == VisibilityPath::ExteriorCameraFallback`.
- Existing PRL portal tests still pass with no changes — interior cameras never enter the new branch.
- `VisibilityStats::pvs_reach` on the new path reports the same baseline shape as `SolidLeafFallback` (use `total_faces`, since neither path consults a PVS bitset).

### Task 2: Static-world pipeline cull mode

**Crate:** `postretro` · **File:** `src/render.rs`

Change `cull_mode: Some(wgpu::Face::Back)` at line 567 to `cull_mode: None`. Add a one-line comment naming the exterior-camera reason and pointing at this plan's rationale.

**Acceptance criteria:**
- `cargo build -p postretro` succeeds.
- No test changes required — the change is a pipeline-state tweak with no observable effect from inside a sealed level.
- Manual: walk around the inside of `assets/maps/test-3.prl` and confirm visual parity with the pre-change render (baseline screenshot kept side-by-side during review).

### Task 3: Manual verification

Compile and load `test-3.prl`. Walk inside the level — verify visual parity with current behavior (no missing or duplicated surfaces, no z-fighting introduced by the cull change). Use the engine's noclip / fly path to step outside the level boundary — verify the interior remains visible from outside, with inward-facing surfaces visible as their back sides. Confirm the title bar shows `path:ExteriorCameraFallback` while outside and reverts to `path:PrlPortal` on re-entry.

**Acceptance criteria:**
- Inside-the-level rendering matches the pre-change baseline.
- Outside-the-level rendering shows interior geometry instead of an empty void.
- Title bar reflects the path transition on entry/exit.

### Task 4: Documentation update

**File:** `context/lib/build_pipeline.md` §Runtime visibility

Add a sentence to the runtime visibility table noting that the runtime falls back to a frustum-only pass when the camera leaves the interior portal component. One line — this is a runtime branch documented at the table that already enumerates runtime paths, not a new section.

**Acceptance criteria:**
- The runtime visibility table or its surrounding prose names the exterior fallback as one of the runtime paths.
- No function names appear in the durable doc.

---

## Sequencing

Tasks 1 and 2 are independent and can ship in either order. Task 3 needs both to land. Task 4 can run in parallel with Task 3.

```
Task 1 (visibility.rs)  ─┐
                         ├── Task 3 (manual verification)
Task 2 (render.rs)      ─┘
Task 4 (docs)            ────────────── parallel
```

---

## Risks and Mitigations

| Risk | Mitigation |
|---|---|
| Disabling back-face cull surfaces a previously-hidden authoring bug (e.g., a brush face emitted with the wrong orientation). | Manual verification in Task 3 covers this — any new visible artifact from inside the level is an immediate failure that blocks the change. The fix is to repair the orientation upstream, not to re-enable culling. |
| `face_count == 0 && !is_solid` matches a leaf that isn't structurally exterior — e.g., an interior leaf with no faces because all of its bounding brushes share planes with adjacent leaves. | Inspect the compiler output for any test map: count leaves matching the signature, confirm they correspond to exterior or genuinely-empty regions. If a false positive exists, the fallback still does the right thing — it draws the level interior with frustum culling — so the worst case is "an interior leaf treats itself as exterior and draws more than it would have." That is a perf concern, not a correctness one. |
| The frustum-only loop allocates per frame, regressing the zero-allocation contract `App::scratch_ranges` enforces for the portal path. | The new branch reuses the same `scratch: &mut Vec<DrawRange>` parameter and the same `scratch.clear()` + `push` pattern as `SolidLeafFallback`. Capacity is reclaimed by main.rs on the next frame the same way. |
| Future renderer work re-enables back-face culling without re-checking this plan's invariance argument. | The render.rs comment added in Task 2 names the exterior-camera dependency explicitly so a reader who edits the line sees what it costs. |

---

## Acceptance Criteria

The plan is done when all of the following hold:

1. `determine_prl_visibility` has an `ExteriorCameraFallback` branch and a corresponding `VisibilityPath` variant.
2. The static-world pipeline at `render.rs:567` uses `cull_mode: None`.
3. From inside `test-3.prl`, the rendered image is visually identical to the pre-change baseline.
4. From outside `test-3.prl` (noclip / fly), the level interior is visible.
5. The title bar `path:` segment shows `ExteriorCameraFallback` while outside the level.
6. `cargo test --workspace` passes. New unit test for the fallback branch is included.
7. `context/lib/build_pipeline.md` §Runtime visibility names the exterior fallback path.

---

## Open Questions

- **Should the in-solid and exterior branches share a helper after both exist?** Defer to post-implementation. The two branches arose for different reasons and merging them now would erase the distinction in commit history. If a third frustum-only fallback shows up, that's the right time to refactor.
- **Does the BSP legacy loader need the same treatment?** Probably yes for consistency, but the BSP path's leaf-flag layout is different and any change there should be its own plan once the symptom is observed on a BSP map.
