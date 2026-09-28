# coop--client-spawn-view-handoff — research

Read at 05ab2d3da (`feat/preferences-comfort-floor`). Symbols, not lines. **V** means read in source this session. **I** means inferred from reading and not executed.

## Client join sequence today

| Step | Client renders | Host slot | Source |
|---|---|---|---|
| Connect, admission sent | splash or frontend | pending → admitted, seat minted | V: `NetClient::queue_control_messages`; host `admit_or_reclaim` in both host polls |
| Host names the map | same | admitted; Relevel sent on admission or at host install | V: `NetServer::set_relevel_catalog_id`, `send_relevel`, `relevel_recipients` |
| Client loads | splash (Loading) | admitted, held | V: `follow_relevel_catalog` enqueues a catalog load |
| Client install | splash | same | V: `install_level_payload` calls `set_level_parity` (the client's declaration) and teleports the camera to `first_spawn`; `install_world_cpu` suppresses the boot pawn for a connected client |
| Client reveal (today: same frame as install) | world from spawn 0, with spawn 0's yaw, at spawn origin | admitted | V: `finish_level_payload` → `clear_splash` → Running |
| Declaration arrives, parity matches | same | participating: placement assigned, pawn spawned and ticked, tuning sent | V: the host net frame's `Participating` arm → `assign_placement` → `host_handle_accept_descriptor_at_placement` |
| Epoch marker, tuning, first snapshot | same | same | V: `client_drain_control` (Tuning); `NetClient::is_participating` |
| `local_player` baseline arms the pawn | camera position jumps to the pawn eye; yaw keeps its prior value | same | V: `maybe_arm_local_pawn`; `follow_camera_to_local_pawn` writes position only; on level entry only `install_level_payload` and `--start-pose` write `camera.yaw` |

Before the pawn arms, no local pawn carries `PlayerMovement`. Remote pawns do not get one on the client. As a result, `local_player_movement_pawn` returns none and the camera does not move (V). Input is not sent without a participation epoch (V: `NetClient::send_input`).

## Join sequence after this brief

| Step | Client | Host slot |
|---|---|---|
| Host install | — | placement pre-assigned per admitted seat; Relevel, then hint |
| Client install | splash; parity declared; stand-in at hinted pose | parity evaluated; divergence cause sent now if content differs; else held for readiness |
| Client Settling | splash; settles at hinted pose | held (client not revealed) |
| Client reveal | world at hinted pose; ready declared | — |
| Ready drained | same | participating: pawn at pre-assigned placement, epoch, tuning |
| First baseline arms | no position or yaw jump | same |

## Placement facts

- **V.** Every `.map` under `content/` has at most one `player_spawn`. `campaign-test` has one.
- **V.** On the host, `spawn_from_player_starts_with_carried_loadout` spawns a pawn at every `player_spawn` and marks the first one local. `capture_player_spawn_placements`, run from the `install_world_cpu` hook, binds every such pawn to its placement. `occupied_live_placements` then reports all of them as occupied.
- **V.** When every index is occupied, `assign_placement` falls back to `next_placement_cursor % count` and logs "every player_spawn placement is occupied". On a single-spawn map the client therefore reuses index 0, the host's spawn, which is also the client's stand-in view. On current content, the jump never happens.
- **I (defect, own brief).** On an N-spawn host map the host's own spawn pass occupies all N placements, so the first joiner falls back to the cursor. `coop--spawn-placement-occupancy` fixes it and counts seated pawns and held seats. It assigns at promotion, so it does not count a placement pre-assigned to a seat that has no pawn yet; this brief extends the rule to cover that.
- **V.** The placement is sticky per seat within a level (`assign_placement`'s early return), cleared at level unload (E15), and scoped by `placement_count`.
- **V.** `level_content_digest` hashes movers, waypoints and static collision inputs. It does not hash `player_spawn` placements.
- **V.** On a host `restartLevel`, the client does not reload: `relevel_is_already_selected` returns early for the active catalog id. It is demoted (the `Holding` arm of `client_drain_control` calls `demote_client_state`) and then re-promoted in place.

## Participation today

- **V.** `NetServer::reevaluate_parity` is the single predicate site. It runs on `set_mod_digest`, `set_level_parity`, and once per client after each Control batch. `parity_cause` returns the first `HoldingCause` in variant order; variant order is documented as diagnostic precedence.
- **V.** `SlotTable::transition` requires a holding cause for participating → admitted. Any readiness-only demotion therefore needs a cause, which is why readiness adds `HoldingCause` variants instead of holding silently.
- **V.** `send_divergence` deduplicates per slot by cause; `NetClient::drain_control` retires the active epoch on any `Divergence(Holding)` frame carrying an epoch.
- **V.** Control is drained per poll to the last retained declaration. An unload (`None`) and a same-level re-install (`Some`) that land in one host batch leave parity unchanged, so parity alone cannot see a client that re-installed and is settling again. The readiness declaration closes that: its retraction at unload is also retained-last, and the batch ends not-ready until the reveal arrives.
- **V.** `NetEndpoint::poll_world_less` never applies snapshots. `poll_world_less_transport` handles `SlotEvent::Participating` by applying the join seed only; `host_handle_lifecycle` ignores the edge. A promotion consumed in a world-less poll therefore leaves a participating slot with no pawn. This is the reason for the host half of readiness.
- **V.** The client sends its join seed only alongside a parity send (`set_level_parity` resets `join_seed_sent`). Returning parity to install restores the seed's install timing, which the SH brief's interim delays to reveal.

## Parity placement

The SH brief moves the level-parity call to the Settling→Running edge on both peers (`NetEndpoint::set_level_parity` dispatches to host and client). That is its only defense against a pawn spawning while a player is on the splash. With readiness in the predicate, the move is no longer needed on either side:

- **Client.** Readiness, not parity timing, now stops promotion during client Settling. Parity at install gives the client its divergence cause during its settle rather than after reveal.
- **Host.** The host-revealed term stops promotion during host Settling. Without it, a host restart would promote a Running, still-ready client during the host's settle, in the world-less-style poll, with no pawn.
- **Kept at install:** Relevel and the join seed, as the SH brief already does, so no split of the install block remains.

Rejected: keep the host's parity at reveal and add only client readiness. It keeps "host installed parity" meaning "installed and revealed", which is the double meaning the owner removed, and it mislabels a settling host as `HostLevelAbsent`.

## Readiness shapes

| Shape | Slot lifecycle | Cost | Verdict |
|---|---|---|---|
| Readiness term in the participation predicate; hold = admitted with a readiness cause | Unchanged stages; predicate gains a term | Two holding causes, one client message, one host setter | **Chosen** |
| New stage between admitted and participating | Adds a transition pair beside the predicate | Duplicates what an admitted-with-cause slot already expresses | Rejected: `networking.md` warns against lifecycle as transition pairs |
| Participating with the pawn withheld | Amends "any entry to participating spawns its pawn" | Epoch, tuning and snapshots to a client that cannot apply them in Settling | Rejected |
| Readiness folded into parity (earlier draft) | Parity means installed and revealed | No new message | Rejected by owner |

Readiness is keyed to level identity, not reset by parity arrivals. Resetting on any parity arrival would strand a client after a mod-digest change: it re-declares parity but never re-reveals, so it would never re-send ready.

## Candidate spawn-view shapes

| Shape | Wire change | Verdict |
|---|---|---|
| Host sends the spawn pose before promotion; the client settles there | One appended server Control message | **Chosen** |
| Reveal at the stand-in, then re-settle after arming | None | Rejected. Two reveals, or the pop stays |
| Keep the splash until arm | None | Rejected. Needs snapshot apply during Settling |
| Client derives the placement from its seat | None | Rejected. Assignment depends on occupancy (E15) |

## Timing

- **I.** The pawnless window after client reveal is the ready declaration's one-way trip, the host's next poll, and the epoch marker, tuning and first baseline's one-way trip: about one round trip plus up to one snapshot interval. Not measured.
- **I.** Between client reveal and the first snapshot, movers show their install phase and remote pawns are absent; the first snapshot corrects both. Today's join has the same window.
- **I.** On a host restart, a Running client's pawnless window is the host's whole settle, up to the Settling timeout. Accepted: the client sees the level, cannot act, and nothing acts on it.

## Premises in the SH brief

- **V.** Today, the camera the SH brief settles from is the spawn origin with no eye height. The first Running frame lifts it by `capsule.eye_height` (`follow_camera_to_local_pawn`). The SH brief's eye-pose chokepoint is where this brief's hint enters.
