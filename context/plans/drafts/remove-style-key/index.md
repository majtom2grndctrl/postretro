# Remove Legacy `style` Key

> **Status:** draft
> **Depends on:** `context/plans/ready/scripting-foundation/plan-2-light-entity.md` — must ship first.
> **Related:** `context/lib/build_pipeline.md` §Custom FGD · `postretro-level-compiler/src/format/quake_map.rs`

---

## Goal

Delete the Quake-era `style` integer key from the FGD, the compiler, and all test maps. Plan 2 replaced it with `brightness_curve`, `color_curve`, and `direction_curve` — first-class authored keyframe channels. Once Plan 2 ships, `style` is dead weight: the FGD still advertises it, the compiler still parses it, and six test maps still author with it. This plan removes all three.

No new features. No new primitives. Cleanup only.

---

## Settled decisions

- **No deprecation period.** Pre-release project; no shims, no warnings-and-remove cycle. One pass: delete, fix all consumers, ship.
- **No migration path for external maps.** Any `.map` file using `style` must be updated to `brightness_curve` by its author. The compiler will error on unknown key if strict validation is added, or silently ignore it if not — either is acceptable because external map files are not a support concern at this stage.
- **`_phase` stays.** Plan 2 promotes `_phase` to a first-class field on `LightAnimation`. It is not removed by this plan.
- **FGD `_bake_only` and `_dynamic` stay.** Those keys are unrelated to animation style.

---

## Sub-plans

### 1. FGD Removal

**Scope.** Remove `style` from the Light base class in `sdk/TrenchBroom/postretro.fgd`. Update the `build_pipeline.md` entity table.

| File | Change |
|------|--------|
| `sdk/TrenchBroom/postretro.fgd` | Delete the `style(integer)` line from the `Light` base class. |
| `context/lib/build_pipeline.md` | Remove `style` from the `light` entity row in the Custom FGD table. |

**Acceptance criteria**

- [ ] `style` does not appear in `sdk/TrenchBroom/postretro.fgd`.
- [ ] TrenchBroom no longer shows an "Animation Style" field on any light entity.
- [ ] `build_pipeline.md` entity table reflects the removal.

---

### 2. Compiler Removal

**Scope.** Remove `style` parsing, the `quake_style_animation` lookup table, and all related branching from the compiler. Remove `style`-keyed test fixtures.

| File | Change |
|------|--------|
| `postretro-level-compiler/src/format/quake_map.rs` | Delete `parse_optional_int(props, "style")` call and its result binding. Delete the `quake_style_animation` function and its call site. Delete the `style == 0` guard around `_phase` and `_start_inactive` warnings — those warnings should remain but now unconditionally apply when `brightness_curve` is absent. Update test fixtures that set `("style", "0")` or `("style", "1")`. |

The `animation` field on the translated light was either `None` (style 0) or populated by `quake_style_animation`. After removal, `animation` is `None` for any light that lacks a `brightness_curve`, `color_curve`, or `direction_curve` key — the same semantics as `style 0`. No change to the downstream `LightAnimation` type or the PRL format.

**Acceptance criteria**

- [ ] `quake_style_animation` does not exist in the compiler.
- [ ] `"style"` is not read from entity property maps anywhere in `postretro-level-compiler/src/`.
- [ ] All test fixtures that set `("style", _)` are updated or removed.
- [ ] `cargo test -p postretro-level-compiler` passes.
- [ ] `cargo clippy -p postretro-level-compiler -- -D warnings` clean.

---

### 3. Test Map Updates

**Scope.** Replace `style` with `brightness_curve` (and where needed `color_curve`) in all test maps that currently use it. No map should retain the `style` key after this sub-plan.

Maps that currently use `style`:

| Map file | `style` values in use |
|----------|-----------------------|
| `content/base/maps/test_animated_weight_maps_single.map` | `"2"` |
| `content/base/maps/test_animated_weight_maps_occluded.map` | `"2"` |
| `content/base/maps/test_animated_weight_maps_cap.map` | `"1"`, `"2"`, `"3"`, `"5"` |
| `content/base/maps/test_animated_weight_maps_mixed.map` | `"2"` |
| `content/base/maps/test-3.map` | `"3"` (four lights) |

Each `style` value maps to the Quake pattern string documented in `quake_style_animation`. Translate each to an equivalent `brightness_curve` keyframe sequence that preserves the animation intent at 10 Hz sampling (10 samples per second, period = sample count × 0.1 s). Exact sample fidelity is not required — a representative curve that exercises the same code path is sufficient for test maps.

**Acceptance criteria**

- [ ] `grep -r '"style"' content/base/maps/` returns no matches.
- [ ] Each updated map compiles without errors or warnings related to unknown entity keys.
- [ ] `test_animated_weight_maps_*` maps produce animated lights when compiled with `prl-build`.

---

### 4. Context Doc Updates

**Scope.** Remove all references to `style` as a live key from context library files. This sub-plan does not touch plan docs in `done/` (those are frozen historical records).

| File | Change |
|------|--------|
| `context/lib/build_pipeline.md` | Already handled in sub-plan 1. No additional changes. |

If `style` appears in any other `context/lib/` file, remove or update the reference. (At the time of drafting, `build_pipeline.md` is the only `context/lib/` file that names `style` as a supported key.)

**Acceptance criteria**

- [ ] `grep -r 'style' context/lib/` returns no results referencing `style` as a light entity key.

---

## Non-goals

- Removing `_phase`. It is promoted to a first-class field by Plan 2 and stays.
- Removing any other light entity keys (`_color`, `_fade`, `delay`, `_dynamic`, `_bake_only`).
- Adding a compiler error for unknown entity keys. Useful but out of scope for this plan.
- Updating autosave map files under `content/base/maps/autosave/`. Autosaves are not source assets; they can be left as-is or deleted as part of normal autosave rotation.
- Updating `plans/done/` documents. Frozen at ship time; do not touch.
- Providing any tooling to convert existing `style` maps to `brightness_curve` format.
