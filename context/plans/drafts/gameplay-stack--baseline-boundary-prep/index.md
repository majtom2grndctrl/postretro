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
- New crate `postretro-combat-model` (leaf over `entities`/`foundation`, `glam`).
- Sinking the **shot-authority** family out of `netcode` into `postretro-combat-model`
  and re-pointing every consumer.
- Sinking the **carried-loadout** family out of `netcode` into
  `postretro-combat-model` and re-pointing every consumer.

### Out of scope
- Moving any large module (`sim`, `netcode`, `scripting`) — those are M2–M4.
- Severing the binary-only up-edges (`TICK_DURATION`, `InputMode`, `render` mover
  structs, `session`/`App`, `fx`-reactions) — M2 boundary-prep (hub `research.md`).
- Any combat *feature* math in `postretro-combat-model` — it holds only the sunk
  types now; Epic 16 grows it later.
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

**Prior commitments.** `development_guide.md` §Target shape names the
`postretro-combat-model` crate; this spec creates it, ahead of its full stat/damage
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
  over `crates/postretro/src` is empty of production hits.
- [ ] The carried-loadout types (`CarriedState`, `TuningPayload`,
  `WieldableTuningPayload`) are defined in `postretro-combat-model`. The
  `scripting/builtins/{net_descriptor,data_archetype,wieldable_inventory}.rs`
  consumers import them from `postretro_combat_model`; `rg
  'crate::netcode::(CarriedState|TuningPayload|WieldableTuningPayload)'` over
  `crates/postretro/src` is empty of production hits.
- [ ] `netcode`'s own uses of the sunk types resolve to `postretro_combat_model`
  (a down-edge), and `netcode`'s replication/materialization tests pass unchanged.
- [ ] Behavior-preserving: the `sim` determinism tests and the `netcode` replication
  tests pass. Any sunk type that derives a replication codec keeps that derive and its
  field layout (Invariant 1).
- [ ] No net-new `unsafe`; `layering_invariants_hold` passes.

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
`glam.workspace = true` (the shot-authority `fire_origin: Vec3`), plus
`postretro-foundation` and `postretro-entities` as the moved types require (confirm
the exact set against the types' fields at build — e.g. an `EntityId`/component
handle pulls `entities`). Add `"crates/combat-model"` to the workspace-root `members`
array, add `postretro-combat-model = { path = "crates/combat-model" }` to
`[workspace.dependencies]`, and add `postretro-combat-model = { workspace = true }` to
the `postretro` binary's `[dependencies]`. `lib.rs` starts empty (module decls added
by Tasks 3–4). Blocks Tasks 3 and 4.

### Task 3: Sink the shot-authority family
Move `AuthorizedShot`, `OpenAuthorizedShot`, `ShotId`, `HIT_RANGE_TOLERANCE`,
`MAX_OPEN_SHOT_AGE_TICKS`, and the timeout-budget helpers (declared in
`crates/postretro/src/netcode/mod.rs`, ~`:456`–`:542`) into `postretro-combat-model`,
widening each moved item to `pub`. Keep `OpenAuthorizedShots` (the container, `mod.rs:542`)
netcode-side unless it too is named outside netcode — `sim/mod.rs:23` imports only
`AuthorizedShot`, `OpenAuthorizedShot`, `ShotId`, so the container likely stays; move
it only if a re-point requires it. Discover every consumer with bare-symbol greps
(`rg '\bAuthorizedShot\b'`, etc.) as well as `crate::netcode::`-qualified ones, and
re-point them to `postretro_combat_model::` — the known site is `sim/mod.rs:23`;
`netcode`'s own uses become down-edges into combat-model. Do not change any field or
value. If `AuthorizedShot` or `ShotId` derives a replication codec (bitcode/serde),
the derive and field order move verbatim (Invariant 1). Lean on `cargo build
--workspace` as the completion gate, not grep alone.

### Task 4: Sink the carried-loadout family
Move `CarriedState`, `TuningPayload`, `WieldableTuningPayload` (declared in `netcode`)
into `postretro-combat-model`, widening each to `pub`. The production consumers are
`crates/postretro/src/scripting/builtins/{net_descriptor.rs:20, data_archetype.rs,
wieldable_inventory.rs:17}` (they `use crate::netcode::{…}` today) plus `netcode`'s own
`lifecycle.rs`/`remote_materialize.rs`; re-point all to `postretro_combat_model::`.
`descriptor_class` appears in `scripting` only in a doc comment
(`data_archetype.rs:276`) — do not move it. Decide `restore_carried_health`'s home:
it is behavior over the registry consumed at `net_descriptor.rs:94` and
`data_archetype.rs:1022` — move it to `postretro-combat-model` with the structs if it
names only combat-model/`entities` types, else leave it in `netcode` reading the
structs down; state which in the PR. No field or value change; preserve any
replication derive and layout (Invariant 1). `cargo build --workspace` is the
completion gate.

## Sequencing

**Phase 1 (concurrent):** Task 1 (baseline — no source change, independent) · Task 2
(scaffold — file-disjoint from Task 1). Task 2 blocks Phase 2.
**Phase 2 (concurrent):** Task 3 · Task 4 — both need the crate from Task 2; they
touch different type sets, files, and consumers, so they parallelize. Each re-points
its own consumers and relies on `cargo build --workspace` to surface any missed site.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| 1. Replication wire layout of any sunk type is unchanged | pre-existing codec derives on the moved types | Task 3 / Task 4 relocate the type — a dropped or re-ordered `Encode`/`Decode` derive silently changes the wire | AC "behavior-preserving": netcode replication + sim determinism tests pass; PR confirms which sunk types cross the wire |

## Rough sketch

`postretro-combat-model/src/lib.rs` declares two modules — `shot_authority` and
`carried_loadout` — re-exporting the moved types at the crate root so consumers write
`postretro_combat_model::AuthorizedShot`. The crate is a pure type leaf: no VM, no
wgpu, no netcode/sim dependency (it sits below them). `fire_origin: Vec3` fixes the
`glam` dep; the loadout/authority types' fields fix whether `entities`/`foundation`
are needed. This mirrors `E19--render-data` (a leaf that sank shared types — `Aabb`,
`LightInfluence` — below the crates that had reached across for them).

## Open questions

- `restore_carried_health` placement (Task 4) — resolved at build by whether it names
  only combat-model/`entities` types. Recorded in the hub's open questions.
- Whether any carried-loadout or shot-authority type crosses the replication wire —
  confirmed at build; drives Invariant 1's scope. Recorded in the hub's open questions.
