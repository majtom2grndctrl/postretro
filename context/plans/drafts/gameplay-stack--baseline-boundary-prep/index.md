# Gameplay-Stack — Baseline & Boundary Prep

> Epic: `gameplay-stack-decomposition` (Milestone 1). First spec — the measurable
> spike before the large `postretro-sim`/`-netcode`/`-ai` cuts. Epic-level direction,
> global ACs, and the target graph live in the hub; this spec does not restate them.

## Goal

Stand up the warm-edit compile-time measurement harness, and extract the lowest leaf
of the gameplay decomposition — `postretro-combat-model` — by sinking the two type
families that today point *up* from the sim/scripting cluster into `netcode`. This
proves the extraction mechanics, inverts two of the cycle's cross-module edges to
down-edges, and fixes the reference numbers every later milestone must beat — all
behavior-preserving and low-risk, before any large module moves.

## Scope

### In scope
- A baseline + dev Cargo-config harness and a committed `baseline.md`, mirroring
  `context/plans/done/E19--baseline-and-cargo-config/`.
- New crate `postretro-combat-model` (leaf below the sim/scripting cluster; deps
  pinned in Task 2).
- Sinking the **shot-authority** family out of `netcode` into `postretro-combat-model`
  and re-pointing every consumer.
- Sinking the **carried-loadout** family out of `netcode` into
  `postretro-combat-model` and re-pointing every consumer.

### Out of scope
- Moving any large module (`sim`, `netcode`, `scripting`) — those are M2–M4.
- Severing the binary-only up-edges (`TICK_DURATION`, `InputMode`, `render` mover
  structs, `session`/`App`, `fx`-reactions) — M2 boundary-prep (hub `research.md`).
- Any combat-model-domain math in `postretro-combat-model`, or relocating existing
  combat logic into it — it holds only the sunk types now.
- Behavior, wire, or scripting-semantics change. Type relocation only.

## Direction

**Problem.** The fixed-tick gameplay cluster (`scripting` + `sim` + `netcode`) is one
cyclic unit inside the 216K bin-only crate: every edit recompiles the whole binary,
and the combat/AI churn locus (`scripting/systems/ai/`, 31 commits/90d) cannot be
isolated. The cause is two-fold — no gameplay crate boundary, and a
`scripting ↔ sim ↔ netcode` cycle that blocks lifting any one module (hub
`research.md`). Two of the cycle's edges are the sim/scripting cluster reaching *up*
into `netcode` for types that are really combat-model domain; sinking them into a leaf
below all three both removes those edges and creates the first extractable crate.

**Prior commitments.** `development_guide.md` §Target shape calls for a combat-model
crate (domain logic off the registry); this spec creates it as `postretro-combat-model`,
ahead of its full stat/damage
role, as the cycle-break sink (hub Decision 1). E19 established the extraction
conventions (`workspace.package` inheritance, `cargo tree` isolation gate,
behavior-preservation, `layering_invariants_hold`) this spec follows. Divergence from
the guide (adding `postretro-netcode`/`-ai`) is argued in the hub, not here — this
spec only builds the leaf both agree on.

**Alternatives rejected.** *One joint gameplay mega-crate* (whole cluster in one
crate): internalizes the cycle with near-zero cycle-breaking, but an AI edit still
recompiles netcode's 43K and all of sim — it never isolates its own churn, failing
the epic's core goal. *Start with the `postretro-sim` move directly*: the largest,
riskiest step, taken before any warm-edit number proves the boundary pays off — the
owner chose to measure first. This spec is that measurement plus the safe first cut.

## Acceptance criteria
Inherits the epic global acceptance criteria (hub).
- [ ] `baseline.md` exists in this plan folder: the host/toolchain block (`cargo -V`,
  `rustc -vV`, OS, target, `cargo metadata --locked`), the method (isolated
  `--target-dir` per case, prime-then-discard for warm cases), and recorded warm-edit
  numbers for three touch-rebuild cases — `scripting/systems/ai/targeting.rs`, a
  `netcode/` file, and a binary-only file (e.g. `startup/`) — captured on the tree at
  this spec's start.
- [ ] `postretro-combat-model` is a workspace member; `cargo build --workspace` and
  `cargo test --workspace` pass. `cargo tree -p postretro-combat-model` shows no
  `wgpu`/`winit`/`glyphon`/`mlua`/`rquickjs`.
- [ ] The shot-authority types (`AuthorizedShot`, `OpenAuthorizedShot`, `ShotId`,
  `HIT_RANGE_TOLERANCE`, `MAX_OPEN_SHOT_AGE_TICKS`, the timeout-budget helpers) are
  defined in `postretro-combat-model`. `sim` imports them from
  `postretro_combat_model`, not `crate::netcode`; `rg 'crate::netcode::(AuthorizedShot|OpenAuthorizedShot|ShotId)'`
  over `crates/postretro/src` is empty of production hits (a review/grep gate — it will not
  match a grouped `use crate::netcode::{…}`, so `cargo build --workspace` after removing the
  types from `netcode`, not re-exporting them, is the real guard).
- [ ] The carried-loadout types (`CarriedState`, `TuningPayload`,
  `WieldableTuningPayload`) are defined in `postretro-combat-model`. The
  `scripting/builtins/{net_descriptor,data_archetype,wieldable_inventory}.rs`
  consumers import them from `postretro_combat_model`; `rg
  '(crate::netcode|super)::(CarriedState|TuningPayload|WieldableTuningPayload)'` over
  `crates/postretro/src` is empty of production hits (a review/grep gate covering the
  `crate::netcode::` and `super::` forms; `cargo build --workspace` with no compatibility
  re-export left in `netcode` is the real guard).
- [ ] `netcode`'s own uses of the sunk types resolve to `postretro_combat_model`
  (a down-edge), and `netcode`'s replication/materialization tests pass unchanged.
- [ ] Behavior-preserving: the `sim` determinism tests and the `netcode` replication
  tests pass. Any sunk type that derives a replication codec keeps that derive and its
  field layout (Invariant 1).
- [ ] No net-new `unsafe` (grep/review gate). `layering_invariants_hold` stays green — it
  guards the existing invariants, not `postretro-combat-model`, so it passes without proving
  this extraction; the edge inversion is proven by `cargo build --workspace` (combat-model
  cannot name `crate::netcode`), and behavior by the determinism/replication tests above.

## Tasks

### Task 1: Baseline + dev Cargo-config harness
Reproduce the E19 measurement method (`context/plans/done/E19--baseline-and-cargo-config/`
is the template) for the gameplay tree. Capture a host/toolchain block and, using one
isolated `--target-dir` per case with warm cases primed by a full build whose time is
discarded, record touch-rebuild warm-edit timings (mtime-only `touch`, no content
change, then rebuild) for three files: `crates/postretro/src/scripting/systems/ai/targeting.rs`
(the churn locus), one `crates/postretro/src/netcode/` file, and one binary-only file
under `crates/postretro/src/startup/`. Also capture `cargo check --workspace` and a
warm no-op. Write it to `baseline.md` in this plan folder. If the E19 dev
`.cargo/config.toml` (opt-in `dev-fast` profile, commented-out linker stanzas) is
already committed, cite it and do not duplicate; if absent, add it comment-only so
default builds on every platform are unchanged. This task changes no source and is
independent of Tasks 2–4; run it against the pre-move tree so its numbers are the
clean M1 reference.

### Task 2: Scaffold `postretro-combat-model`
Create `crates/combat-model/` with a `Cargo.toml` (package `postretro-combat-model`,
`version.workspace = true` + the rest of the `[workspace.package]` inheritance, a
`description`) and `src/lib.rs`. Follow the inheritance *structure* of a minimal leaf
precedent (`crates/render-data/Cargo.toml`), not its dep set: this crate's deps are
`glam.workspace = true` (`AuthorizedShot.fire_origin: Vec3`), `serde.workspace = true`
(`TuningPayload` and `WieldableTuningPayload` derive `Serialize`/`Deserialize`),
`postretro-net` (`ShotId::from_parts` takes a `NetworkId`, declared in `postretro-net` —
a method signature, not a struct field, so a fields-only scan misses it), plus
`postretro-foundation` and `postretro-entities` (the moved fields name `EntityId`,
`AmmoReserve`, `WeaponPlacementDescriptor`, `WIELDABLE_SLOT_CAPACITY`). Confirm the final
set with `cargo build --workspace`; all four are clean down-edges below the cluster and
none is a `cargo tree` forbidden crate. Depend on `postretro-foundation`/`postretro-entities`
with default features only — do not enable `script-ffi`; if `cargo tree -p postretro-combat-model`
still shows mlua/rquickjs via workspace feature unification, the isolation holds as long as
combat-model itself never activates that feature. Add `"crates/combat-model"` to the workspace-root `members`
array, add `postretro-combat-model = { path = "crates/combat-model" }` to
`[workspace.dependencies]`, and add `postretro-combat-model = { workspace = true }` to
the `postretro` binary's `[dependencies]`. `lib.rs` declares and re-exports the two
modules the later tasks fill — `mod shot_authority; pub use shot_authority::*; mod
carried_loadout; pub use carried_loadout::*;` — and Task 2 creates empty `shot_authority.rs`
and `carried_loadout.rs` beside it. Task 3 and Task 4 each own one module file and neither
edits `lib.rs`. Blocks Tasks 3 and 4.

### Task 3: Sink the shot-authority family
Move `AuthorizedShot`, `OpenAuthorizedShot`, `ShotId`, `HIT_RANGE_TOLERANCE`,
`MAX_OPEN_SHOT_AGE_TICKS`, and the timeout-budget helpers (declared in
`crates/postretro/src/netcode/mod.rs`, ~`:456`–`:542`) into `postretro-combat-model`'s
`shot_authority.rs` (created by Task 2; `lib.rs` already re-exports it), widening each
moved item to `pub`. Keep `OpenAuthorizedShots` (the container in
`netcode/mod.rs`) netcode-side unless it too is named outside netcode — `sim/mod.rs`'s
`use crate::netcode` imports only `AuthorizedShot`, `OpenAuthorizedShot`, `ShotId`, so
the container likely stays; move
it only if a re-point requires it. Discover every consumer with bare-symbol greps
(`rg '\bAuthorizedShot\b'`, etc.) as well as `crate::netcode::`-qualified ones, and
re-point them to `postretro_combat_model::` — the known site is `sim/mod.rs`'s `use crate::netcode`;
`netcode`'s own uses become down-edges into combat-model. Do not change any field or
value. If `AuthorizedShot` or `ShotId` derives a replication codec (bitcode/serde),
the derive and field order move verbatim (Invariant 1). Lean on `cargo build
--workspace` as the completion gate, not grep alone.

### Task 4: Sink the carried-loadout family
Move `CarriedState` (`netcode/seat.rs`), `TuningPayload` and `WieldableTuningPayload`
(`netcode/tuning_payload.rs`) into `postretro-combat-model`'s `carried_loadout.rs`
(created by Task 2; `lib.rs` already re-exports it), widening each to `pub`.
`TuningPayload`'s inherent impl travels with it: its private `epoch` field,
`TuningPayload::new`, and the `TUNING_PAYLOAD_EPOCH` const `new` reads must all move
together — otherwise `new` reaches up into `netcode`, an illegal up-edge. Widen `new`
and any method `netcode` calls to `pub`; `epoch` stays private inside combat-model. The
JSON codec (`encode_tuning_payload`/`decode_tuning_payload`, `TuningPayloadError`) stays
in `netcode`, reading the moved types down. One netcode test (`payload_round_trips_…` in
`tuning_payload.rs`) constructs `TuningPayload` via a struct literal that sets the private
`epoch` and keeps `view_feel = Some` to prove `encode` clears it — it will not compile once
`epoch` is a foreign crate's private field. Split it: the construction moves into a
`postretro-combat-model` test (where `epoch` is reachable) and the codec assertion stays in
`netcode` operating on a value combat-model hands it, or give combat-model a test-only
constructor that sets `epoch` and retains `view_feel`. State which in the PR.

Discover every consumer with bare-symbol greps (`rg '\bCarriedState\b'`, `\bTuningPayload\b`,
`\bWieldableTuningPayload\b`) as well as the `crate::netcode::`- and `super::`-qualified forms —
the list is not exhaustive. Production consumers span
`scripting/builtins/{net_descriptor,data_archetype,wieldable_inventory}.rs`, `netcode`'s own
`lifecycle.rs`/`remote_materialize.rs` (via `super::`), `startup/lifecycle.rs`, `main.rs`, and
`netcode/{endpoint,host}.rs`. Re-point every form to `postretro_combat_model::` and leave no
compatibility re-export in `netcode`: drop the moved names from `netcode/mod.rs`'s
`pub(crate) use seat::{…}` and `use tuning_payload::{…}` re-export lines (keep the ones that
stay, e.g. `SeatTable`, `finish_host_poll`).
`descriptor_class` appears in `scripting` only in a doc comment in `data_archetype.rs`
— do not move it. `restore_carried_health` moves to `postretro-combat-model` with the
structs: it names only `CarriedState`/`EntityRegistry`/`EntityId` and calls `postretro_entities`'
`set_health_absolute` — all combat-model/`entities` types — so the `netcode/mod.rs` seat
re-export drops it too. No field or value change; preserve any replication derive and layout
(Invariant 1). `cargo build --workspace` is the completion gate.

## Sequencing

**Phase 1 (concurrent):** Task 1 (baseline — no source change, independent) · Task 2
(scaffold — file-disjoint from Task 1). Task 2 owns `lib.rs` and both module files, so
no later task edits `lib.rs`. Task 2 blocks Phase 2.
**Phase 2 (sequential):** Task 3 — sink shot-authority into its module file.
**Phase 3 (sequential):** Task 4 — sink carried-loadout. Sequential after Task 3, not
concurrent: both co-edit `netcode/lifecycle.rs` and the `netcode/mod.rs` re-export/import
region, so parallel worktrees would conflict there. Each fills its own combat-model module
file and re-points its own consumers, leaning on `cargo build --workspace` to surface any
missed site.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| 1. Replication wire layout of any sunk type is unchanged | the `serde` `Serialize`/`Deserialize` derives on `TuningPayload`/`WieldableTuningPayload` (their host tuning payload, carried opaquely as JSON by `postretro-net`) | Task 3 / Task 4 relocate a type — dropping a `serde` derive silently breaks that payload; no sunk type derives bitcode `Encode`/`Decode`, and `AuthorizedShot` is documented as never crossing the wire | AC "behavior-preserving": netcode replication + sim determinism tests pass; PR confirms which sunk types cross the wire |

## Rough sketch

`postretro-combat-model/src/lib.rs` declares two modules — `shot_authority` and
`carried_loadout` — re-exporting the moved types at the crate root so consumers write
`postretro_combat_model::AuthorizedShot`. The crate is a pure type leaf: no VM, no
wgpu, and no dependency on the binary's `netcode`/`sim` modules (it sits below them).
Its deps are pinned by the moved types, not guessed — `glam`, `serde`, `postretro-net`,
`postretro-entities`, `postretro-foundation` (Task 2), all clean down-edges. This
mirrors `E19--render-data` (a leaf that sank shared types — `Aabb`,
`LightInfluence` — below the crates that had reached across for them).

## Open questions

- `restore_carried_health` placement (Task 4) — resolved from source: it names only
  `CarriedState`/`EntityRegistry`/`EntityId` + entities' `set_health_absolute`, so it moves
  to `postretro-combat-model` with the structs. The hub's open questions record the same.
- Whether any sunk type crosses the replication wire — resolved from source.
  `TuningPayload` and `WieldableTuningPayload` derive `serde` `Serialize`/`Deserialize`
  and cross the wire as an opaque JSON payload, so the move preserves both derives;
  `CarriedState` and every shot-authority type derive no codec (`AuthorizedShot` is
  documented as never crossing the wire). This fixes Invariant 1's scope; the hub's
  open questions record the same resolution.
