# Lighting `_dynamic` FGD Flag

> **Status:** draft — small plan, suitable for delegation to a background Sonnet agent.
> **Part of:** the lighting stack rework. Siblings: `lighting-old-stack-retirement/`, `lighting-compiler-rework/`, `lighting-runtime-rework/`.
> **Gates:** `lighting-compiler-rework/`, `lighting-runtime-rework/` — both consume `MapLight.is_dynamic`.
> **Related:** `context/lib/build_pipeline.md` §Custom FGD · `context/plans/in-progress/lighting-foundation/1-fgd-canonical.md` (original FGD sub-plan; this extends it).

---

## Context

The lighting rework splits lights into **static** (bake into lightmap + SH + spec-only buffer; no runtime shadow) and **dynamic** (runtime direct evaluation with optional shadow-map pool slot; no bake contribution). Authors tag lights in TrenchBroom via a single FGD property. Downstream bakers and runtime consumers branch on the flag.

Static is the default — existing maps need no edits, and the expected authoring flow is to mark only a handful of lights dynamic per level.

---

## Goal

Add `_dynamic` as a boolean property on the three light entity classes (`light`, `light_spot`, `light_sun`) in the FGD. Plumb through the parser, translator, and canonical `MapLight`.

---

## Approach

Single-pass change across four layers, top to bottom:

1. **FGD.** Add `_dynamic(choices) : "Dynamic (runtime pool)" : 0 = [ 0 : "Static (baked)", 1 : "Dynamic" ]` to each of `light`, `light_spot`, `light_sun` in `assets/postretro.fgd`. Absent in existing maps parses as static.
2. **Parser.** In `prl-build`'s entity-property extraction path, pick up `_dynamic` alongside existing keys. Treat missing / non-integer values as `false`.
3. **Translator.** In `postretro-level-compiler/src/format/quake_map.rs`, read the parsed value and set `MapLight.is_dynamic`.
4. **Canonical type.** Add `is_dynamic: bool` to `MapLight` in the level-compiler's map-light module. Default `false` in any derive / construction helper.
5. **Doc update.** `context/lib/build_pipeline.md` §Custom FGD table — add the new property row.

---

## Files to modify

| File | Change |
|------|--------|
| `assets/postretro.fgd` | Add `_dynamic` property to `light`, `light_spot`, `light_sun` |
| `postretro-level-compiler/src/format/quake_map.rs` | Parse `_dynamic`, set on `MapLight` |
| Canonical `MapLight` definition (module path resolved during implementation) | Add `is_dynamic: bool` field |
| `context/lib/build_pipeline.md` §Custom FGD | Document the new property |

---

## Acceptance Criteria

1. `cargo test -p postretro-level-compiler` passes.
2. Compiling an existing test map without edits succeeds and every `MapLight` has `is_dynamic == false`.
3. A test map with `_dynamic 1` on a light compiles and the corresponding `MapLight` has `is_dynamic == true`.
4. FGD documentation updated in `build_pipeline.md`.
5. `cargo clippy --workspace -- -D warnings` clean.
6. No new `unsafe`.

---

## Out of scope

- Baker and runtime consumers of `is_dynamic` — `lighting-compiler-rework/` and `lighting-runtime-rework/`.
- UDMF support for the flag — separate initiative; architecture accommodates without refactor.
- Authoring migration (marking any lights dynamic in existing maps) — author-driven, not part of this plan.
