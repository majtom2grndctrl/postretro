// Local fluid presentation at the App: the camera eye's view tint and world
// muffle, and the per-frame fluid surface draws.
// See: context/lib/rendering_pipeline.md §7.3a, §7.8 (Fluid view tint) · context/lib/audio.md §1 (World muffle)

mod eye;

#[cfg(test)]
mod tests;

pub(crate) use eye::FluidEye;
use eye::FluidLook;

use glam::Vec3;
use postretro_collision::{EnvironmentView, EnvironmentVolumes};
use postretro_combat_model::FluidMovementTable;
use postretro_foundation::{FluidDescriptor, FluidRegistry};
use postretro_level_loader::LevelWorld;
use postretro_render_cpu::fluid_surface::{FluidSurfaceDraw, FluidSurfaceRenderCollector};
use postretro_visibility::VisibleCells;

use crate::App;

/// Surface opacity frame capture draws every fluid volume at. Capture is
/// VM-free and loads no manifest, so it cannot read a fluid's own opacity;
/// a fixed one keeps the pass visible and timed in captures.
#[cfg(feature = "capture")]
pub(crate) const CAPTURE_FLUID_SURFACE_OPACITY: f32 = 0.6;

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

/// This frame's fluid surface draws, far to near: surfaces whose bounds touch
/// a visible cell, at their fluid's local surface opacity. A dry volume, or
/// one whose fluid this machine does not declare, draws nothing.
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
            local_fluid_presentation(volumes, fluids, local, volume)
                .map(|fluid| fluid.surface_opacity)
        },
    )
}

impl App {
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

    /// Drop the eye's fluid presentation at level unload: no tint or muffle
    /// outlives its level, including on the world-less frames that follow.
    pub(crate) fn clear_fluid_eye_presentation(&mut self) {
        self.fluid_eye = FluidEye::default();
        if let Some(renderer) = self.renderer.as_mut()
            && renderer.is_full_ready()
        {
            renderer.set_fluid_view_tint([0.0; 4]);
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
