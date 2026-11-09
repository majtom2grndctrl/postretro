// Capture's VM-free receivers: spawn, model validation and draw collection against the main, drawn
// and mirror cell sets. See: context/lib/rendering_pipeline.md §7.8

use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use glam::Vec3;
use postretro_entities::{ComponentKind, ComponentValue, EntityRegistry};
use postretro_visibility::VisibleCells;

use crate::runtime_movers::{
    ENGINE_AUTO_CLOSE_MS, KinematicMoverRenderCollector, spawn_loaded_kinematic_movers,
};
use crate::scripting::builtins::{ClassnameDispatch, apply_classname_dispatch, register_builtins};
use crate::scripting::map_entity::MapEntity;
use crate::scripting_systems::attachments::SocketPoseResolver;
use crate::scripting_systems::ghost_render::{GhostDrawFrame, GhostRenderCollector};
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::scripting_systems::mesh_anim::MeshClipTables;
use crate::scripting_systems::mesh_render::MeshRenderCollector;
use crate::scripting_systems::mover_ghost_render::{MoverGhostFrame, MoverGhostRenderCollector};

/// Materialize just the VM-free receivers capture can render at a loaded
/// instant. This deliberately does not call `install_world_cpu`: that path
/// also runs data scripts and fires `levelLoad`.
pub(super) fn spawn_capture_receiver_registry(
    world: &postretro_level_loader::LevelWorld,
) -> Result<EntityRegistry> {
    let mut registry = EntityRegistry::new();
    spawn_loaded_kinematic_movers(&mut registry, world, ENGINE_AUTO_CLOSE_MS)
        .context("failed to spawn capture kinematic movers")?;

    // `MapEntity` is the scripting-facing adapter over PRL records. Built-in
    // dispatch is VM-free, so this gives capture the exact map-authored
    // `prop_mesh` spawn contract without admitting the data-script sweep.
    let map_entities: Vec<MapEntity> = world.map_entities.iter().cloned().map(Into::into).collect();
    let mut dispatch = ClassnameDispatch::new();
    register_builtins(&mut dispatch);
    apply_classname_dispatch(&map_entities, &dispatch, &mut registry);

    Ok(registry)
}

/// Validate model handles placed by capture's built-in map dispatch. An empty
/// prop handle must fail capture instead of silently removing its draw.
/// The renderer cache key is this verbatim string, so load it before the mesh
/// collector submits an instance using the same handle.
pub(super) fn capture_mesh_models(registry: &EntityRegistry) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for (id, value) in registry.iter_with_kind(ComponentKind::Mesh) {
        let ComponentValue::Mesh(mesh) = value else {
            continue;
        };
        if mesh.model.is_empty() {
            bail!("capture prop_mesh receiver {id:?} has an absent or empty `model` key");
        }
        if seen.insert(mesh.model.clone()) {
            models.push(mesh.model.clone());
        }
    }
    Ok(models)
}

/// Test entry, with no mirror.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) fn collect_capture_receiver_draws(
    registry: &EntityRegistry,
    world: &postretro_level_loader::LevelWorld,
    visible_cells: &VisibleCells,
    eye: Vec3,
    mover_collector: &mut KinematicMoverRenderCollector,
    mesh_collector: &mut MeshRenderCollector,
    ghost_links: &mut crate::ghost_links::GhostLinks,
    ghost_collector: &mut GhostRenderCollector,
    mover_ghost_collector: &mut MoverGhostRenderCollector,
) {
    collect_capture_receiver_draws_with_mirror(
        registry,
        world,
        CaptureCullViews {
            main: visible_cells,
            drawn: visible_cells,
            mirror: None,
        },
        eye,
        mover_collector,
        mesh_collector,
        ghost_links,
        ghost_collector,
        mover_ghost_collector,
    );
}

/// The cell sets capture's receivers collect against, as the play frame's:
/// the main view's (meshes' forward draws and shadows, ghosts, the mover
/// ghost cull), every drawn cell (movers' beauty draws and SH sample
/// regions), and the planar mirror's (mirror-reached meshes), `None` with no
/// mirror.
pub(super) struct CaptureCullViews<'a> {
    pub(super) main: &'a VisibleCells,
    pub(super) drawn: &'a VisibleCells,
    pub(super) mirror: Option<&'a VisibleCells>,
}

/// Mirror the windowed render-frame collector calls for capture's spawned
/// receivers, with the planar mirror's sets. A single-instant capture has no
/// tick history or animation clock: alpha is therefore 1.0 and animation time
/// is 0.0. Ghosts evaluate once at rest and present each link's style from the
/// scene's own table, or the floor style without one (capture loads no
/// manifest), never through the flash clamp.
#[allow(clippy::too_many_arguments)]
pub(super) fn collect_capture_receiver_draws_with_mirror(
    registry: &EntityRegistry,
    world: &postretro_level_loader::LevelWorld,
    views: CaptureCullViews<'_>,
    eye: Vec3,
    mover_collector: &mut KinematicMoverRenderCollector,
    mesh_collector: &mut MeshRenderCollector,
    ghost_links: &mut crate::ghost_links::GhostLinks,
    ghost_collector: &mut GhostRenderCollector,
    mover_ghost_collector: &mut MoverGhostRenderCollector,
) {
    let visible_cells = views.main;
    mover_collector.collect_with_mirror(registry, world, views.main, views.drawn, 1.0);

    let clip_tables = MeshClipTables::new();
    let hit_zones = HitZoneStore::new();
    mesh_collector.collect_with_mirror(
        registry,
        world,
        visible_cells,
        views.mirror,
        1.0,
        0.0,
        &clip_tables,
        eye,
        &hit_zones,
    );

    ghost_links.evaluate_for_capture(registry);
    ghost_collector.collect(
        registry,
        ghost_links.members(),
        &GhostDrawFrame {
            world,
            visible: visible_cells,
            anim_time: 0.0,
            camera_pos: eye,
            tables: &clip_tables,
            socket_poses: &SocketPoseResolver::new(&hit_zones),
            force_resample_model: &|_| false,
            limiter: None,
            // A capture never involves the limiter.
            transparency: Default::default(),
        },
    );
    mover_ghost_collector.collect(
        registry,
        ghost_links.members(),
        &MoverGhostFrame {
            world,
            visible: visible_cells,
            movers: mover_collector,
            limiter: None,
            transparency: Default::default(),
        },
    );
}
