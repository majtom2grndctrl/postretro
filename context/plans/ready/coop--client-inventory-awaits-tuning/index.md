# coop--client-inventory-awaits-tuning

Brief · compact · reads: `context/lib/networking.md` §What gates, and what replicates instead, §Combat authority (active-slot re-sync), §Session-state ledger · read at fd8982c

## Problem
Raised in review of the lossy-link loss-handling work (PR #601). Basis: a contract violation found by reading source, not a defect a player has seen. Cause: when a connected client's pawn baseline arms before the host's tuning payload installs, the client composes the pawn's weapon inventory from its own descriptor. That is the local-registry fallback `networking.md` §What gates forbids for a replicated value. #601 corrects it when the payload lands: adoption moves the client to the payload's spawn active slot. In the join window nothing can touch the selection, so adoption always applies and today's join already ends on the host's slot (`research.md`). The cost is what the fallback leaves and what the correction carries:
- the client instantiates weapons from its own loadout;
- ammo reserve seeded from that loadout outlives the merge;
- adoption stays wired into the client's tuning install for the pawn's whole life.

No client reader shows that reserve today, and no inventory consumer presents the pawn before the payload, because movement also waits for it. When done, a connected client composes its pawn's inventory once, from the host's payload, whichever of baseline and payload lands first. On the late-payload path the client starts on the host's spawn active slot, with no adoption.

## Decisions
- **Compose from the payload only.** The armed local pawn holds no inventory, and so no weapon, until a payload installs. The first install composes the inventory from the payload's slot layout and spawn active slot, as the payload-first path does today. Composing from the client's own descriptor is removed.
- **Late-payload adoption is removed.** Once the install composes, adoption has nothing left to correct. Later payloads keep today's merge, which preserves a same-canonical instance and its live state (`plans/done/E16--wieldable-switching-inventory` O41). The spawn active slot is fixed at spawn, and a later payload never re-applies it to the selection.
- **The active-slot re-sync stays.** `networking.md` §Combat authority keeps it for divergence that composition parity misses, such as a spawn slot naming a weapon the client cannot compose (`research.md` P15).
- **Carried state stays host-side.** The payload carries slot identities and the spawn slot, but no magazines or reserve. The client seeds those from weapon descriptors, as its payload-first path does today. No client reader of them exists (`research.md`). Replicating carried resources is not this brief's work.
- **No wire or epoch change.** The payload already carries everything composition needs. The tuning epoch, `WIRE_VERSION` and `SNAPSHOT_VERSION` are untouched.
- **Layer placement.** Client only: the sim's client-inventory materializer, the netcode client endpoint's tuning install, and the binary's Control drain that calls it. The host's composition, carried restore and payload build are unchanged.
- **Non-goals.**
  - Deferring the arm itself until the payload. That is the rival in Path, and it touches reconcile and the camera hand-off (`coop--client-spawn-view-handoff`).
  - The client's pre-arm Input. Its `firing_slot` defaults to 0 in the window, and a fresh fire press there fires the host pawn's slot-0 weapon whatever its active slot (`research.md`). That predates this brief, which neither causes nor changes it. It is on the roadmap as `coop--pre-arm-fire-resolves-slot-zero`.

## Acceptance
### Automated
Rows marked *(fails today)* fail on the build before this brief. The rest guard behaviour that today's adoption already delivers. Pins are in `research.md`.

**The fallback is gone**
- [ ] *(fails today)* A client whose own descriptor's loadout differs from the host's never instantiates a weapon from its own loadout at any point in the join. Its first inventory is the host's slot layout.
- [ ] *(fails today)* Between the baseline and the payload, the armed pawn has no inventory.
- [ ] *(fails today)* A late payload leaves the pawn's ammo reserve and magazines equal to what the payload-first path composes from the same payload. No ammo type from the client's own loadout remains. Pin P2.
- [ ] *(fails today)* A demotion or level unload between baseline and payload composes nothing, from the previous level's payload or from the client's own descriptor, until the next payload installs. Pins P12, P13.

**The join still lands on the host's slot**
- [ ] After a level change that carried a selection off the first slot, a payload delayed past the baseline composes the client active on the carried slot. The payload goes through the client endpoint's install, and the baseline through the snapshot arm. The client's own descriptor's loadout differs from the host's. The host and the client name the same slot within one round trip, and no switch declaration of any kind is sent.
- [ ] Same as above, but the player pressed a slot key in the window. The press switches nothing and declares nothing. After the payload installs, the host and the client still agree within one round trip without a re-sync declaration. The first slot key pressed after the install reaches the host as an ordinary switch declaration. Pin P10.
- [ ] A late payload whose spawn active slot names an empty slot composes active on the first populated slot.
- [ ] A late payload with no weapons composes an inventory with every slot empty. A later payload that adds a weapon fills that slot and makes it active, without recomposing. Pin P9.

**Unchanged paths**
- [ ] With the payload before the baseline, or in the same frame, composition is as it was before this brief: once, from the host's layout, active on the spawn slot. Pin P1.
- [ ] *(fails today)* A second payload that changes weapon tuning and names a different spawn active slot, with the selection untouched, keeps each same-canonical instance, its magazine and its live timers, and does not move the active slot. Pin P4.
- [ ] After the player switches off the composed slot, later snapshots of the pawn, a full baseline refresh included, do not recompose the inventory or move the active slot. Pin P3.
- [ ] Starting from an inventory a first payload composed, a later payload that adds, removes, renames or names an unknown weapon grows, releases or replaces that slot as before, and leaks no entity.
- [ ] A payload that fails its epoch check before composition composes no inventory and logs the failure. The next good payload composes from itself. A failure after composition leaves the inventory as it was. Pins P6–P8.
- [ ] The active-slot re-sync still converges a host whose slot diverged after a switch.

**Gates**
- [ ] Grep gate: no late-payload adoption remains. The client's tuning install takes no selection-idle input, and nothing sets the active slot from a payload after composition.
- [ ] Diff gate: the host's composition, carried restore and payload build are unchanged. Only the client's call to own-descriptor composition goes.
- [ ] The wire version matches main (`cargo test -p xtask -- --ignored wire_version_matches_main`). The tuning epoch stays 14 and the snapshot version stays 21.

### Manual
- [ ] Conditioned-link co-op join (`networking.md` §Testing the conditioned link) with Control loss heavy enough to land the payload after the baseline, after a level change that carried a non-first selection. The client's first viewmodel is the carried weapon, and the log shows no "re-declaring slot" line.

## Path
- Composition seam: `materialize_net_local_wieldable_inventory_from_tuning`, whose no-payload branch calls `compose_wieldable_inventory`. Its doc comment claims descriptor defaults make the pawn responsive before Control arrives; `research.md` shows they do not. Arm-time caller: `materialize_armed_local_pawn` via the snapshot arm path. Install-time caller: `NetEndpoint::install_tuning_payload` → `apply_installed_movement_tuning_to_armed_pawn`, which already composes from slots when the pawn has no inventory.
- Removal: `spawn_slot_adoption_open`, `adopt_payload_spawn_active_slot`, the `selection_idle` parameter, and the `Tuning` arm's adoption handling in main.rs's `client_drain_control`, plus their tests. The selection-latch reset after adoption can go too: with no movement pawn the latch clears every frame, so it is already clear when the late payload composes (`research.md`).
- The merge tests in `net_descriptor.rs` seed their starting inventory through the no-payload branch. Re-seed them from a payload-composed inventory and keep their assertions.
- `networking.md` §What gates already states payload-only composition, with a sentence marking it not yet built. Drop that sentence at landing.
- Strongest rival: don't arm the local pawn until the payload installs. That gives one ordering but delays reconcile seeding and the camera hand-off, and those belong to other seams. The chosen shape changes only what the arm composes.
- Runs beside `movement--static-overlap-invariant`, in build elsewhere. The seams are disjoint: that brief owns the movement substrate and the agent controller. Both may touch `netcode/src/lib.rs`, but in different functions, so neither waits on the other. Whichever lands second merges the base branch.
- First slice: the late-payload join row. No harness runs it as written. `switch_order.rs`'s fixture never arms the local pawn. `receive_alternate.rs` arms through replication, but it stubs the Control install, expects an inventory on every arm, and shares one descriptor table between host and client (`research.md`). Extend `receive_alternate.rs` first.

## Open questions
None.
