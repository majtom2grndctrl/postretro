// Per-frame resolve pass: fill pending entry stamps, recover runtime rebase
// origins after serde, and clear completed fades. Plus the fade-window
// predicates it drives.
// See: context/lib/scripting.md §10.3 (Mesh Animation)

use crate::components::mesh::MeshComponent;
use crate::registry::{EntityId, EntityRegistry};

use super::{FadeSourceKind, MeshAnimation};

impl MeshAnimation {
    /// The crossfade window (seconds) of the current state, treating a
    /// non-positive `crossfadeMs` (or an undeclared current state) as a hard cut
    /// (`0.0`). The window governs when an active fade reaches weight 1.0.
    fn current_crossfade_seconds(&self) -> f32 {
        self.states
            .get(&self.current_state)
            .map(|s| (s.crossfade_ms / 1000.0).max(0.0))
            .unwrap_or(0.0)
    }

    /// True if the active fade (recorded `previous_state`) has reached weight
    /// `>= 1.0` at `now` — i.e. the crossfade window measured from the current
    /// state's `entered_at` has fully elapsed. False while a stamp is still
    /// pending (`entered_at == None`): a fade that has not started cannot have
    /// completed. A hard-cut window (`crossfade <= 0`) completes immediately on
    /// the first resolved frame.
    fn fade_completed_at(&self, now: f64) -> bool {
        let Some(entered_at) = self.entered_at else {
            return false;
        };
        let crossfade = self.current_crossfade_seconds();
        if crossfade <= 0.0 {
            return true;
        }
        (now - entered_at) as f32 >= crossfade
    }

    /// Whether the per-frame resolve pass must act on this entity. Steady-state
    /// entities (entry stamp resolved, no recorded fade) are skipped — touching
    /// them would clone, no-op mutate, and rewrite the component every frame.
    /// Work is due for exactly four reasons:
    /// - a pending current entry stamp to fill (`entered_at == None`);
    /// - a missing current rebase origin to recover from its entry stamp;
    /// - an active fade whose previous stamp is still pending (carry/fill it);
    /// - an active fade whose previous rebase origin is missing or that has
    ///   reached weight 1.0 and must be cleared.
    fn resolve_pass_has_work(&self, now: f64) -> bool {
        self.entered_at.is_none()
            || self.rebase_time.is_none()
            || (self.previous_state.is_some()
                && (self.previous_entered_at.is_none()
                    || self.previous_rebase_time.is_none()
                    || self.fade_completed_at(now)))
    }

    /// Clear a completed fade back to steady state: no previous state, no
    /// previous stamp, fade source reset to its `Clip` default, and the
    /// interrupt-outgoing stash dropped. After this the collector samples the
    /// single new clip (the pose at weight 1.0).
    fn clear_completed_fade(&mut self) {
        self.previous_state = None;
        self.previous_entered_at = None;
        self.clear_previous_playback_time();
        self.fade_source = FadeSourceKind::Clip;
        self.interrupted_outgoing = None;
    }
}

/// Resolve every mesh entity's pending entry stamps from the frame's
/// post-advance animation-clock value, and clear fades that have completed.
/// Runs in the render-frame collection sub-stage, immediately before the mesh
/// collector, with a mutable registry.
///
/// Four jobs, on exactly the entities that need them (steady-state entities are
/// skipped — see [`MeshAnimation::resolve_pass_has_work`] — so the hot path does
/// not clone and rewrite untouched components every frame):
/// - A pending `entered_at` (`None`) is filled with `now`.
/// - A pending `previous_entered_at` accompanying an active fade is filled too
///   (a switch out of a freshly-entered state where the previous stamp could not
///   be carried).
/// - Runtime-only rebase origins skipped by direct component serde are restored
///   from their preserved entry stamps, retaining the live unscaled phase on the
///   first resolve instead of restarting it at `now`.
/// - A fade that has reached weight 1.0 (window measured from the current
///   state's `crossfadeMs`) is cleared back to steady state, so the next
///   `switch_animation_state` does not mistake a finished fade for an in-flight
///   one and record a spurious snapshot capture. At weight 1.0 the collector
///   already shows only the new clip, so clearing is pose-equivalent.
///
/// This seam fills the stamps so clip-local times and fade windows have a
/// concrete origin; the fade source-kind decision is made earlier (at switch
/// time, in [`switch_animation_state`]) and the per-frame capture inputs are
/// computed downstream by the render-frame collector.
///
/// [`switch_animation_state`]: super::switch_animation_state
pub fn resolve_pending_animation_stamps(registry: &mut EntityRegistry, now: f64) {
    use crate::registry::ComponentKind;

    // Collect ids first so we don't hold an immutable borrow across the mutable
    // writes. Mesh instance counts are small relative to a frame's work.
    let pending: Vec<EntityId> = registry
        .iter_with_kind(ComponentKind::Mesh)
        .filter_map(|(id, value)| match value {
            crate::registry::ComponentValue::Mesh(mesh) => mesh
                .animation
                .as_ref()
                .filter(|a| a.resolve_pass_has_work(now))
                .map(|_| id),
            _ => None,
        })
        .collect();

    for id in pending {
        let Ok(mut component) = registry.get_component::<MeshComponent>(id).cloned() else {
            continue;
        };
        let Some(anim) = component.animation.as_mut() else {
            continue;
        };
        if anim.entered_at.is_none() {
            anim.entered_at = Some(now);
        }
        if anim.rebase_time.is_none() {
            // Direct component serde deliberately drops the runtime rebase
            // triple but retains `entered_at`. Anchor the rebuilt timeline to
            // that preserved entry stamp so its first resolve keeps the phase
            // it was already showing; a genuinely pending entry was just
            // stamped above and therefore correctly begins at `now`.
            anim.rebase_time = anim.entered_at;
            anim.rebase_elapsed = 0.0;
        }
        if anim.previous_state.is_some() && anim.previous_entered_at.is_none() {
            anim.previous_entered_at = Some(now);
        }
        if anim.previous_state.is_some() && anim.previous_rebase_time.is_none() {
            // Mirror the current-side serde recovery for an outgoing fade leg.
            // `previous_entered_at` is either the carried live stamp or the
            // pending fallback filled just above.
            anim.previous_rebase_time = anim.previous_entered_at;
            anim.previous_rebase_elapsed = 0.0;
        }
        // Clear a fade that has reached weight 1.0. Re-checked after filling the
        // current stamp above, so a fade entered with a pending stamp is
        // evaluated against the stamp it was just assigned (a hard-cut window
        // clears on this first resolved frame).
        if anim.previous_state.is_some() && anim.fade_completed_at(now) {
            anim.clear_completed_fade();
        }
        let _ = registry.set_component(id, component);
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_pending_animation_stamps;
    use crate::components::animation::test_support::{spawn_animated, two_state_animation};
    use crate::components::animation::{switch_animation_state, FadeSourceKind};
    use crate::components::mesh::MeshComponent;
    use crate::registry::{EntityRegistry, Transform};

    #[test]
    fn serde_rebase_recovery_preserves_current_and_previous_entry_phase() {
        // Component serde intentionally omits runtime triples, but it retains
        // the authored/live entry stamps. The first resolve must rebuild each
        // missing origin from its own stamp, not from the resolve instant.
        let mut value = MeshComponent::animated("decraniated".into(), two_state_animation());
        let anim = value.animation.as_mut().unwrap();
        anim.states.get_mut("attack").unwrap().crossfade_ms = 20_000.0;
        anim.current_state = "attack".into();
        anim.entered_at = Some(4.0);
        anim.previous_state = Some("idle".into());
        anim.previous_entered_at = Some(2.0);
        anim.rebase_time = Some(4.0);
        anim.rebase_elapsed = 7.0;
        anim.previous_rebase_time = Some(2.0);
        anim.previous_rebase_elapsed = 9.0;

        let restored: MeshComponent =
            serde_json::from_value(serde_json::to_value(value).unwrap()).unwrap();
        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        registry.set_component(id, restored).unwrap();

        resolve_pending_animation_stamps(&mut registry, 10.0);
        let anim = registry
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert_eq!(anim.rebase_time, Some(4.0));
        assert_eq!(anim.previous_rebase_time, Some(2.0));
        assert!((anim.scaled_elapsed(10.0) - 6.0).abs() < 1.0e-9);
        assert!((anim.previous_scaled_elapsed(10.0) - 8.0).abs() < 1.0e-9);
    }

    #[test]
    fn resolve_pass_skips_steady_state_entity() {
        // A steady-state animated entity (entry stamp resolved, no recorded
        // fade) must NOT be picked up by the resolve pass: the predicate reports
        // no work, and a second resolve at a later clock leaves the component
        // byte-identical (no needless clone/no-op-mutate/rewrite each frame).
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg);
        resolve_pending_animation_stamps(&mut reg, 1.0);

        let before = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(before.entered_at, Some(1.0));
        assert_eq!(before.previous_state, None);
        // The pass predicate reports no work for a steady-state entity.
        assert!(
            !before.resolve_pass_has_work(2.0),
            "steady-state entity must report no resolve-pass work"
        );

        // Running the pass again at a later clock must not alter the component.
        resolve_pending_animation_stamps(&mut reg, 2.0);
        let after = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(before, after, "steady-state component must be untouched");
    }

    #[test]
    fn resolve_pass_clears_fade_after_crossfade_window() {
        // idle→attack records a fade (attack crossfade = DEFAULT_CROSSFADE_MS =
        // 150ms = 0.15s). Resolve at the switch instant retains the fade; once
        // the clock passes the window, the next resolve clears it back to steady
        // state and resets the fade source to Clip.
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg);
        resolve_pending_animation_stamps(&mut reg, 1.0);
        switch_animation_state(&mut reg, id, "attack"); // records fade (idle→attack)

        // First resolve fills the new entry stamp at 1.0; fade still in flight.
        resolve_pending_animation_stamps(&mut reg, 1.0);
        let during = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(during.entered_at, Some(1.0));
        assert_eq!(
            during.previous_state.as_deref(),
            Some("idle"),
            "fade retained during the window (weight < 1.0)"
        );

        // Advance past the 0.15s window and resolve again → fade cleared.
        resolve_pending_animation_stamps(&mut reg, 1.0 + 0.2);
        let after = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(
            after.previous_state, None,
            "completed fade is cleared once the window elapses"
        );
        assert_eq!(after.previous_entered_at, None);
        assert_eq!(after.fade_source, FadeSourceKind::Clip);

        // A subsequent switch must see no in-flight fade: it records `Clip`, not
        // a spurious `Snapshot` capture.
        switch_animation_state(&mut reg, id, "idle");
        let next = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(
            next.fade_source,
            FadeSourceKind::Clip,
            "no spurious snapshot capture after a completed fade",
        );
    }

    #[test]
    fn resolve_pass_retains_fade_within_crossfade_window() {
        // During the crossfade window (weight < 1.0) the resolve pass must NOT
        // clear the fade: previous_state stays Some so the collector keeps
        // blending the outgoing pose.
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg);
        resolve_pending_animation_stamps(&mut reg, 1.0);
        switch_animation_state(&mut reg, id, "attack");
        resolve_pending_animation_stamps(&mut reg, 1.0);

        // Halfway through the 0.15s window: fade must still be present.
        resolve_pending_animation_stamps(&mut reg, 1.0 + 0.075);
        let anim = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert_eq!(
            anim.previous_state.as_deref(),
            Some("idle"),
            "fade retained mid-window (weight < 1.0)"
        );
        assert_eq!(anim.previous_entered_at, Some(1.0));
    }

    #[test]
    fn resolve_pass_fills_pending_spawn_stamp() {
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg);
        // Spawn leaves entered_at pending.
        assert!(
            reg.get_component::<MeshComponent>(id)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .entered_at
                .is_none()
        );

        resolve_pending_animation_stamps(&mut reg, 4.25);
        assert_eq!(
            reg.get_component::<MeshComponent>(id)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .entered_at,
            Some(4.25)
        );
    }

    #[test]
    fn resolve_pass_fills_previous_stamp_for_active_fade() {
        // A switch out of a state whose own stamp was pending leaves
        // previous_entered_at None; but a normal switch carries the previous
        // stamp. Here we cover the carried case plus the new pending current.
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg);
        resolve_pending_animation_stamps(&mut reg, 1.0);
        switch_animation_state(&mut reg, id, "attack");

        resolve_pending_animation_stamps(&mut reg, 3.0);
        let anim = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert_eq!(anim.entered_at, Some(3.0), "new current stamp filled");
        assert_eq!(
            anim.previous_entered_at,
            Some(1.0),
            "previous stamp carried from before the switch, not overwritten"
        );
    }
}
