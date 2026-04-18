# Map Leak Diagnostics

> **Status:** draft — design sketch; refine before promoting to `ready/`.
> **Related:** `context/plans/done/exterior-leaf-cull/` (flood-fill that already detects leaks but only warns) · `context/plans/in-progress/lighting-foundation/8-sdf-shadows.md` (SDF baker parity test depends on watertight geometry).
> **Motivating issue:** The SDF shadow baker ([sdf_bake.rs](../../../../postretro-level-compiler/src/sdf_bake.rs)) classifies empty bricks as interior/exterior via a +Y parity ray from the brick center. Non-watertight geometry breaks parity, producing wrong-sign SDF values and garbage shadows. Rather than try to recover robustly at bake time (expensive, approximate), the compiler should fail fast with helpful diagnostics so the modder fixes the leak.

---

## Goal

Promote map leaks from a soft warning to a **hard compilation error with pointfile output**, matching the Quake/ericw-tools modder UX. A leak means "the interior flood-fill reached the void" — the level shell has a hole. Leaks break PVS, indirect lighting, and SDF shadow baking. They must be fixed in the source map, not worked around in the compiler.

**Modder UX:**
- `prl-build` exits non-zero with a clear error message naming the leak.
- A `.pts` pointfile is written alongside the input `.map`, containing the polyline from an interior entity to the void.
- TrenchBroom can load the `.pts` file to visualize the leak path directly in the editor.

---

## Background — what's already there

- [exterior-leaf-cull](../../done/exterior-leaf-cull/index.md) already flood-fills empty leaves from a player-start seed, classifies the rest as exterior, and **warns** when every empty leaf is exterior ("no interior empty leaves remain after exterior culling — map may be unsealed or have a leak").
- The flood-fill infrastructure and portal graph exist; the data needed to emit a pointfile is already in memory when the warn fires.
- The SDF baker (sub-plan 8) currently assumes watertight input. A leak silently produces bad bake output.

---

## Pipeline integration

```
prl-build
  → geometry extraction
  → brush BSP
  → exterior flood-fill  ←— currently warns on leak; upgrade to hard error + pointfile
  → portals / PVS
  → BVH
  → SH bake
  → SDF bake            ←— assumes watertight; relies on the error gate above
  → pack
```

Leak detection runs early (right after flood-fill), so downstream stages (PVS, BVH, baking) never execute on a broken map. Bake-time costs are not wasted on maps that will be rejected.

---

## Scope

### In scope

- **Hard-error gate** in `prl-build`: when the flood-fill from the player-start seed fails to seal any interior, exit with a non-zero status and an actionable error message.
- **Pointfile output (`.pts`)**: a polyline tracing a path from the player-start (or any interior seed) through portals to the nearest void-adjacent leaf. Format matches Quake/ericw-tools convention (one vertex per line, `x y z\n`).
- **Seed selection**: find an interior reference point from a canonical entity (e.g. `info_player_start`); fall back to the bounding-box center with a warning if none exists.
- **CLI flag** `--allow-leaks` to downgrade the error back to a warning, for modders who deliberately ship unsealed test maps. Default is strict.
- **SDF baker coupling**: with leaks gated out upstream, the parity test in `sdf_bake.rs::parity_inside` can assume watertight input; drop the current +Y-infinity heuristic in favor of seeding from the same player-start point used for the leak-detection flood-fill. Migrate the seed through `SdfBakeInputs`.
- **Documentation**: add a short "fixing leaks" note to `context/lib/build_pipeline.md` with a screenshot/instructions for loading `.pts` in TrenchBroom.

### Out of scope

- Automatic leak repair (patching the geometry).
- Multi-leak reporting in a single run. The first leak is found, the pointfile is written, compilation fails. Re-run after fixing to find the next one.
- Leak visualization inside the engine at runtime (the `.pts` file is a compile-time author aid).
- Reporting leaks for dynamic or point entities separately from the world shell.

---

## Open questions

1. **Pointfile format compatibility.** Does TrenchBroom's current `.pts` loader match the original Quake format exactly, or is there a TB-native variant? Verify before implementation to avoid a format mismatch surprising the user.
2. **Seed entity canonicalization.** The FGD already defines `info_player_start`. Is there ever a map with multiple starts (co-op / deathmatch variants)? If so, any of them should work; pick the first one deterministically.
3. **Error message taxonomy.** Should we distinguish "no player-start entity" from "player-start is in the void" from "player-start is in a sealed interior but another empty leaf leaks"? The first two are authoring errors with different fixes; the last is the classic leak.

---

## Acceptance criteria

- [ ] `prl-build` returns a non-zero exit status on any leak, by default.
- [ ] A `.pts` file is written alongside the `.map` input when a leak is detected; its format is readable by TrenchBroom's pointfile loader.
- [ ] The error message names the seed entity used, the coordinates of the void-adjacent leaf, and the path to the `.pts` file.
- [ ] `--allow-leaks` CLI flag downgrades the error to a warning and skips `.pts` emission.
- [ ] `sdf_bake.rs::parity_inside` is simplified to seed from the canonical interior point (player-start), and the current +Y-infinity comment/heuristic is retired.
- [ ] A regression test: a deliberately leaky `.map` fixture fails compilation and produces a `.pts` file with at least one vertex outside the world AABB.
- [ ] Documentation updated in `context/lib/build_pipeline.md` with a "fixing leaks" subsection.

---

## Rough implementation sketch

1. **Seed resolution.** In `prl-build`, extract the `info_player_start` origin from the parsed entity list before the BSP stage; thread it into flood-fill inputs.
2. **Flood-fill upgrade.** Change the current warn path in `visibility/mod.rs` to return a `LeakReport { seed_leaf, void_leaf, portal_path }` when no interior remains. The caller in `main.rs` decides whether to error or warn based on `--allow-leaks`.
3. **Pointfile writer.** New module `postretro-level-compiler/src/leak_report.rs`. Takes a `LeakReport` and writes the `.pts` file. Path reconstruction: walk portals backward from the first void-adjacent empty leaf to the seed leaf, emit portal centroids as vertices.
4. **SDF baker seed.** Thread the same player-start origin into `SdfBakeInputs`; replace `parity_inside`'s +Y ray with a ray from the brick center to the seed, counting intersections.
5. **Test fixture.** Add `assets/maps/test_leak.map` — a simple room with one missing brush face. Assert compilation fails with exit code 1 and produces `test_leak.pts`.

---

## When this plan ships

Durable knowledge migrates to `context/lib/build_pipeline.md`:

- Leak detection is a compiler error by default; `--allow-leaks` is the escape hatch.
- `.pts` pointfile format and TrenchBroom loading instructions.
- The watertight-geometry contract that downstream stages (SDF baker, future AO baker, etc.) rely on.

The plan document itself is removed per `development_guide.md` §1.5.
