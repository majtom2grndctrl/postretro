// Level install of the environment volumes: the query set built once from the loaded records.
// See: context/lib/boot_sequence.md §3 (Level Install Order)

use std::time::Instant;

use postretro_collision::EnvironmentVolumes;
use postretro_level_loader::LevelWorld;

use crate::App;

/// Build the level's environment volume set and log its one-time build cost.
/// Shared by the windowed install, the headless driver and the manifest
/// reload, so all resolve against the same set. `fluid_names` is the fluid
/// table the volumes' names resolve against, in index order; a name it lacks
/// loads dry with a warning, while that volume's gravity and push still apply.
pub(crate) fn build_level_environment_volumes(
    world: &LevelWorld,
    fluid_names: &[String],
) -> EnvironmentVolumes {
    let started = Instant::now();
    for record in &world.environment_volumes.volumes {
        if !record.fluid.is_empty() && fluid_index(fluid_names, &record.fluid).is_none() {
            log::warn!(
                "[Loader] fluid_volume entity {} names undeclared fluid '{}'; it loads dry",
                record.source_entity,
                record.fluid,
            );
        }
    }
    let volumes =
        postretro_physics::environment_volumes::environment_volumes_from_level(world, |name| {
            fluid_index(fluid_names, name)
        });
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

/// A fluid's index is its position in the table. The registry caps the table
/// at what `u16` indexes, so the conversion only fails past that cap.
fn fluid_index(fluid_names: &[String], name: &str) -> Option<u16> {
    let position = fluid_names.iter().position(|declared| declared == name)?;
    u16::try_from(position).ok()
}

impl App {
    /// The fluid table this machine resolves level fluid names against, in
    /// index order: the session manifest's declarations. This is the one
    /// source every install and rebuild reads. Task 6b points a connected
    /// client at the host's table from the tuning payload here, never its own.
    pub(crate) fn environment_fluid_names(&self) -> Vec<String> {
        self.session
            .as_ref()
            .map(|session| {
                session
                    .scripting
                    .script_ctx
                    .data_registry
                    .borrow()
                    .fluids
                    .names()
            })
            .unwrap_or_default()
    }

    /// After a committed manifest reload, re-resolve the installed level's
    /// fluid volumes when the fluid table changed, so a newly declared fluid
    /// stops loading dry. No level, or an unchanged table, leaves the set as is.
    pub(crate) fn refresh_environment_fluids(&mut self, previous_names: &[String]) {
        let names = self.environment_fluid_names();
        if names == previous_names {
            return;
        }
        let Some(world) = self.level.as_ref() else {
            return;
        };
        self.environment_volumes = build_level_environment_volumes(world, &names);
    }
}
