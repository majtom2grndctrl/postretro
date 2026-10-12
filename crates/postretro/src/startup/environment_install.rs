// Level install of the environment volumes: the query set built once from the loaded records.
// See: context/lib/boot_sequence.md §3 (Level Install Order)

use std::time::Instant;

use postretro_collision::EnvironmentVolumes;
use postretro_level_loader::LevelWorld;

/// Build the level's environment volume set and log its one-time build cost.
/// Shared by the windowed install and the headless driver so both resolve
/// against the same set.
pub(crate) fn build_level_environment_volumes(world: &LevelWorld) -> EnvironmentVolumes {
    let started = Instant::now();
    // No session fluid table is resolved against yet, so every fluid volume
    // loads dry; its gravity and push still apply.
    let volumes =
        postretro_physics::environment_volumes::environment_volumes_from_level(world, |_| None);
    let build_ms = started.elapsed().as_secs_f64() * 1000.0;
    let leaf_count: usize = world
        .environment_volumes
        .volumes
        .iter()
        .map(|volume| volume.brushes.len())
        .sum();
    log::info!(
        "[Loader] environment volumes: {} volumes, {leaf_count} leaves, built in {build_ms:.3} ms",
        volumes.len(),
    );
    volumes
}
