# Heat + cell weapon resources — design contract

Every track reads this file before its brief. Worktree: `/Users/dhiester/Projects/Personal/postretro-heat-cell`, branch `feat/heat-cell-resources`. Another agent works in the main checkout; never touch `/Users/dhiester/Projects/Personal/postretro`.

## Goal

The weapon descriptor's `resource` union gains its two open variants, `heat` and `cell`, with a per-tick resource update that runs independently of fire: heat dissipates, cells regenerate. The dev mod's plasma rifle becomes the cell reference weapon; heat ships with test coverage only. Owner-facing HUD state follows the health pattern (raw value plus a companion max), replicates owner-private like the ammo values, and connected clients predict silent or dry-fire pulls from it.

## Decisions

| Decision | Consequence |
|---|---|
| Heat and cell settings live inside the `resource` block, like shipped ammo — not flat on the weapon block as `context/research/weapon-model.md` sketched. | One authoring placement for every variant's numbers; `effective()` projects them beside the flat stats so a later augment has somewhere to land. |
| Heat is a gate layered on the fire-rate cooldown, never a replacement. | A heat weapon still obeys `fireRateMs`; both gates must pass. |
| The shot that crosses `overheatAt` fires, then latches the weapon overheated. The latch clears only when heat reaches exactly 0. | Lockout length is `overheatAt / coolPerSecond`; authors tune it through the cooling rate. |
| Pulls while overheated are silent (`WeaponFireAuthorization::Rejected`), not dry fire. The crossing shot emits an `overheat` weapon event with a matching `overheat` sound key. | One cue per overheat rather than a click per pull. |
| An empty cell dry-fires (`Empty`), exactly as an empty magazine does, including resetting the cooldown. | The existing dry-fire presentation, sound key and client prediction apply unchanged. |
| Overheat is a latch on the heat resource, **not** a `WieldableState` variant. | Switching lowers the outgoing weapon, which would overwrite a state variant; a double quick-switch must not clear an overheat. It matches `entity_model.md`'s rule that fire authorization is computed without consulting weapon state — the latch is a resource gate, like the magazine check. |
| Every weapon instance updates its resource every fixed tick, wielded or holstered (owner decision). | Swapping away to cool a gun is a real tactic. This deliberately differs from `cooldown_remaining_ms`, which freezes on inactive instances; that behavior is unchanged. |
| Reload input does nothing on heat or cell weapons. | No reload path grows a heat or cell branch. |
| HUD slots follow the health pattern — raw value plus max — not 0–1 fractions (owner decision). | A modder builds a cell bar the way they build the health bar: `bind` the value, `max` the capacity slot. |
| Heat and cell have no reserve and no pickup interaction. | `AmmoReserve`, resource grants and duplicate-pickup policy are untouched. |

## Invariants

**Descriptor (`WeaponResource`, serde tag `kind`).** All numbers are `f32` and must be finite. Validation errors name the field as `components.weapon.resource.<field>`, like ammo.

| `kind` | Field | Required / default | Validity |
|---|---|---|---|
| `"heat"` | `heatPerShot` | required | `> 0` and `<= overheatAt` |
| | `overheatAt` | required | `> 0` |
| | `coolPerSecond` | required | `> 0` (zero would latch forever) |
| | `coolDelayMs` | default `0` | `>= 0` |
| | `overheatBehavior` | default `"lockout"` | closed enum `OverheatBehavior`; `"lockout"` is its only member. Unknown values are rejected. |
| `"cell"` | `capacity` | required | `> 0` |
| | `costPerShot` | required | `> 0` and `<= capacity` |
| | `regenPerSecond` | required | `>= 0` (zero is a non-recharging battery) |
| | `regenDelayMs` | default `0` | `>= 0` |

Units:
- Heat and charge are unitless author-scale numbers.
- Rates are units per **second**.
- Delays are **milliseconds**.

Field names are camelCase on the author surface. Rust mirrors them in snake_case: `heat_per_shot`, `overheat_at`, `cool_per_second`, `cool_delay_ms`, `capacity`, `cost_per_shot`, `regen_per_second`, `regen_delay_ms`.

**Runtime state (`WeaponComponent`).**
- At most one of the ammo, heat and cell resources is present, and construction from the descriptor guarantees it.
- Heat live state:
  - heat value, clamped to `0..=overheat_at`;
  - the `overheated` latch;
  - `idle_ms`, the milliseconds since the last accepted shot.
- Cell live state:
  - charge, clamped to `0..=capacity`;
  - `idle_ms`.
- Spawn state: heat 0 and not overheated; cell charge at capacity. `idle_ms` starts at 0 for both.
- Hot reload (`refresh_from_descriptor`):
  - Same kind: replace the tuning and keep the live values, clamping them into the new bounds.
  - Different kind: rebuild the resource fresh from the descriptor.
  - The overheated latch survives a same-kind reload.
- `effective()` projects the heat and cell tuning as `Option` members of `EffectiveStats`, beside the existing `ammo`.

**Tick order (authoritative path: host sim and single-player local, the one shared weapon machine).**
1. The resource update runs **once per fixed tick for every `WeaponComponent` that carries heat or cell**. That includes holstered instances and unowned world pickups. For the active weapon it runs **before** the fire gate in the same tick.
2. Update rule: add `dt_ms` to `idle_ms`, then apply the rate change for the full `dt` if either condition holds:
   - heat: `idle_ms >= cool_delay_ms` **or** the weapon is overheated (lockout ignores the delay);
   - cell: `idle_ms >= regen_delay_ms`.

   This is the same idle-then-apply shape as `tick_bloom`: no partial-tick splitting.
3. When heat reaches 0 during an update, the `overheated` latch clears.
4. On an accepted shot, after the cooldown and resource gates pass:
   - Heat: add `heat_per_shot`. If heat is now `>= overheat_at`, clamp it to `overheat_at`, set the latch, and emit `overheat`. Then set `idle_ms = 0`.
   - Cell: subtract `cost_per_shot`, then set `idle_ms = 0`.
5. Gate verdict, evaluated after the existing cooldown and wants-fire checks:
   - heat overheated → `Rejected`;
   - heat not overheated → `Accepted`, even when this shot will overshoot;
   - cell with `charge < cost_per_shot` → `Empty`.
6. Connected clients never run the resource update. They read the replicated values.

**`overheat` event.**
- It is emitted from the shooter through the same `WeaponFireEvents` emission route as `dry_fire`.
- `WeaponSounds` gains `overheat` (author key `overheat`), played at the firing pawn.
- On a connected client, the owner's cue comes from the rising edge of the replicated `player.overheated` flag for the active slot, following the pattern in `sound_events/client_reload.rs`.

**Engine state slots.** Every slot is `Readonly` and `persist: false`, and describes the sampled active weapon. The five value slots are `ReplicationScope::OwnerPrivatePlayer`. `player.weaponResource` is `ReplicationScope::None` (amended after Track A).

| Wire name | SDK path | Type | Default | Range | Wire shape |
|---|---|---|---|---|---|
| `player.weaponResource` | `player.weaponResource` | Enum `none` / `ammo` / `heat` / `cell` | `"none"` | — | not replicated; published locally on every role |
| `player.heat` | `player.heat` | Number | None | `0..inf` | WieldableSlotOptionalNumber |
| `player.overheatAt` | `player.overheatAt` | Number | None | `0..inf` | WieldableSlotOptionalNumber |
| `player.overheated` | `player.overheated` | Boolean | `false` | — | WieldableSlotBoolean |
| `player.cell` | `player.cell` | Number | None | `0..inf` | WieldableSlotOptionalNumber |
| `player.cellCapacity` | `player.cellCapacity` | Number | None | `0..inf` | WieldableSlotOptionalNumber |

**Kind slot amendment (after Track A).** A Plain owner-private slot falls back to the slot table's global value in `owner_private_source_value`. A remote client would therefore receive the host player's kind, not its own. `player.weaponResource` instead follows `player.weapon.current` and `player.spread`: every role publishes it locally, from its own active wieldable's `WeaponComponent`. Connected clients included. So a connected client's kind leads the host-correlated value slots by up to one round trip after a local switch, which is the same lag the weapon-name label already has.

**Value slot sourcing.** Each connected owner receives its own pawn's heat, cell and overheat values. They are sourced per pawn in `owner_private_source_value`, as `AmmoSlotProjection` does for the magazine, and never from the slot table's global value. When the host's active weapon has no heat, `player.heat` and `player.overheatAt` travel as the `[slot]` absence; the cell slots do the same. `player.overheated` travels as `[slot, 0]`.

Publish and absence rules mirror `player.ammo`:
- A live active weapon of another kind **clears** that kind's number slots and writes `player.overheated = false`. The kind slot is written for every live active weapon, and `"none"` means a resourceless weapon.
- With no pawn or no active weapon, nothing is written (the existing staleness contract).

**Client pull prediction** (extends `client_pull_presentation`). These are presentation only; a wrong guess costs a sound.
- Heat: `player.overheated` true for the active slot → `Silent`.
- Cell: `player.cell < cost_per_shot` for the active slot → `DryFire`.
- An absent value, or one that describes another slot → `Fire` (the existing rule).

## Tracks and file ownership

| Track | Model | Owns | Depends on |
|---|---|---|---|
| **A — authoritative core** | opus | `crates/foundation`, `crates/entities`, `crates/sim` (except `sim/src/weapon/client_pull.rs`), the local overheat sound in `crates/postretro/src/sound_events/descriptors.rs`, `sdk/types/*`, typedef fixtures | — |
| **B — connected client** | opus | `crates/netcode` wire shapes and projection, `crates/sim/src/weapon/client_pull.rs`, the connected-client overheat cue in `crates/postretro/src/sound_events/` | A merged |
| **Orchestrator** | — | `content/dev/**`, `docs/**`, `context/**` | A (and B for docs) |

Module layout: heat and cell land in new modules, not appended to large files:
- a descriptor module beside `types/combat.rs`;
- a component module beside `components/weapon.rs`;
- a per-tick update module under `sim/src/sim/weapon_stage/`.

`types/combat.rs` (2k lines) and `weapon.rs` (1.2k lines) grow only by the wiring lines.

Compile-forced spillover outside a track's files is allowed and must be reported. Example: struct-literal constructions of `WeaponComponent` or `EffectiveStats` in another crate's tests.

## Acceptance

Run every command from the worktree root. Each command must report a nonzero test count.

**Track A**
- `cargo test -p postretro-foundation --lib weapon` covers heat and cell serde defaults, and rejection of each invalid row in the descriptor table.
- `cargo test -p postretro-entities --lib weapon` covers:
  - construction and the at-most-one-resource rule;
  - hot-reload clamping, and the overheated latch surviving a same-kind reload;
  - the `effective()` projection.
- `cargo test -p postretro-sim --lib` with heat and cell filters covers:
  - the crossing shot fires, then locks out;
  - lockout pulls are silent, never dry fire;
  - lockout cools through `coolDelayMs` and clears only at 0;
  - a holstered instance cools and regenerates while another is active;
  - a cell below cost dry-fires and resets the cooldown;
  - the regen delay;
  - the `overheat` emission;
  - HUD slot publish and clear per kind.
- `cargo run -p postretro-sim --bin gen-script-types` is followed by `git status --short sdk/types`, which shows only the intended heat and cell additions. The typedef fixture tests pass.
- `cargo check --workspace --all-targets` passes.

**Track B**
- `cargo test -p postretro-netcode --lib state_slots` covers the new wire shapes, round-trips and absence.
- `cargo test -p postretro-sim --lib client_pull` covers overheated → Silent, a cell below cost → DryFire, and a mismatched slot → Fire.
- `cargo test -p postretro --bin postretro sound_events` covers the overheat rising edge; a held flag and a mixed-slot frame produce no cue.
- `cargo check --workspace --all-targets` passes.
- Two checks carried over from Track A: `cargo test -p postretro --bin postretro sound_events` covers the local `overheat` cue, and `cargo clippy -p postretro-foundation -p postretro-entities -p postretro-sim -p postretro-netcode --all-targets -- -D warnings` passes.

**Orchestrator**
- The plasma rifle descriptor loads (dev scripts build).
- The dev HUD shows a cell bar only while a cell weapon is wielded.
- Manual (visual, the owner): fire the plasma rifle until the cell is empty, hear the dry fire, watch it regenerate after the delay, and see the bar refill while holstered.

## Open

- Whether `overheatBehavior` grows `"vent"` (a manual early vent), and what it means. The enum is the seam; nothing else is built for it.

## Status at pause (2026-09-29)

Landed on the branch:
- **Track A** (`50077af65`): all of its gates are green except clippy and `sound_events`, both carried into Track B's gate.
- **Content, docs and context:** the plasma rifle is the cell reference, the dev HUD has a cell bar, `docs/scripting-reference.md` covers heat and cell, and `entity_model.md` and the router are updated. `tsc` shows no errors in the edited dev scripts. Ten errors already exist in seven other dev scripts.
- **Track B:** committed as WIP in `b5a4cad9d`. Resume in this order:
  1. Rerun `cargo test -p postretro-netcode --lib state_slots` to confirm a fixture edit that expects 14 slots, up from 9.
  2. `resource_projection::tests::a_malformed_heat_or_cell_sample_rejects_the_batch` fails: the client accepts `[slot, -1.0]` on `player.cell`. Check whether `apply_store_slot_batch` enforces readonly engine-slot ranges for correlated samples. If it doesn't, drop that row and report it; don't widen the change.
  3. Run the whole `cargo test -p postretro-netcode --lib`.
  4. Run `cargo test -p postretro --bin postretro sound_events`. This is the first compile of the `main.rs` wiring (`observe_client_weapon_edges`, `client_overheat_edge`).
  5. Update `context/lib/networking.md` §Combat authority:
     - the heat and cell owner-private values and their absences;
     - the pull rules: overheated → silent, a short cell → dry fire;
     - the overheat cue: the rising edge of the replicated latch for the wielded slot;
     - the resource kind is local on every role.
  6. Run `cargo check --workspace --all-targets`, then clippy with the command from Track B's acceptance.
  7. Squash the WIP commit.
- Then: one `opus` review pass, `/preflight`, and the PR. Manual visual proof is still owed.

Disk: run builds with `CARGO_INCREMENTAL=0`. After each step, if free space is under 15 GB, clear the worktree's incremental cache or `cargo clean -p` the PostRetro crates. On macOS, `timeout` isn't available.
