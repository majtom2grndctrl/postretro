// A connected client's predicted tick, from prediction to the camera endpoint, as
// one ordered seam the frame loop and its teleport proofs share.
// See: context/lib/networking.md §Snapshot apply ordering · context/lib/movement.md §6

use crate::frame_timing::InterpolableState;
use crate::netcode;

impl crate::App {
    /// One connected-client predicted tick after its command is built. `predict` runs
    /// weapon then movement prediction and returns the movement result. Then, in order:
    /// a predicted teleport turns the view, detaches mover carry and starts its cue;
    /// the camera follows the presented pose, the registry pawn plus the correction
    /// offset prediction left; the offset decays one step; the shots this tick queued
    /// freeze this tick's aim; and timing holds the arrival eye on a teleport tick, or
    /// pushes the eye on any other.
    pub(crate) fn run_client_predicted_tick(
        &mut self,
        has_player_pawn: bool,
        predict: impl FnOnce(&mut Self) -> Option<netcode::ClientPredictionTickResult>,
    ) -> Option<netcode::ClientPredictionTickResult> {
        let prediction_tick = predict(self);
        let predicted_teleport = prediction_tick
            .as_ref()
            .and_then(|tick| tick.movement_events.teleport);
        if let Some(event) = predicted_teleport.as_ref() {
            super::apply_predicted_teleport(
                &mut self.camera,
                &mut self.mover_yaw_carry_ground,
                &mut self.pending_teleport_sounds,
                &self.collision_world.teleporters,
                event,
            );
        }
        // Tick-rate camera follow tracks the PRESENTED local pose: the snapped
        // registry pose plus the decaying presentation offset. Folding the offset in
        // here, before the timing push, keeps both timing endpoints in presented
        // space, so the render-rate blend between them is the correction smoother
        // and the render stage must not re-add the offset.
        let presentation_offset = netcode::client_local_presentation_offset(
            self.session
                .as_ref()
                .and_then(|session| session.net_endpoint.as_ref()),
        );
        let identity = self.session.as_ref().and_then(|session| {
            let registry = session.scripting.script_ctx.registry.borrow();
            if has_player_pawn {
                crate::follow_camera_to_local_pawn(
                    &mut self.camera,
                    &registry,
                    presentation_offset,
                );
            }
            super::local_teleport_identity(&registry)
        });
        // Decay once now that this tick's camera pose has baked in the value.
        netcode::client_decay_local_correction(
            self.session
                .as_mut()
                .and_then(|session| session.net_endpoint.as_mut()),
        );
        self.client_weapon
            .close_tick_aim(crate::client_weapon::TickAim {
                pose: crate::PresentedAimPose {
                    position: self.camera.position,
                    yaw: self.camera.yaw,
                    pitch: self.camera.pitch,
                },
                teleport: identity,
            });
        if predicted_teleport.is_some() {
            super::hold_local_arrival(
                &self.camera,
                &mut self.frame_timing,
                &mut self.mover_yaw_carry_ground,
            );
        } else {
            self.frame_timing
                .push_state(InterpolableState::new(self.camera.position));
        }
        prediction_tick
    }
}
