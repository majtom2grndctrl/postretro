# coop--timed-slot-projection — research

Moved from `coop--lossy-link-delivery/research.md` via `drafts/ui-tween-clock` (read at b0a5c0f; symbols rechecked at d6f3471).

- `hud.ts` `reloadMeter` binds `player.reloadProgress` with no tween, and visibility follows `player.reloadActive`.
- On a connected client, `PlayerHudStatePublisher::tick_for_role_and_report_sampled_weapon` skips reload publish. The value is the host's `WeaponComponent::reload_feedback_sample`: an endpoint stream, else `1 - remaining/total`. It is sent as `ReplicatedWireShape::WieldableSlotNumber` at `SNAPSHOT_TICK_INTERVAL` = 2 (~30 Hz, unreliable).
- `ClientStateApply` writes it straight into the store. Jitter reorders snapshots, and newest-wins drops the late ones. A loss leaves a ~66 ms hold.
- `WEAPON_COOLDOWN_SLOT` (`player.weaponCooldownMs`) shares the wire shape, so the executor checks whether any HUD binds it.
- Host-time anchor precedent: `PresentationBindings::accept_presentation_clock` / `presentation_host_time` (client/bindings.rs): host anchor + RTT/2 + (local `script_time` − local anchor). Consumed by mesh_anim.rs `rebase_time` and by the damaged-enemy roster in presentation/overlays.rs, which clamps host now to a high-water mark.

