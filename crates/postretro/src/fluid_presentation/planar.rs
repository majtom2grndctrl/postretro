// Planar water candidates at the App: reflecting, probe-bound volumes, at install and refresh.
// See: context/lib/rendering_pipeline.md §7.3a

use postretro_level_loader::LevelWorld;
use postretro_render_cpu::fluid_surface::reflection::volume_has_bound_face;

use super::{local_fluid_presentation, surface_look};
use crate::App;

impl App {
    /// Install `world`'s planar candidates from the active fluid table, at
    /// level install, before the first frame and before the renderer's fluid
    /// install. Resets selection history, and hands the renderer the
    /// candidate verdict its planar target lifetime follows.
    pub(crate) fn install_planar_candidates(&mut self, world: &LevelWorld) {
        let reflecting = self.planar_reflecting_volumes(world);
        self.planar_render.install(world, &reflecting);
        self.sync_planar_candidate();
    }

    /// Re-install the installed level's planar candidates after a fluid
    /// refresh: a fluid's reflectivity or the table itself may have changed.
    /// Resets selection history, as an install does; a refresh that removes
    /// the last candidate releases the renderer's planar target.
    pub(crate) fn reinstall_planar_candidates(&mut self) {
        if let Some(world) = self.level.as_ref() {
            let reflecting = self.planar_reflecting_volumes(world);
            self.planar_render.install(world, &reflecting);
            self.sync_planar_candidate();
        }
    }

    /// The renderer's planar target exists only while the installed level
    /// holds the candidate plane selection reads: one source, the app's
    /// candidates (`rendering_pipeline.md` §7.3a, Target lifecycle).
    fn sync_planar_candidate(&mut self) {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_planar_candidate(self.planar_render.has_candidate());
        }
    }

    /// The volumes of `world` a plane may be selected from: their fluid's
    /// local look reflects (reflectivity above zero) and the volume has a
    /// probe-bound face, the reflecting gate planar keeps.
    pub(crate) fn planar_reflecting_volumes(&self, world: &LevelWorld) -> Vec<u32> {
        let Some(session) = self.session.as_ref() else {
            return Vec::new();
        };
        let local = session.scripting.script_ctx.data_registry.borrow();
        (0..world.environment_volumes.volumes.len())
            .filter_map(|volume| {
                let fluid = local_fluid_presentation(
                    &self.environment_volumes,
                    &self.environment_fluids,
                    &local.fluids,
                    volume,
                )?;
                let row = surface_look(
                    fluid,
                    volume_has_bound_face(world.reflection_probes.as_ref(), volume),
                )
                .pack();
                (row.reflectivity() > 0.0).then_some(volume as u32)
            })
            .collect()
    }
}
