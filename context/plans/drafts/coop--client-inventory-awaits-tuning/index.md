# coop--client-inventory-awaits-tuning

Brief · compact · reads: `context/lib/networking.md` §What gates, and what replicates instead, §Combat authority (active-slot re-sync), §Session-state ledger · read at b0a5c0f

> **Investigate at next review:** a possible separate defect, not yet traced. During the join window the client's Input sends `firing_slot` as 0, and the host's `prepare_remote_pawn_command` resolves remote fire against that slot of its own inventory. After a level change that carried the selection off slot 0, fire held in the window could resolve against a weapon the player isn't holding. Confirm whether the host actually fires it, and if so give it its own ticket (`research.md`; Decisions › Non-goals).

## Problem
Raised in review of the lossy-link loss-handling work (PR #601). Basis: a defect found by reading source; #601 patched its symptom. Cause: when a connected client's pawn baseline arms before the host's tuning payload installs, the client composes the pawn's weapon inventory from its own descriptor, a local fallback for a value the host replicates. #601 then reconciles the late payload onto that composition: it adopts the payload's spawn active slot while the selection is untouched, and an active-slot re-sync converges the host to the client's slot when adoption is skipped. Nothing the player sees gains from composing early. The client also materializes movement only from the payload. Every inventory consumer traced (viewmodel, weapon HUD, selection, switch declaration, weapon prediction, re-sync) reads only the local movement pawn, so a pre-payload inventory is never presented (`research.md`). When done, a connected client composes its pawn's inventory once, from the host's payload, whichever of baseline and payload lands first. On the late-payload path its active slot is the host's spawn active slot, and no adoption or re-sync is needed to reach it.

## Decisions
- **Compose from the payload only.** The armed local pawn holds no inventory until a payload installs. The first install composes it from the payload's slot layout and spawn active slot. This path already runs today when the payload lands first. Composing from the client's own descriptor is removed. It is the local-registry fallback that `networking.md` §What gates forbids for a replicated value.
- **Join-window behaviour: no weapon until the payload.** Owner chose option (a). Between baseline and payload the pawn holds nothing. That matches what the player sees today, because movement already waits for the payload. Every other Decision depends on this one.
- **Late-payload adoption is removed.** It existed only to correct a composition from the client's own descriptor. Later payloads keep today's merge: they preserve a same-canonical instance and its live state (`plans/done/E16--wieldable-switching-inventory` O41). The spawn active slot is fixed at spawn, and a later payload does not re-apply it to the selection.
- **The active-slot re-sync stays.** `networking.md` §Combat authority keeps it for any divergence that composition parity misses. This brief only takes away the join race that was its common trigger. Undo cost if the owner retires it later: one module and its call site.
- **Carried state stays host-side.** The payload carries slot identities and the spawn slot. It carries no magazines and no reserve. The client composes those from descriptor seeds, as its payload-first path does today. Prior research found no client reader of them, and the executor re-verifies that. A reader found on the client is a false premise and stops the build. Replicating carried resources is not this brief's work.
- **No wire or epoch change.** The payload already carries everything composition needs. The tuning epoch stays 14, and `WIRE_VERSION` and `SNAPSHOT_VERSION` are untouched.
- **Layer placement.** Client only: the netcode client endpoint and the sim's client-inventory materializer. The host's composition, carried restore and payload build are unchanged.
- **Non-goals.**
  - Deferring the arm itself until the payload. That is the rival in Path, and it touches reconcile and the camera hand-off (`coop--client-spawn-view-handoff`).
  - The client's pre-payload Input. The host already acts on it while the client predicts nothing, and that predates this brief. Its `firing_slot` defaults to 0 in the window, and the host resolves remote fire against that slot (`research.md`). That needs a separate fix, and this brief neither causes it nor changes it.

## Acceptance
### Automated
**Late payload (baseline arms first)**
- [ ] A client whose own descriptor's loadout differs from the host's never instantiates a weapon from its own loadout at any point in the join. Its first inventory is the host's slot layout.
- [ ] Between the baseline and the payload, the armed pawn has no inventory.
- [ ] After a level change that carried a selection off the first slot, a payload delayed past the baseline composes the client active on the carried slot. The host and the client name the same slot within one round trip, and no re-sync declaration is sent.
- [ ] Same as above, but the player pressed a slot key in the window. After the payload installs, the host and the client still agree within one round trip without a re-sync declaration. Any switch the player's input causes reaches the host as an ordinary switch declaration.
- [ ] A late payload whose spawn active slot names an empty slot composes active on the first populated slot.

**Unchanged paths (regression guards)**
- [ ] With the payload before the baseline, composition is as it was before this brief: the host's layout, active on the spawn slot.
- [ ] A second payload that changes only weapon tuning keeps each same-canonical instance, its magazine and its live timers, and does not move the active slot.
- [ ] A payload that fails its epoch check composes no inventory and logs the failure.
- [ ] A demotion or level unload between baseline and payload composes nothing from the previous level's payload.
- [ ] The active-slot re-sync still converges a host whose slot diverged after a switch.

### Manual
- [ ] Conditioned-link co-op join (`networking.md` §Testing the conditioned link) with Control loss heavy enough to land the payload after the baseline, after a level change that carried a non-first selection. The client's first viewmodel is the carried weapon, and the log shows no "re-declaring slot" line.

## Path
- Composition seam: `materialize_net_local_wieldable_inventory_from_tuning`, whose no-payload branch calls `compose_wieldable_inventory`. Its doc comment claims descriptor defaults make the pawn responsive before Control arrives; `research.md` shows they do not. Arm-time caller: `materialize_armed_local_pawn` via the snapshot arm path. Install-time caller: `NetEndpoint::install_tuning_payload` → `apply_installed_movement_tuning_to_armed_pawn`, which already composes from slots when the pawn has no inventory.
- Removal: `spawn_slot_adoption_open`, `adopt_payload_spawn_active_slot`, the `selection_idle` parameter, and the `Tuning` arm's caller in main.rs's `client_drain_control`, plus their tests. The install's return value can become "the slot composition chose", so the caller still resets the selection latch to it.
- `networking.md` §What gates and §Combat authority describe the composition and adoption this brief removes. Rewrite those sentences at promotion.
- Strongest rival: don't arm the local pawn until the payload installs. That gives one ordering but delays reconcile seeding and the camera hand-off, and those belong to other seams. The chosen shape changes only what the arm composes.
- First slice: the conditioned-link late-payload row, driven through `switch_order.rs`'s harness. It falsifies the claim that composing on install reaches the host's slot with no re-sync.

## Direction review
`/validate-plan`: *Direction sound*, conditional on the owner choosing (a) for the join window. The owner chose (a), so the condition is met.

## Open questions
- ~~Join-window behaviour~~ — **resolved: (a)**, the armed pawn holds no weapon until the payload (owner decision).
- Whether the active-slot re-sync should be retired once join composition is exact. Recommendation: keep it (Decisions). — owner: project owner — **delegated** unless the owner reopens it
