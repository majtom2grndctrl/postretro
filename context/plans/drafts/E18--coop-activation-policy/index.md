# E18-B — Co-op Activation Policy

## Goal

Grow the shipped trigger activation gate from its single any-player rule into a small closed policy vocabulary, so level designers can author co-op puzzle affordances that pull teammates together or gate on the host. A trigger picks its policy in TrenchBroom; the host-authoritative gate enforces it. Opens the co-op puzzle design space the E18 capstone needs. Design intent: `context/research/co-op-triggers-trap-pools.md` §4.3.

## Scope

### In scope

- `activation_policy` authored choice on `trigger_volume`, four values: `any` (default, today's per-edge behavior), `host_only`, `count` (with `activation_count`), `all`. FGD → compiler → PRL section v3 → component → bridge.
- `occupancy_includes_dead` authored boolean on `trigger_volume` (default off): whether a dead pawn counts as an occupant. Orthogonal to `activation_policy`.
- One unifying runtime predicate — **effective occupant** = spatial overlap ∧ (alive ∨ `occupancy_includes_dead`) — driving per-edge fires, threshold counts, and paired exits.
- Per-edge policies (`any`, `host_only`): filter the activator inside the existing gate. `host_only` fires only for the listen-server host's own pawn.
- Threshold policies (`count`, `all`): fire once on the rising edge of effective-occupancy meeting the threshold, and fire the paired exit once on the falling edge — a per-trigger transition, not a per-player edge.
- Alive filtering from player health, read inside the trigger stage (no sim-tick signature change).
- Dev-tools Triggers tab: policy, effective occupancy, and satisfied columns.

### Out of scope

- Cross-volume simultaneity ("each player on their own switch in a separate room, all at once"). That is a cross-volume AND, delivered by the companion spec E18-B2 via a counter slot + crossing — not a per-trigger policy.
- Frag-the-corpse-to-release-the-plate. `occupancy_includes_dead = on` makes a corpse hold a plate; *removing* the corpse by killing it needs damageable-corpse lifecycle (E18-C/entity lifecycle). This spec enables the mechanic, it does not deliver corpse despawn.
- Respawn timing and death re-arm (E18-R). B defines the effective-occupant predicate and reads alive state; when a respawn removes a pawn from a volume is E18-R's.
- Runtime scripting toggle of policy or `occupancy_includes_dead` (an arm/disarm-style primitive). Policy is authored map data in v1.
- Combining a per-edge policy with a threshold ("all, but only if the host is among them"). One policy per trigger.
- Exposing policy/armed/satisfaction through the `world.query` trigger snapshot — engine-owned, matching the E18-A decision to keep armed/phase unexposed.

## Acceptance criteria

- [ ] A map authoring `activation_policy = all` (or `count`/`host_only`) compiles; a pre-v3 `.prl` still loads with policy defaulting to `any`, `activation_count` to 1, `occupancy_includes_dead` off, and every E18-A trigger behavior unchanged.
- [ ] `any` policy reproduces E18-A behavior exactly: every effective-occupant rising edge fires per player, subject to the unchanged arm/latch/rearm gate (existing E18-A trigger tests pass unmodified).
- [ ] `host_only`: an enter edge from the host's own pawn fires; an enter edge from a connected remote pawn is suppressed; the host's paired exit still fires on leave.
- [ ] `count` with `activation_count = 2`: with two alive players overlapping, the trigger fires exactly once as the second arrives (not at one occupant); a single paired exit fires once effective occupancy drops below two; a third arrival does not re-fire.
- [ ] `all` with two connected players: fires once when both alive players occupy simultaneously; does not fire with one; paired exit fires once when either leaves. With zero alive players the trigger never fires (not vacuously satisfied).
- [ ] `occupancy_includes_dead = off` (default): a player who dies while standing on a `count`/`all`/hold plate is dropped from effective occupancy — its threshold falls and its paired exit fires as if the player left.
- [ ] `occupancy_includes_dead = on`: a dead pawn overlapping the volume continues to count toward occupancy and keeps a threshold satisfied; a solo dead pawn on an `any` plate that fired while alive holds its paired exit until the body physically leaves the volume.
- [ ] A threshold fire is attributed to a deterministic activator (the lowest effective-occupant `PlayerId`), and the tick's event stream stays ordered by `(trigger, player)` — two identical headless runs produce identical fire sequences.
- [ ] With `--features dev-tools`, the Triggers tab shows each trigger's policy, effective occupancy, and satisfied state; without the feature no trigger UI compiles in.
- [ ] Determinism harness: a headless tick sequence exercising a `count` and an `all` trigger produces identical fire/exit streams across two runs.

## Tasks

### Task 1: Deliver `activation_policy`, `activation_count`, `occupancy_includes_dead` from FGD to component

End-to-end field delivery mirroring E18-A's `on_fire`/`on_exit` addition. **FGD** (`sdk/TrenchBroom/postretro.fgd` `trigger_volume` entry, currently lines 318–351): add `activation_policy(choices)` default `"any"` with values `any`/`host_only`/`count`/`all`; `activation_count(int)` default `1` (used only by `count`); `occupancy_includes_dead(choices)` `0`/`1` default `0`, description naming the corpse-holds-plate semantics. **Compiler** (`crates/level-compiler/src/trigger_volumes.rs` `resolve_trigger_volume`): parse `activation_policy` string→u8 (`any|0`→0, `host_only|1`→1, `count|2`→2, `all|3`→3; unknown bails, matching the `activation`/`fire_mode` precedent at lines 23–34, 59–63); parse `activation_count` u32 (default 1; bail if policy is `count` and count < 1); parse `occupancy_includes_dead` 0/1→bool default false (mirror `enabled_on_spawn` at 75–85). Add the three fields to `MapTriggerVolume` (`crates/level-compiler/src/map_data.rs:177`) and copy them through `encode_trigger_volumes_section`. **Wire** (`crates/level-format/src/trigger_volumes.rs`): append `activation_policy: u8`, `activation_count: u32`, `occupancy_includes_dead: bool` to `TriggerVolumeRecord` after `on_exit`; bump `TRIGGER_VOLUMES_VERSION` 2→3; add a `has_activation_policy` decode branch (`3` reads the three fields, `1`/`2` default them to `0`/`1`/`false`); keep the per-version trailing-bytes check; validate `activation_policy > 3` rejects on decode, mirroring the `activation`/`command`/`fire_mode` range checks. Update the round-trip test and add a v2-decode test. **Component** (`crates/entities/src/components/trigger_volume.rs`): add `pub enum TriggerActivationPolicy { Any, HostOnly, Count(u32), All }` (serde `snake_case`, count embedded), `#[serde(default)]` to `Any`; add `pub activation_policy: TriggerActivationPolicy` and `pub occupancy_includes_dead: bool` (both `#[serde(default)]`) to `TriggerVolumeComponent`; extend `new(...)` with the two params (now ten args; the existing `#[allow(clippy::too_many_arguments)]` covers it). Enumerate and update every `new(...)` caller: the bridge (Task-1 same file), and the two `#[cfg(test)]` constructors in `crates/postretro/src/trigger_system.rs` (`spawn_trigger` helper ~line 683 and the sequenced-primitive test ~line 1644). **Bridge** (`crates/postretro/src/scripting/systems/trigger_volume_bridge.rs` `populate_from_level`): map `record.activation_policy` u8 + `record.activation_count` u32 → `TriggerActivationPolicy` (count only for variant 2) and pass it plus `record.occupancy_includes_dead` into `new(...)`.

### Task 2: Effective occupancy, per-edge policy filter, threshold policies

Runtime work in `crates/postretro/src/trigger_system.rs` (536 lines, under the split threshold). **Alive set:** extend `canonical_player_capsules` (line 317) to also read `HealthComponent` per pawn and record alive per `PlayerId`, where alive = health component absent **or** `current > 0.0 && current.is_finite()` (the death predicate used by `crates/postretro/src/scripting/systems/health.rs`). Return the alive set alongside the capsule map. **Effective occupancy:** in the per-trigger loop (lines 192–235), replace raw `overlapping` with `effective_present = overlapping && (alive.contains(player) || trigger.occupancy_includes_dead)` at both the leaving filter and the entering check, so `occupants[trigger]` holds effective occupants; `occupancy()` (line 109) then reports effective count (the E18-A occupancy test spawns players without health, so alive⇒effective==spatial and it still passes). **Per-edge policies:** in `evaluate_trigger_activation` (line 456), after the existing arm/latch/rearm gate, add the policy filter: `Any`⇒fire; `HostOnly`⇒fire only if `matches!(activator, PlayerId::Local(_))`; `Count`/`All`⇒never fire on the per-player path (handled by the threshold path below). **Threshold path:** for a trigger whose policy is `Count`/`All`, bypass per-player enter/exit edges and instead compute the effective-occupancy count, compare it to a new per-trigger satisfaction latch `satisfied: BTreeMap<EntityId, bool>` on `TriggerSystem` (alongside `occupants`/`paired_enters` at lines 96–98), and emit exactly one synthetic edge on a transition: rising (unsatisfied→satisfied, threshold = `activation_count` for `Count`, or the current alive-player count and ≥1 for `All`) runs the arm/latch/rearm gate and fires `on_fire`; falling (satisfied→unsatisfied) fires the paired `on_exit`. Attribute both synthetic edges to the lowest effective-occupant `PlayerId` (deterministic; document the convention). Keep the whole fire stream ordered by `(trigger, player)` and the gate the sole enter-fire path. Extend the `#[cfg(test)]` gate-fire/paired-exit recorders and add tests per the AC list (per-edge `host_only`, `count`, `all`, death-drops-occupancy on/off, zero-alive `all`, determinism).

### Task 3: Dev-tools policy + satisfaction columns

Add policy and satisfaction to the Triggers diagnostics tab. Engine side (`crates/postretro/src/trigger_diagnostics.rs` `collect_trigger_diagnostics_rows`, lines 21–59): populate new row fields — a policy label string (e.g. `all`, `count(2)`, `host_only`, `any`) from the component, and a `satisfied` bool from a new `TriggerSystem` accessor exposing the Task-2 satisfaction latch (per-edge policies report satisfied = occupancy > 0). Renderer side (`crates/renderer/src/render/debug_ui/mod.rs`): add `activation_policy: String` and `satisfied: bool` to `TriggerDiagnosticsRow` (lines 97–110); bump `draw_triggers_tab` `num_columns` (line 709 ff.) and add the header + body cells. Update the diagnostics row test. Everything compiles out without `--features dev-tools`.

## Sequencing

**Phase 1 (sequential):** Task 1 — field delivery; Tasks 2 and 3 consume the component fields.
**Phase 2 (sequential):** Task 2 — runtime policy + effective occupancy; Task 3 reads its satisfaction accessor.
**Phase 3 (sequential):** Task 3 — dev-tools, consuming Task 2's satisfaction accessor.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| activation policy | `TriggerActivationPolicy{Any,HostOnly,Count(u32),All}` | `activation_policy: u8` (0..=3) + `activation_count: u32`; serde `snake_case`, default `any` | n/a (authored in map) | n/a | `activation_policy` + `activation_count` |
| dead occupants | `TriggerVolumeComponent::occupancy_includes_dead: bool` | `occupancy_includes_dead: bool` (serde default false) | n/a | n/a | `occupancy_includes_dead` (0/1) |

The threshold count rides inside `Count(u32)` in Rust but stores as a separate `u32` on the wire; it is ignored for non-`count` policies on decode. Enum discriminant order is pinned by the wire u8 mapping.

## Wire format

`TriggerVolumesSection` (SectionId 44) v2→v3: after `on_exit`, append `activation_policy` (`u8`), `activation_count` (`u32` LE), `occupancy_includes_dead` (`u8` bool as `enabled_on_spawn` already encodes). `TRIGGER_VOLUMES_VERSION` bumps to 3; decoder accepts v1/v2 (three fields default `0`/`1`/`false`) and v3, rejects others; `activation_policy > 3` rejects; trailing-bytes check enforced per version. Mirrors the section's existing hand-rolled LE cursor codec — no new patterns.

## Script syntax examples

Authoring is map-side (KVPs on the `trigger_volume` brush), not scripted. A "huddle together" plate and a "leave your corpse" plate:

```
// trigger_volume brush A — both alive players must stand together to open the gate
activation_policy = all
on_fire = "openGate"
on_exit = "closeGate"

// trigger_volume brush B — a body (or a living player) held on the pad keeps the bridge extended
activation_policy = count
activation_count = 1
occupancy_includes_dead = 1
on_fire = "extendBridge"
on_exit = "retractBridge"
```

The `openGate`/`extendBridge` reactions are authored exactly as in E18-A (mover commands, `setState`, etc.); this spec only changes *when* the trigger decides to fire them.

## Open questions

None blocking. Decisions pinned rather than left open:

- Threshold fires are attributed to the lowest current effective-occupant `PlayerId` — deterministic, and the value is only observed by paired-exit bookkeeping and dev logs, never by reactions (reaction args are load-time-fixed; activator parameterization stays out of scope per E18-A).
- `host_only` on a future dedicated server (E15 Phase 4) has no `PlayerId::Local` pawn and is therefore inert there; documented, not designed for, until that server exists.
- A runtime toggle of policy / `occupancy_includes_dead` (an arm/disarm-style reaction primitive) is deferrable without breaking this design — the fields are plain component state a later primitive can mutate.
- The frag-the-corpse-to-release mechanic composes from `occupancy_includes_dead = on` plus damageable-corpse despawn owned by entity lifecycle (E18-C); B ships the occupancy half only.
