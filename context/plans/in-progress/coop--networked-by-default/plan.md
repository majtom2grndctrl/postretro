# coop--networked-by-default — plan of record

mode: resumable
status: blocked
read at: fe1881a35

## Session state

- Owner selected resumable mode on 2026-10-10; the promoted brief had no mode header.
- Claim commit `fe1881a35` pushed to `origin/main`.
- Feature branch: `codex/coop-networked-by-default`.
- Source verification stopped on a false Decision premise. No implementation or tests run. No Decision or Acceptance wording changed.
- `git diff 221fdaf4a -- crates` is empty: research baseline has no source drift.

## Block: held wieldables can retain networking components

Decision 1 says a held wieldable “carries `Weapon` only,” and defines descriptor replication solely by spawn path and remaining live components. The exemption table says held wieldables are not networked; AC 4 requires pickup to unregister them.

Current accepted descriptors can combine `Weapon`, `Touchable`, and `Health`. Pickup removes `Mesh` and `Touchable`, but retains `Health`. Under Decision 1's proposed classifier, that held item still qualifies as networked. A loadout instance with `DefaultWeapon` provenance has the same problem immediately after composition. This contradicts the held exemption and AC 4; it is not source drift or a renamed Path symbol.

Source evidence at `fe1881a35`:

- `crates/scripting-core/src/data_descriptors/js/entity.rs:85–114` parses and validates weapon, touchable, and health independently; `:171–185` returns their combined descriptor.
- `crates/scripting-core/src/data_descriptors/lua/entity.rs` accepts the same combination through independent component validators.
- `crates/scripting-core/src/runtime/mod_init_exec.rs:273–287` retains the parsed descriptor; `crates/scripting-core/src/runtime/core.rs:381` commits entity types. `crates/entities/src/data_registry.rs:951` and `:1014` handle identity/deduplication without rejecting this component combination.
- `crates/sim/src/scripting/builtins/data_archetype.rs:638–645` attaches authored health; `:655–711` also attaches Brain/Agent when behavior exists.
- `crates/sim/src/scripting/builtins/wieldable_inventory.rs:170–188` spawns the full loadout descriptor with `DefaultWeapon` provenance, then removes only Mesh and Touchable.
- `crates/sim/src/scripting/builtins/wieldable_inventory.rs:237–264` acquires a live item into Inventory, then removes only Mesh and Touchable.
- `crates/sim/src/sim/touch.rs:593–624` routes acquisition through that helper; its world-item cleanup also leaves Health intact.
- Existing `authored_loadout_strips_world_item_components_from_held_wieldable` at `wieldable_inventory.rs:541` covers Mesh/Touchable removal, not Health or Brain/Agent retention.

This is a source-traced counterexample, not an executed test result. Adding `health: { max: 10 }` to an otherwise valid touchable weapon descriptor is sufficient; no mesh or behavior is needed.

## Owner decision needed

Recommended amendment to Decision 1, not applied:

> An entity currently held in an Inventory is excluded from descriptor-entity replication, regardless of its remaining components. Outside an Inventory, the stated spawn-path and live-component classifier applies. Pickup unregisters the entity; dropping it re-registers it. Holding does not require removing its other components.

This preserves the held-item exemption and AC 4 without narrowing accepted descriptor combinations or deleting component state on pickup. It changes Decision 1's explicit iff predicate, so the executor cannot adopt it without owner direction. If accepted, reconcile the “Weapon only” premise and the held-item discussion with that rule, and extend pickup/drop proof to health-bearing items and `DefaultWeapon` instances. The owner may choose a different policy instead.

## Corrections and audit notes

These do not resolve the block or change Decisions/Acceptance:

- Task 9 source-specific crossing/levelLoad gates also live in `crates/postretro/src/startup/lifecycle.rs:1135–1210`; include that file and staged-recomposition coverage, beyond `startup/reaction_validation.rs`.
- Task 1's sim extraction must preserve `descriptor_carries_brain` access from `descriptor_mesh_component` (`data_archetype.rs:757`), rather than the stated direct caller `attach_descriptor_components`.
- Only the sim `host_replicated` extraction is an immediate prerequisite for the dummy slice; the other four Task 1 splits can precede their consuming tasks.
- Task 9's existing private player-event traversal returns the first violation. Adapt it to enumerate all reached MachineLocal reactions for the required once-per-(binding, reaction) warning.
- Task 8 must preserve order across the currently separate local and routed command queues. Changing recipient variants alone does not meet the stated push-order contract.
- Task 8 also updates the app named-event wrapper (`crates/postretro/src/main.rs:417`) and the crossing context constructor (`crates/sim/src/scripting/reactions/mod.rs:60`).
- Task 11's deterministic clock proof needs an injected RTT/clock seam: `ClientTimeSync` currently uses an Instant-based `EngineClock`, while the conditioned app fixture advances only its packet conditioner's virtual clock. Use the planned RTT apply input or an explicit test-clock hook; do not use wall-clock sleeps.
- Task 11 must account for the emitter bridge adding frame delta immediately after detecting a new spin tween (`crates/postretro/src/scripting/frame_systems/emitter_bridge.rs:259–265`). Verify that a seeded age survives the same frame's bridge pass without double advancement.
- The app conditioned harness is `crates/postretro/src/host_activations/conditioned_tests.rs`; it currently exercises combat, so presentation bridge/routing proof must extend its CPU seam.
- AC 6 needs the meshless target's client declaration through host HIT application in addition to local HitVolume query tests. AC 9's snapshot interval is 2 ticks at 60 Hz, about 33.3 ms (`crates/netcode/src/host.rs:24`).

## Resume

1. Apply only the owner's authorized rule/wording; return to **Verify source** and recheck the held-item conflict and its consumers.
2. Complete source verification for every Decision and Path; do not treat the partial audit above as full approval.
3. Write the complete AC-to-proof table for automated rows 1–21 and manual rows M1–M4, then the implementation tasks and ownership. The first milestone should prove the networked dummy path, with only necessary split prerequisites ahead of it.
4. Commit a `status: proposed` resumable plan and stop for the owner's plan skim. A resolution of this block alone is not approval of a not-yet-written implementation plan.

Disk at this checkpoint: 37 GiB free; no cache cleanup needed.
