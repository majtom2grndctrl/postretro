# gameplay-stack--sim-crate

Brief · resumable · Epic `gameplay-stack-decomposition` (M2) · reads: `context/lib/development_guide.md` §Layering invariants · `context/lib/scripting.md` §12 · `context/lib/networking.md` §Weapon placement is content · read at 3acc6d6

## Problem

Developer-raised, anticipated need. `crates/postretro` is a single bin-only crate of ~222K LOC, so every edit to it recompiles all of it: touching `startup/`, `input/`, or `main.rs` rebuilds the entire fixed-tick gameplay cluster, and touching gameplay rebuilds everything else. The cause is that the cluster has no crate boundary — `development_guide.md` §Layering invariants records why ("Collision lives in the binary. So do the fixed-tick gameplay systems… because they call into the binary's collision module"), and M1 removed the cycle that blocked the lift. When this is done, the fixed-tick core is a `postretro-sim` crate below the binary with collision inside it, and an edit to binary-only code recompiles no gameplay.

## Decisions

- **`postretro-sim` holds the fixed-tick cluster and collision**: `sim`, `scripting` (with its `scripting_systems` `#[path]` root), `collision`, `nav`, `weapon`, `movement`, `kinematic_mover`, `agent`, `agent_steering`, `combat_positioning`, `trigger_system`, `trigger_bindings`, `trigger_commands`, `trigger_pools`, `impact_policy`, `impact_effects`, `spawner`, `health`, `grant`, `fx`. The `scripting ↔ sim` cycle becomes intra-crate and legal. External deps are glam, parry3d, serde, serde_json, log over `foundation`/`entities`/`combat-model`/`net`/`scripting-core`; the VMs arrive transitively through `scripting-core` and are expected there.
- **`fx` moves whole and adds no dependency edge.** Its contents are emitter and fog reaction primitives over `entities` and `scripting-core` only — no wgpu, no `crate::render`, no `glam`. The presentation data the epic's open question assumed still lives there now sits in `postretro-render-cpu::fx`. This resolves that question; the hub records it.
- **`scripting/systems/input_mode.rs` stays in the binary.** It self-describes as App composition rather than a subsystem output, and every consumer — `startup/lifecycle.rs`, `session/mod.rs`, `main.rs` — is binary-side. Leaving it behind dissolves the `input::InputMode` up-edge without sinking a type; its own call into the script store becomes a down-edge.
- **`TICK_DURATION` moves into `postretro-sim`; `frame_timing` reads it down.** The fixed-tick period belongs to the crate that defines the fixed tick. Inverting here is cheaper than sinking a `Duration` to a leaf that owns no tick vocabulary.
- **`resolve_weapon_placement` sinks to `postretro-foundation`**, with `legacy_weapon_placement` and `BASE_OFFSET`. Foundation already defines `WeaponPlacementDescriptor`, `PlacementOffset` and `PlacementRotation` and already documents the legacy fallback, so the resolution rule joins the type it resolves. Both sim and the binary then call down, and the netcode caller keeps working unchanged through M3. The precedence chain is mirrored in SDK doc strings under `scripting/primitives/`; those strings move or stay accurate.
- **`MAX_DELAY_MICROS` sinks to `postretro-net`**, which already owns the sibling timing vocabulary (`DEFAULT_MICROS_PER_TICK`, `SAMPLE_PERIOD_US`). Netcode's edge is unchanged and `spawner.rs` gains a down-edge. This is the one surviving cluster→netcode up-edge M1 left behind.
- **Only the tests that name netcode types move to the binary.** Test modules in `sim/weapon_stage.rs`, `sim/touch.rs`, `sim/determinism_tests.rs`, `scripting/systems/ai_tests.rs` and `impact_effects.rs` reach netcode harness types (`MovementOwners`, `NetworkIdAllocator`, `OpenAuthorizedShots`, `SeatTable`, `HostCommandQueues`, `ReplicableSet`), and `postretro` is bin-only so no dev-dependency can reach it. The netcode-touching tests relocate and call sim through its public API; the harness types do not sink, because they are netcode session state and sinking them would put replication state in a leaf below the crate that owns it. Most tests in those files name no netcode type and stay put.
- **Visibility widens only where a boundary is crossed.** `pub(crate)` becomes `pub` for items the binary or a relocated test now reaches — the `simulate_tick` seam among them. A blanket widening of the cluster's `pub(crate)` surface is not part of this work.
- **Non-goals.** Netcode stays in the binary (M3 lifts it) and AI stays inside `postretro-sim` (M4 carves it); both are epic-sequenced and neither is achievable here. Spawn attack-windup keeps tracking the interpolation ceiling — decoupling it is a gameplay-semantics change against the behavior table in `plans/done/E10--enemy-multi-attack/`, not an extraction. No behavior, wire, scripting-semantics or SDK-typedef change. No combat-logic consolidation into `postretro-combat-model`.

## Acceptance

### Automated
- [ ] `cargo build --workspace` and `cargo test --workspace` pass.
- [ ] `layering_invariants_hold` passes; `postretro-sim` has no dependency on `postretro`, and `postretro` is still depended on by nothing.
- [ ] `cargo tree -p postretro-sim` shows no `wgpu`, `winit` or `glyphon`. `mlua`/`rquickjs` appear transitively via `scripting-core`.
- [ ] No production reference to `crate::netcode::` remains anywhere in `postretro-sim`, and none to a binary-root item (`App`, `session`, `resolve_weapon_placement`, `frame_timing`, `input`, `render`).
- [ ] Touching a binary-only file leaves `postretro-sim` unrebuilt; touching a `postretro-sim` file rebuilds it and relinks the binary.
- [ ] The M15 Phase 0 determinism harness passes unchanged — same tick sequence, same event grouping.
- [ ] `scripts-build` emits byte-identical SDK typedefs against the pre-move tree.
- [ ] Weapon placement resolves identically at both ends of the precedence chain: an entirely unauthored weapon still yields the legacy offset with zero rotation, and an instance-authored weapon still overrides all three lower tiers.
- [ ] A newly spawned enemy still cannot attack for 250 ms, and an enemy whose attack cooldown already exceeds that keeps the larger value.
- [ ] Every relocated test still runs — confirm the filter matched, and that pre-move and post-move test counts agree for `sim/weapon_stage.rs`, `sim/touch.rs`, `sim/determinism_tests.rs`, `scripting/systems/ai_tests.rs` and `impact_effects.rs`.
- [ ] No item is widened to `pub` without a caller outside `postretro-sim`.

### Manual
- [ ] `cargo run -p xtask -- run content/dev/maps/campaign-test.prl` plays identically: movement feel, enemy engagement, weapon fire, triggers, movers.
- [ ] A dev-tools launch (`--features dev-tools`) still starts and its debug UI still draws.
- [ ] A two-peer co-op session still joins, replicates and reconciles — netcode reaches down into the new crate and that path is unproven by unit tests alone.

## Path

Non-binding.

- **First slice: boundary-prep, before any module moves.** Sever the five up-edges — `TICK_DURATION`, `resolve_weapon_placement`, `MAX_DELAY_MICROS`, leaving `input_mode.rs` behind, and the `fx` re-export aliases in `scripting/reactions/{mod,registry}.rs`. Each lands green on the existing tree and none depends on the crate existing. This is the thinnest path that falsifies the riskiest assumption: that the survey of up-edges is complete. If severing surfaces edges this brief does not name, the manifest is wrong and the transplant should wait.
- **Then test relocation**, also on the existing tree. After it, `rg 'crate::netcode' ` over the cluster is empty in production and test code alike, and the transplant carries no test debt.
- **Then one transplant.** E19's terminal renderer cut established move-everything-at-once over staged extraction: a partial cluster does not compile, so staging buys checkpoints that cannot be green. The rival — module-by-module with shim re-exports — trades a large atomic diff for a long sequence of temporary API surface, and the epic's own execution model already rejects it.
- **Seams to investigate by symbol:** `sim::simulate_tick` and `simulate_tick_with_presentation_aim` are already fully data-injected and call nothing in `input`/`frame_timing`/`render` (`plans/done/M15--p0-headless-sim-seam/` formalized this) — they are the natural public face of the crate. `scripting_systems` is re-rooted through `#[path]` in `main.rs`; the move makes it an ordinary module and that indirection should not survive.
- `crates/postretro/src/main.rs` is ~13.6K lines. This work only removes from it; do not take a split on as part of the transplant.

## Open questions

- Placement of `agent_diagnostics`, `mover_diagnostics`, `trigger_diagnostics`, `door_occluder_diagnostics` — they have no in-tree callers and may be registered only from `main.rs` — **delegated**: the executor decides and reports it in the plan of record.
- Placement of `sprite_collection` (`derive_collection_id` has production callers on both sides of the boundary), `presentation_pool` (named in an `impact_policy` signature) and `runtime_movers` — **delegated**: follow the dependency direction that avoids a shim.
