# Research — Per-Player Currency

Findings behind the spec's decisions, including why its shape changed once.

## Code grounding

| Claim | Source |
|---|---|
| The owner-private replication scope exists and its enum names are pinned; only the *mod-facing declaration* was withheld | `crates/entities/src/slot_table.rs`; `context/plans/done/M15--p35-state-slot-replication/index.md` — allow mod `network: "shared"`, "Reject public `network: \"ownerPrivate\"` for mod stores until a per-player authoring namespace exists" |
| The wire tracker already keys replicated values by slot and owner, and the owner-private resolver dispatches per-owner projections before a global fall-through | `crates/postretro/src/netcode/state_slots.rs` — `owner_private_source_value` |
| A session player identity already exists and outlives nothing durable — a local pawn id or a remote client id | `crates/postretro/src/trigger_system.rs` — `PlayerId::{Local, Remote}` |
| The save document is a flat slot-name-to-value map at version 1 — no per-player section, no save slots | `crates/postretro/src/scripting/state_persistence.rs` — `PersistedState`, `collect_persisted_state` |
| No profile, account, or cloud identity exists anywhere in the tree | searched `crates/` for profile/account/save-slot concepts; only a GPU render profile matches |
| `slot.add` today rejects any target and lowers to a self-referential add on a global slot | `crates/postretro/src/impact_policy.rs` — `bind_effect` |
| The HUD publisher republishes player slots each frame from local state, skipping rather than resetting when a source is absent | `crates/postretro/src/scripting/systems/ui_proxy.rs` |
| The activators-or-tag dual is shipped on a damage builder — the shape the reaction write path copies | `sdk/lib/data_script.ts` |

## Why the shape changed

The first draft backed a per-player slot with a per-entity state field on the
owning pawn. `/validate-plan` returned **under-scoped**, and the diagnosis
survived checking. Three consequences all fell out of that one placement:

- **Could not persist.** Per-entity state dies with the level; the spec had to
  scope persistence out, contradicting `combat-events.md`'s own `persist: true`
  XP sketch.
- **Could only be earned by dealing damage.** `EntityStateComponent` has exactly
  one write site in the tree — inside an impact policy. No trigger volume, no
  crossing, no level-load seed could award a currency.
- **Needed a second declaration spelling later.** Fusing cardinality with backing
  into one `perPlayer: "<field>"` key meant a per-player slot that was *not*
  pawn-backed would need a different spelling for the same replication scope —
  when Phase 3.5 had already reserved `network: "ownerPrivate"` for exactly this.

The owner-instanced store shape resolves all three by putting cardinality where
the concept lives. It also removes a cross-epic collision: the first draft
proposed overturning `scripting.md` §11's same-entity write seam, which
`E10--enemy-aggro-model` (in-progress) records in its own durable-decisions
table and is actively building on. Nothing in the current shape touches that
seam.

## Source-addressed per-entity state — checked for consumers, found none

The first draft's Task 1 (letting an impact policy write per-entity state on the
*source*) was pulled. Before pulling it, the plausible consumers were checked:

| Candidate | Needs it? |
|---|---|
| `E10--behavior-state-graph` (shipped) | No — built over the keystone using target-addressed state, and shipped without it. The roadmap called this the keystone's second consumer. |
| `E10--enemy-stagger` (draft) | No — the engine writes the stagger pulse itself, on the *damaged* entity. |
| `E10--enemy-multi-attack` (draft) | No. |
| `E10--enemy-aggro-model` (in-progress) | Cut a candidate-scoped `@state.*` filter pending this seam, and named two possible seams — a write path outside impact policies, *or* candidate-scoped reach into `defineStore` slots. It favored the latter for "persistent per-player stats (faction standing, mission flags)," warning: "Pick that seam before adding the leaves; building the `EntityStateComponent` arm now would guess it." |

So: no clear consumer, and the one candidate explicitly warned against building it
speculatively. Worth noting for whoever resumes the aggro plan — its favored seam
gets materially more attractive once per-owner store slots exist, since faction
standing is a persisted per-player value, which is what this spec makes
declarable.

## Device-local persistence — what it settles and what it opens

Progression saves to each player's own device, so the save file *is* the player
identity and no durable account concept is needed. Single-player, the dominant
case, has one owner and branches nowhere.

The one thing it opens is the join seed. Reward policy evaluates host-side, so a
policy reading a currency ("double past level 10") needs the guest's total in the
session — which means the value flows client to host at join, inverting the
host-authoritative direction Phase 3.5 established. The considered alternative —
host issues reward deltas, client accumulates its own total, nothing uploads —
has a strictly better trust story but makes a currency unreadable from policy,
which is a real expressiveness loss for a threat model (a player editing their
own save, among friends) that does not warrant it.
