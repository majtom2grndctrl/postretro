# title-and-options-screen — plan of record

mode: compact
status: landed-with-gaps
read at: e36e86b57

## Corrections

- No cited source changed after the brief's `read at` commit. The intervening source changes are confined to the level compiler, so the grounded UI, options, input, and renderer reads remain current.
- The brief's `0.1–10.0` mouse-sensitivity slider was illustrative and does not match the engine's radians-per-raw-unit storage. The authored control uses `0.0005–0.01` with a `0.0005` step, spanning the shipped `0.002` default without a hidden unit conversion.

## Delegated answers

- Shadow tiers — `low` = 512, `medium` = 768, `high` = 1024 spot-shadow pixels. `high` is the default and preserves the shipped allocation ceiling; the lower tiers produce meaningful 4× and ~1.8× per-layer memory reductions without raising the current 96-layer VRAM floor.
- Fog tiers — `low` = 1.0, `medium` = 0.5, `high` = 0.25 world units per march step. `medium` preserves the shipped default; the endpoints span the existing dev-tools control while leaving per-map pixel scale untouched.
- Fog quality surface — `fog_step_size` alone is sufficient for this brief. The player tier does not override or cap worldspawn `fog_pixel_scale`, preserving map-authority exactly.
- Save debounce — a deterministic 250 ms settle timer driven by frame delta. Each accepted slot change applies immediately and restarts the timer; close and clean exit synchronously flush pending state through `PlayerOptions::save`.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| A1 graphics fields round-trip/default/corrupt fallback | `player_options_graphics_quality_roundtrips_and_defaults`; `load_returns_defaults_and_preserves_file_when_graphics_quality_is_unknown` | passed |
| A2 option-slot coercion, validation, readonly and unknown rejection | catalog/slot-table suites; `write_state_slot_json_validates_every_option_slot`; existing readonly warning and unknown-slot error paths | passed |
| A3 opening options seeds non-default in-memory values | `opening_options_seeds_every_slot_from_player_options` | passed |
| A4 matching-field update, one settled save, last-write-wins, close/exit flush, atomic file | `one_slot_change_updates_only_matching_field_and_applies_input`; `slider_changes_debounce_to_one_last_value_save`; `closing_and_exiting_flush_pending_saves`; existing atomic-save test | passed |
| A5 fog tier maps both endpoints to renderer setter values | `fog_quality_maps_to_ray_march_step_size` | passed |
| A6 full renderer rebuild re-applies current fog tier | full-init calls the app render-profile seam after `ensure_full_ready`; mapping/default tests | passed |
| A7 shadow tiers map to distinct construction resolutions; pool count fixed | `shadow_quality_maps_to_spot_resolution`; construction threads configured resolution while `SHADOW_POOL_SIZE` stays 96 | passed |
| A8 composed enum control predicate and reaction write | `production_title_and_options_trees_preserve_composed_control_contracts` bundles the real dev manifest and proves predicate/bind/style plus matching `setState` | passed |
| A9 reopen before save re-seeds from in-memory value | `reopening_options_seeds_unsaved_in_memory_value` | passed |
| A10 title-time input option applies before first level | bridge input-effect tests plus session-lifetime `InputSystem` across level install | passed |
| A11 shadow reload uses current in-memory option | level install reads current `PlayerOptions`, updates renderer boot state, and `spot_shadow_quality_rebuilds_only_when_level_boundary_resolution_changes` proves the boundary predicate | passed |
| A12 save failure keeps field/slot and file intact; later change retries | `failed_save_keeps_applied_value_and_later_change_retries` | passed |
| A13 no live shadow rebuild path | grep gate confirms the setter only changes CPU boot state; rebuild is confined to full-init/level install | passed |
| A14 renderer apply uses app chokepoint, never UI snapshot; UI writes no store/graphics state | grep gate plus app render-profile tests and real-manifest contract test | passed |
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
| 1 | Prove the riskiest end-to-end seam with the mouse-sensitivity slot: catalog/SDK exposure, options-open seed, bridge apply, deterministic debounce, save, and reopen behavior. | integrating executor | — | complete |
| 2 | Complete the option-slot bridge for invert-Y, view-feel, crouch, fog, and shadow; add graphics enums and persistence/error coverage. | integrating executor | 1 | complete |
| 3 | Thread shadow resolution through renderer full construction and apply fog/shadow profiles only through the app render-profile chokepoint, including rebuild ordering tests. | integrating executor | 2 | complete |
| 4 | Author the dev title/options trees and composed controls/reactions; preserve frontend camera hold across owned submenus and add script/UI contract coverage. | integrating executor | 1, 2 | complete |
| 5 | Integrate both frame paths and clean exit, run focused gates, review/fix loop, final preflight, durable context update, and landing table. | integrating executor | 3, 4 | complete |

## Landing record

- Implementation checkpoint: `acfb6a130` (`Build title and options screens`).
- Review panel: correctness, contract, hygiene, and adversarial passes found no functional defects. Six stale comments were corrected; the post-fix focused gate passed.
- Final preflight (2026-09-15): `cargo fmt --check` passed; `cargo clippy --target-dir target/preflight-clippy -- -D warnings` passed after deriving `Default` for `OptionsBridge`; full `cargo test` passed.
- Landing status is `landed-with-gaps`: A1–A14 have automated proof; M1–M12 remain owner-run visual/in-engine checks because this execution did not exercise an interactive GPU/input session or mutate the owner's real config directory.
