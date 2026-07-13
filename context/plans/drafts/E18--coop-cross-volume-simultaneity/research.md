# Code-grounding notes (2026-07)

Source-verified facts for the puzzle fixture. The `incrementState`/`decrementState` primitive mechanism is grounded in `E18--state-delta-reactions/research.md`; this file covers only what the fixture composes.

## Crossing math (the fixture is correct)

`build_crossing` (`crates/entities/src/data_descriptors/validate/entities.rs`) stores `threshold: above / max` (line 45; `below / max` line 35); `max` defaults to 1.0 (line 21). The watcher normalizes the observed value as `raw / max` (`crates/scripting-core/src/state_crossings.rs:64,82`) and Above fires on `prev <= threshold && cur > threshold` (`:122`). So `above: 2.5, max: 3` → threshold 0.833; counter 3 → 1.0 fires, counter 2 → 0.667 does not. The authored `above` is a **raw count**, divided by `max` at build time — not a pre-normalized fraction.

## State declaration

`setupLevel` returns `LevelManifest { reactions, crossings, uiTrees }` (`sdk/lib/data_script.ts:84-92`) — **no `state` field**. Slots declare via `defineStore` (`data_script.ts:273`) → `StoreDeclaration { namespace, schema }` (`:97`), per-slot `StoreSlotSchema` (`:95`). Shared-global is `network: "shared"` on the slot → `ReplicationScope::SharedGlobal` (`store_bridge.rs:462`).

## Client converge / mover authority (why solve is host-side)

- `ClientStateApply::apply_snapshot_state` (`crates/postretro/src/netcode/state_slots.rs:674`, struct `:617`) is the P3.5 client converge path for the replicated counter.
- `networking.md:298`: only the host evaluates trigger overlap and fires commands; commands mutate replicated mover phase and clients reconcile "without ever evaluating the trigger locally." Crossing detection runs on every peer over the replicated slot table, so each client's crossing **does** fire `solve` locally — but a client-side mover-start is inert (clients never author movers; they reconcile replicated phase). The vault opens via replication; the redundant local fire is a harmless no-op. Late-join replays the crossing once from baseline — also inert.

## Plate re-arm requires `fire_mode = multiple`

A `once` plate latches after its first enter and never re-increments; the decrement-on-exit / re-increment-on-re-enter cycle needs `fire_mode = multiple`. `on_exit` fires regardless of `fire_mode` (E18-A paired-exit), so the decrement always lands; only the re-increment needs `multiple`.
