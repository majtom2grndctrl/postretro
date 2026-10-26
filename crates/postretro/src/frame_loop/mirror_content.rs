// The planar mirror's content inputs: its cells for mesh collection, and its billboards.
// See: context/lib/rendering_pipeline.md §7.3a

use postretro_entities::registry::EntityRegistry;
use postretro_level_loader::LevelWorld;
use postretro_sim::particle_pool::ParticlePool;
use postretro_visibility::{Frustum, VisibleCells};

use crate::render::Renderer;
use crate::render_preparation::PlanarRenderPreparation;
use crate::scripting_systems::particle_render::ParticleRenderCollector;

/// The mirror's visible cells on a frame it draws, for the mesh collector's
/// mirror reach; `None` with no mirror view.
pub(super) fn mirror_cells(planar: &PlanarRenderPreparation) -> Option<&VisibleCells> {
    planar.camera().map(|_| planar.visible())
}

/// Collect the main view's billboards over `visible` and `frustum` and, on a
/// frame the mirror draws, the mirror's over its cells and cropped cull
/// frustum, in one pass over the particles; then hand the mirror's to the
/// renderer's mirror sprite store. A frame with no mirror view hands none.
#[allow(clippy::too_many_arguments)]
pub(super) fn collect_particles(
    planar: &PlanarRenderPreparation,
    collector: &mut ParticleRenderCollector,
    registry: &EntityRegistry,
    particles: &ParticlePool,
    world: Option<&LevelWorld>,
    visible: &VisibleCells,
    presentation_tick: f64,
    frustum: &Frustum,
    renderer: &mut Renderer,
) {
    let camera = planar.camera();
    collector.collect_views_at_tick(
        registry,
        particles,
        world,
        visible,
        presentation_tick,
        Some(frustum),
        camera.as_ref().map(|camera| (planar.visible(), camera)),
    );
    renderer.set_planar_particle_draws(collector.iter_mirror_collections());
}
