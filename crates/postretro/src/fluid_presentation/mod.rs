// Local fluid presentation at the App: the camera eye's view tint, fog weight
// and world muffle, the per-frame fluid surface draws with their full look
// (opacity, depth fog, wave, reflection), and the surfaces' current drift.
// See: context/lib/rendering_pipeline.md §7.3a, §7.8 (Fluid view tint) · context/lib/audio.md §1 (World muffle)

mod drift;
mod eye;
mod planar;

#[cfg(test)]
mod tests;

pub(crate) use eye::FluidEye;
use eye::FluidLook;

use glam::Vec3;
use postretro_collision::{EnvironmentView, EnvironmentVolumes};
use postretro_combat_model::FluidMovementTable;
use postretro_foundation::{FluidDescriptor, FluidRegistry};
use postretro_level_loader::LevelWorld;
use postretro_render_cpu::fluid_surface::reflection::volume_has_bound_face;
use postretro_render_cpu::fluid_surface::{
    FluidSurfaceDraw, FluidSurfaceLook, FluidSurfaceRenderCollector,
};
use postretro_visibility::VisibleCells;

use crate::App;

/// Surface opacity frame capture draws every fluid volume at. Capture is
/// VM-free and loads no manifest, so it cannot read a fluid's own opacity;
/// a fixed one keeps the pass visible and timed in captures.
#[cfg(any(feature = "capture", test))]
pub(crate) const CAPTURE_FLUID_SURFACE_OPACITY: f32 = 0.6;

/// Frame capture's look for a named fluid's volume. Capture loads no
/// manifest, so it draws at the fixed capture opacity with no fog or wave,
/// reflecting as water (reflectivity 1) wherever a face binds a probe; it
/// never sets a reflection style, so the renderer's default, pixel, holds.
#[cfg(any(feature = "capture", test))]
pub(crate) fn capture_surface_look(has_bound_face: bool) -> FluidSurfaceLook {
    FluidSurfaceLook {
        has_bound_face,
        ..FluidSurfaceLook::surface_only(CAPTURE_FLUID_SURFACE_OPACITY)
    }
}

/// The local presentation of installed volume `volume`'s fluid: the local
/// manifest's declaration of the name the active table gives that fluid, or
/// `None` when this machine declares none ("shows none"). Takes the App's
/// fields apart so per-frame callers can borrow them beside a live collector.
pub(crate) fn local_fluid_presentation<'r>(
    volumes: &EnvironmentVolumes,
    fluids: &FluidMovementTable,
    local: &'r FluidRegistry,
    volume: usize,
) -> Option<&'r FluidDescriptor> {
    let fluid = volumes.volume(volume)?.fluid?;
    fluids.local_presentation(fluid, local)
}

/// A fluid's surface look as the renderer packs it: opacity, depth fog, wave
/// and reflectivity from the local declaration, reflecting wherever the volume
/// has a probe-bound face (`has_bound_face`).
pub(crate) fn surface_look(fluid: &FluidDescriptor, has_bound_face: bool) -> FluidSurfaceLook {
    FluidSurfaceLook {
        surface_opacity: fluid.surface_opacity,
        fog_distance: fluid.fog_distance,
        // Resolved, never the raw field: an omitted `fogColor` follows the
        // fluid's `viewTint`, so a reload that changes only the tint changes
        // the packed murk too.
        fog_color: fluid.resolved_fog_color(),
        wave_period: fluid.wave_period,
        wave_amplitude: fluid.wave_amplitude,
        wave_length: fluid.wave_length,
        surface_reflectivity: fluid.surface_reflectivity,
        has_bound_face,
    }
}

/// This frame's fluid surface draws, far to near: surfaces whose bounds touch
/// a visible cell, in their fluid's local look. A dry volume, or one whose
/// fluid this machine does not declare, draws nothing. Whether a volume has a
/// probe-bound face is one binary search over the level's bindings.
pub(crate) fn collect_fluid_surface_draws<'c>(
    collector: &'c mut FluidSurfaceRenderCollector,
    world: &LevelWorld,
    visible: &VisibleCells,
    camera_eye: Vec3,
    volumes: &EnvironmentVolumes,
    fluids: &FluidMovementTable,
    local: &FluidRegistry,
) -> &'c [FluidSurfaceDraw] {
    collector.collect(
        &world.environment_volumes.volumes,
        visible,
        |cell| world.cell_bounds(cell as usize),
        camera_eye,
        |volume| {
            local_fluid_presentation(volumes, fluids, local, volume).map(|fluid| {
                surface_look(
                    fluid,
                    volume_has_bound_face(world.reflection_probes.as_ref(), volume),
                )
            })
        },
    )
}

impl App {
    /// Re-resolve every installed fluid surface face's drift and hand it to the
    /// renderer. Drift is the steady state a swimmer settles at under the
    /// nominal push over the face, from the values prediction uses: the active
    /// fluid table's viscosities and the local player's terminal velocity.
    /// Called at level install, after the boot pawn spawns, and on every
    /// fluid-table or tuning refresh (`refresh_environment_fluids`). Only the
    /// renderer's drift buffer changes, so no draw or per-volume parameter row
    /// is touched.
    pub(crate) fn resolve_fluid_surface_drift(&mut self) {
        self.fluid_surface_drift = match self.level.as_ref() {
            Some(world) => drift::resolve_fluid_surface_drift(
                &world.environment_volumes.volumes,
                &self.environment_volumes,
                &self.environment_fluids,
                self.drift_terminal_velocity(),
                crate::frame_timing::TICK_DURATION.as_secs_f32(),
            ),
            None => Vec::new(),
        };
        if let Some(renderer) = self.renderer.as_mut()
            && renderer.is_full_ready()
        {
            renderer.set_fluid_surface_drift(&self.fluid_surface_drift);
        }
    }

    /// The terminal velocity that caps the local player's drift layer. A
    /// connected client reads its installed tuning payload, which arrives with
    /// the host's fluid table; host and offline read the local pawn's live
    /// `PlayerMovementComponent`, the value movement caps against, and fall back
    /// to the player descriptor (the pawn's class, else `player`) while no pawn
    /// carries one. With no movement tuning at all no swimmer exists to agree
    /// with, and drift takes no cap.
    ///
    /// Drift re-resolves only on install and on a fluid-table or tuning
    /// refresh, so a mid-level local pawn respawn or class change keeps the
    /// previous cap until the next refresh (no such path exists today).
    fn drift_terminal_velocity(&self) -> f32 {
        let Some(session) = self.session.as_ref() else {
            return f32::INFINITY;
        };
        if let Some(endpoint @ crate::netcode::NetEndpoint::Client { .. }) =
            session.net_endpoint.as_ref()
        {
            return endpoint
                .client_movement_tuning()
                .map_or(f32::INFINITY, |movement| movement.fall.terminal_velocity);
        }
        let registry = session.scripting.script_ctx.registry.borrow();
        let pawn = registry.local_player_pawn();
        if let Some(movement) = pawn.and_then(|pawn| {
            registry
                .get_component::<postretro_foundation::PlayerMovementComponent>(pawn)
                .ok()
        }) {
            return movement.fall.terminal_velocity;
        }
        let class = pawn
            .and_then(|pawn| {
                registry
                    .get_component::<postretro_entities::provenance::DescriptorProvenance>(pawn)
                    .ok()
                    .map(|provenance| provenance.canonical_name.clone())
            })
            .unwrap_or_else(|| "player".to_string());
        session
            .scripting
            .script_ctx
            .data_registry
            .borrow()
            .entities
            .iter()
            .find(|descriptor| descriptor.canonical_name.as_deref() == Some(class.as_str()))
            .and_then(|descriptor| descriptor.movement.as_ref())
            .map_or(f32::INFINITY, |movement| movement.fall.terminal_velocity)
    }

    /// Advance the camera eye's fluid presentation by one presented frame.
    /// Runs once per rendered frame on the effective render eye, after the
    /// frame eye is assembled and before the audio step.
    pub(crate) fn update_fluid_eye(&mut self, eye: Vec3, frame_dt: f32) {
        // The eye queries read volumes only; gravity and viscosity are unused.
        let view = EnvironmentView::new(0.0, &self.environment_volumes, &[]);
        let registry = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.data_registry.borrow());
        let volumes = &self.environment_volumes;
        let fluids = &self.environment_fluids;
        self.fluid_eye.update(view, eye, frame_dt, |volume| {
            let registry = registry.as_ref()?;
            local_fluid_presentation(volumes, fluids, &registry.fluids, volume).map(FluidLook::of)
        });
    }

    /// Drop the eye's fluid presentation at level unload: no tint, muffle or
    /// fog weight outlives its level, including on the world-less frames that
    /// follow.
    pub(crate) fn clear_fluid_eye_presentation(&mut self) {
        self.fluid_eye = FluidEye::default();
        if let Some(renderer) = self.renderer.as_mut()
            && renderer.is_full_ready()
        {
            renderer.set_fluid_view_tint([0.0; 4]);
            // The dry weight (1): the next level's first frame fogs from
            // outside, as a freshly built renderer would.
            renderer.set_fluid_fog_weight(self.fluid_eye.fog_weight());
        }
        if let Some(audio) = self
            .session
            .as_mut()
            .and_then(|session| session.audio.as_mut())
        {
            audio.set_world_muffle(0.0);
        }
    }
}
