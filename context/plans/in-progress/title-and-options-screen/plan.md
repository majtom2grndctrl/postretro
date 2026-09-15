# title-and-options-screen — plan of record

mode: compact
status: active
read at: e36e86b57

## Corrections

- No cited source changed after the brief's `read at` commit. The intervening source changes are confined to the level compiler, so the grounded UI, options, input, and renderer reads remain current.

## Delegated answers

- Shadow tiers — `low` = 512, `medium` = 768, `high` = 1024 spot-shadow pixels. `high` is the default and preserves the shipped allocation ceiling; the lower tiers produce meaningful 4× and ~1.8× per-layer memory reductions without raising the current 96-layer VRAM floor.
- Fog tiers — `low` = 1.0, `medium` = 0.5, `high` = 0.25 world units per march step. `medium` preserves the shipped default; the endpoints span the existing dev-tools control while leaving per-map pixel scale untouched.
- Fog quality surface — `fog_step_size` alone is sufficient for this brief. The player tier does not override or cap worldspawn `fog_pixel_scale`, preserving map-authority exactly.
- Save debounce — a deterministic 250 ms settle timer driven by frame delta. Each accepted slot change applies immediately and restarts the timer; close and clean exit synchronously flush pending state through `PlayerOptions::save`.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| A1 graphics fields round-trip/default/corrupt fallback | `player_options_graphics_quality_roundtrips_and_defaults`; `load_returns_defaults_and_preserves_file_when_graphics_quality_is_unknown` | achievable as stated |
| A2 option-slot coercion, validation, readonly and unknown rejection | engine-state catalog/slot-table tests plus `write_state_slot_json_validates_every_option_slot`; existing readonly log test; system-reaction unknown-slot log test | achievable as stated |
| A3 opening options seeds non-default in-memory values | `opening_options_seeds_every_slot_from_player_options` | achievable as stated |
| A4 matching-field update, one settled save, last-write-wins, close/exit flush, atomic file | `one_slot_change_updates_only_matching_field`; `slider_changes_debounce_to_one_last_value_save`; `closing_options_flushes_pending_save`; `exiting_flushes_pending_save`; existing atomic-save test | achievable as stated |
| A5 fog tier maps both endpoints to renderer setter values | `fog_quality_maps_low_and_high_step_sizes` | achievable as stated |
| A6 full renderer rebuild re-applies current fog tier | app render-profile seam test around full-init apply order plus renderer fog state test | achievable as stated |
| A7 shadow tiers map to distinct construction resolutions; pool count fixed | `shadow_quality_maps_low_and_high_resolutions`; renderer construction-source test proving pool uses configured resolution and fixed `SHADOW_POOL_SIZE` | achievable as stated |
| A8 composed enum control predicate and reaction write | script-compiler/dev manifest fixture asserting option button `checked`/`bind` predicate and named `setState` reaction share the same slot/value | achievable as stated |
| A9 reopen before save re-seeds from in-memory value | `reopening_options_seeds_unsaved_in_memory_value` | achievable as stated |
| A10 title-time input option applies before first level | options bridge effect test plus existing session-lifetime `InputSystem` level-install boundary | achievable as stated |
| A11 shadow reload uses current in-memory option | pre-full-init render-profile mapping test using changed `PlayerOptions` | achievable as stated |
| A12 save failure keeps field/slot and file intact; later change retries | `failed_save_keeps_applied_value_and_later_change_retries` | achievable as stated |
| A13 no live shadow rebuild path | grep gate: `shadow_quality`/`shadowQuality` references; only options store, slots, authored UI, and pre-full-init shadow resolution mapping may reach renderer | achievable as stated |
| A14 renderer apply uses app chokepoint, never UI snapshot; UI writes no store/graphics state | grep gate plus app render-profile module tests | achievable as stated |
| M1 title Play/Options/Exit and submenu Back flow | owner, in-engine runbook | manual-visual |
| M2 sensitivity changes live and persists | owner, in-engine runbook | manual-visual |
| M3 invert-Y changes live, highlights, persists | owner, in-engine runbook | manual-visual |
| M4 view-feel zero suppresses presentation and persists | owner, in-engine runbook | manual-visual |
| M5 crouch hold/toggle changes live, highlights, persists | owner, in-engine runbook | manual-visual |
| M6 fog tier changes density live and persists | owner, fogged-map runbook | manual-visual |
| M7 shadow reload hint and changed resolution after reload | owner, shadowed-map runbook | manual-visual |
| M8 title-time options apply on first loaded map | owner, in-engine runbook | manual-visual |
| M9 first boot defaults and settings creation | owner, clean-config runbook | manual-visual |
| M10 all six controls reachable and operable by keyboard/gamepad | owner, accessibility/input runbook | manual-visual |
| M11 corrupt settings fallback preserves file | owner, corrupt-config runbook | manual-visual |
| M12 submenu cancel/Start stack discipline | owner, in-engine runbook | manual-visual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Prove the riskiest end-to-end seam with the mouse-sensitivity slot: catalog/SDK exposure, options-open seed, bridge apply, deterministic debounce, save, and reopen behavior. | integrating executor | — | |
| 2 | Complete the option-slot bridge for invert-Y, view-feel, crouch, fog, and shadow; add graphics enums and persistence/error coverage. | integrating executor | 1 | |
| 3 | Thread shadow resolution through renderer full construction and apply fog/shadow profiles only through the app render-profile chokepoint, including rebuild ordering tests. | integrating executor | 2 | |
| 4 | Author the dev title/options trees and composed controls/reactions; preserve frontend camera hold across owned submenus and add script/UI contract coverage. | integrating executor | 1, 2 | |
| 5 | Integrate both frame paths and clean exit, run focused gates, review/fix loop, final preflight, durable context update, and landing table. | integrating executor | 3, 4 | |
