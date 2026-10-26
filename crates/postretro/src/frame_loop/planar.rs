// The frame's planar mirror step: after the camera walk, before any visibility consumer.
// See: context/lib/rendering_pipeline.md §7.3a

use crate::App;
use crate::camera::RenderCamera;
use crate::render_preparation::VisibleRenderPreparation;

/// O1: the camera walk (`main`) has run. Read the tier once, from the tier
/// the renderer draws this frame with; then select, seed from this frame's
/// camera reach, walk the mirror on its own scratch, and join its reach to
/// light reachability. The walk's time lands in `main`'s visibility stages,
/// which the frame folds under `visibility`. Below very high nothing runs.
pub(super) fn prepare_planar_mirror(
    app: &mut App,
    camera: &RenderCamera,
    main: &mut VisibleRenderPreparation,
) {
    let Some(world) = app.level.as_ref() else {
        return;
    };
    let preset = app
        .renderer
        .as_ref()
        .and_then(|renderer| renderer.planar_reflection_preset());
    app.planar_render.prepare(
        world,
        camera,
        main,
        preset,
        &app.blocked_portals,
        &main.stats.cpu,
    );
    main.include_mirror_lighting(world, &app.planar_render);
}
