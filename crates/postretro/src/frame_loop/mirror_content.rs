// The planar mirror's content inputs: its cells for mesh collection, and its billboards.
// See: context/lib/rendering_pipeline.md §7.3a

use postretro_entities::registry::EntityRegistry;
use postretro_level_loader::LevelWorld;
use postretro_sim::particle_pool::ParticlePool;
use postretro_visibility::VisibleCells;

use crate::render::Renderer;
use crate::render_preparation::PlanarRenderPreparation;
use crate::scripting_systems::particle_render::ParticleRenderCollector;

/// The mirror's visible cells on a frame it draws, for the mesh collector's
/// mirror reach; `None` with no mirror view.
pub(super) fn mirror_cells(planar: &PlanarRenderPreparation) -> Option<&VisibleCells> {
    planar.camera().map(|_| planar.visible())
}

/// Collect the mirror's billboards over its cells and cropped cull frustum,
/// after the main view's collection and apart from it, and hand them to the
/// renderer's mirror sprite store. A frame with no mirror view hands none.
pub(super) fn collect_mirror_particles(
    planar: &PlanarRenderPreparation,
    collector: &mut ParticleRenderCollector,
    registry: &EntityRegistry,
    particles: &ParticlePool,
    world: Option<&LevelWorld>,
    presentation_tick: f64,
    renderer: &mut Renderer,
) {
    match planar.camera() {
        Some(camera) => collector.collect_mirror_at_tick(
            registry,
            particles,
            world,
            planar.visible(),
            presentation_tick,
            &camera,
        ),
        None => collector.clear_mirror(),
    }
    renderer.set_planar_particle_draws(collector.iter_mirror_collections());
}
