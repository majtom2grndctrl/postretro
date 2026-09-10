# Gameplay-Stack Decomposition — research

> Source-grounded findings behind the epic. Not the spec contract. Point-in-time
> snapshot (HEAD `claude/gallant-maxwell-anr5xv`); every landed extraction moves
> files, so re-ground each spec against the live tree before building it.

## Why the largest binary modules resist extraction

The `postretro` binary is ~216K LOC, bin-only. Largest modules:

| Module | LOC | 90d commits | 90d lines | Inbound sites/files |
|---|---|---|---|---|
| `scripting/` | 60,295 | 60 (highest) | ~69K | 295 / 58 |
| `netcode/` | 43,443 | 23 (lowest) | ~44K | 58 / 18 |
| `sim/` | 19,299 | 28 | ~20K | 49 / 21 |
| `main.rs` | 13,556 | 32 | — | crate root (not extractable) |

`scripting/`, `netcode/`, `sim/` form a **mutual cycle** — all six directed edges
live in production code:

| edge | prod refs | edge | prod refs |
|---|---|---|---|
| scripting → sim | 11 | sim → scripting | 6 |
| scripting → netcode | (carried-loadout, below) | sim → netcode | 18 (shot-authority) |
| netcode → sim | 18 | netcode → scripting | 20 |

Cargo forbids cross-crate cycles. A single crate may hold the cycle internally; it
only must not point *up* into the binary. So no one of the three becomes its own
crate without either co-moving the others or sinking the shared types down.

`scripting/systems/` (re-rooted as `scripting_systems` via `#[path]` in `main.rs`)
is **not** a self-contained leaf: production code under it imports `crate::sim`,
`crate::weapon`, `crate::collision`, the `crate::scripting::` parent
(`primitives::store`, `reactions`, `builtins`), plus `nav`, `agent_steering`,
`kinematic_mover`, `combat_positioning`, `frame_timing`, `input`, `impact_effects`,
`trigger_system`. Its items are ~313 `pub(crate)` — never built as a crate boundary.

## Combat/AI is scripting-runtime fusion, and it is the churn locus

Combat/AI in the binary ≈ 28K LOC, ~14K inside the scripting tree:

| Combat / AI code | LOC | 90d commits | Location |
|---|---|---|---|
| AI cluster | 6,342 | **31** | `scripting/systems/ai/` |
| Hit zones | 3,722 | 2 (stable) | `scripting/systems/hit_zones.rs` |
| Reaction dispatch + scheduler | 3,370 | — | `scripting/systems/{system_reactions,reaction_scheduler}.rs` |
| Health | ~980 | — | `scripting/systems/health.rs` + `health/` |
| Weapon fire stage | 6,614 | — | `sim/weapon_stage.rs` + `sim/weapon_stage/` |
| Weapon module | 3,441 | 13 | `weapon/` |
| Impact policy | 3,289 | 2 (stable) | `impact_policy.rs` |
| Combat positioning | 996 | — | `combat_positioning.rs` |

`scripting/systems/ai/` is 31 of scripting's 60 commits/90d — the real churn
engine. `impact_policy.rs` and `hit_zones.rs` are large but **stable** (2 commits
each): isolating them buys nothing. Recent commit subjects confirm the driver —
`feat(ai)`/`fix(ai)`/`fix(combat)` (faction crossfire, combat perception,
retaliation, damage ledgers).

**AI is inseparable from the scripting runtime today.** `scripting/systems/ai/`
imports the VM host directly: `scripting::primitives::store::{read,write}_store_slot`,
`scripting::reactions::dispatch_*`, `scripting::builtins::{data_archetype,net_descriptor}`
(`ai/brain_programs.rs:250`). The host reaches back down to register
`scripting_systems::{ai,hit_zones,system_reactions}`. AI reads/writes the script
store, dispatches reactions, resolves data archetypes — it *is* the runtime. A
standalone AI crate requires decomposing the VM runtime (sink store + reaction
dispatch to `scripting-core`, invert the `sim ↔ AiRuntime` seam behind a trait,
invert the host→systems registration). That is milestone M4, not a near-term lift.

## The `simulate` seam is extraction-ready and data-injected

`sim::simulate_tick` / `simulate_tick_with_presentation_aim` (`sim/mod.rs:391,447`)
take everything by parameter — `registry`, `&CollisionWorld`, `&HitZoneStore`,
`Option<&NavGraph>`, `&[MoverCollider]`, `TriggerTickContext` — and invert caller
work through closures (`post_movement`, `on_impact`, `ingest_ready_remote_hits`).
The body's only in-binary up-call is `crate::impact_effects::tick_deferred_effects`
(`sim/mod.rs:483`) — inside the cluster. The seam calls **nothing** in
`input`/`frame_timing`/`render`. The shipped `context/plans/done/M15--p0-headless-sim-seam/`
began this formalization. Its one concrete coupling to AI is a `&mut
scripting_systems::ai::AiRuntime` parameter (`sim/mod.rs:400,457`) — intra-crate
today; a trait if AI ever leaves (M4).

## Up-edges: the fused cluster is nearly self-contained

Everything the whole gameplay cluster reaches *up* into that would stay binary-only,
production refs (test-only edges excluded):

| up-edge | prod refs | what it is |
|---|---|---|
| `frame_timing::TICK_DURATION` | ~4 | one const (`reaction_scheduler.rs:84`, `particle_render.rs:207`) |
| `input::InputMode` | 1 | one enum (`scripting/systems/input_mode.rs:9`) |
| `render::{KinematicMoverInstance, MoverOccluderAabb}` | 1 | presentation structs (`runtime_movers.rs:21`) |
| `session::{Session, evaluate_pending_in_tick_impacts}` | 3 | `netcode/endpoint.rs:505`, `impact_policy.rs:2070,2218` |
| `crate::App` | 3 | `netcode/endpoint.rs:377`, `netcode/state_slots.rs:3359-3362` |
| `fx::{emitter,fog}_reactions` | 4 | re-exported by `scripting/reactions/mod.rs:17-18` |
| audio, camera, view_feel, startup, observability, capture, options | 0 | — |

`entities` already stores the combat state (`components/{health,weapon,brain,agent,
projectile,ammo_reserve,wieldable_state,grant,inventory}.rs`); `scripting-core`
already hosts `reaction_dispatch.rs`.

## The two up-edge families that sink into `postretro-combat-model`

Both point from the sim/scripting cluster *up* into `netcode`. They are combat-model
domain (damage range, loadout authority), so sinking them to a leaf below all three
inverts the cycle edge and is behavior-preserving.

**Shot-authority** — declared in `netcode/mod.rs`; imported by `sim/mod.rs:23`:
`AuthorizedShot` (`:484`, carries `fire_origin: Vec3` → cannot live in glam-free
`net`), `OpenAuthorizedShot` (`:536`), `OpenAuthorizedShots` (`:542`, the container —
may stay netcode-side), `ShotId` (`:456`), `HIT_RANGE_TOLERANCE` (`:476`),
`MAX_OPEN_SHOT_AGE_TICKS` (`:477`), and the timeout-budget helpers (`:522,532`).

**Carried-loadout** — declared in `netcode`; imported by
`scripting/builtins/{net_descriptor,data_archetype,wieldable_inventory}.rs`:
`CarriedState`, `TuningPayload`, `WieldableTuningPayload`, `restore_carried_health`.
`descriptor_class` appears in scripting only as a doc comment
(`data_archetype.rs:276`) — not a code edge, does not sink.

**Wire risk to confirm at build:** whether any carried-loadout type derives
bitcode/serde for replication. If it crosses the wire, the move must preserve the
`Encode`/`Decode` derive and field layout (`networking.md` wire-mirror contract).

## Existing crate sizes (the floors the cluster sits above)

`scripting-core` 44,426 · `entities` 16,662 · `net` 11,263 · `foundation` 8,383 ·
`script-compiler` 3,446. The binary's `scripting/`/`netcode/` are the higher
runtime layers on top of these — not code misfiled out of them.

## Prior art — the E19 render-stack decomposition

`context/plans/drafts/E19--render-stack-decomposition/` (hub) + `context/plans/done/E19--*`
(mostly shipped) extracted the render/CPU/GPU stack into a one-way crate graph. This
epic mirrors its playbook: baseline-first, boundary-prep, one crate at a time in
dependency order, re-grounded per spec; correctness by construction (`cargo tree`
isolation, acyclicity-by-compile, behavior-preservation), not reviewer trust. The
`layering_invariants_hold` test fails `cargo test` on an upward edge or widened
chokepoint. `context/lib/development_guide.md` §Target shape names the `postretro-sim`
crate (collision moves in) and a combat-model crate as the documented target; this
epic realizes both and adds `postretro-netcode` and (M4) `postretro-ai`.
