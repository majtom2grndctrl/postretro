// Level install of the environment volumes: the query set built once from the loaded records.
// See: context/lib/boot_sequence.md §3 (Level Install Order)

use std::time::Instant;

use postretro_collision::EnvironmentVolumes;
use postretro_combat_model::FluidMovementTable;
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{ComponentKind, ComponentValue};
use postretro_foundation::{MovementState, PlayerMovementComponent};
use postretro_level_loader::LevelWorld;

use crate::App;

/// Where this machine's fluid table comes from, which decides which fluid
/// names the level's volumes resolve and who warns about an undeclared name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum FluidTableSource {
    /// Offline or hosting: this machine's own manifest. It is the only source
    /// that warns on an undeclared fluid name.
    #[default]
    OwnManifest,
    /// A connected client with the host's table from an installed tuning
    /// payload. A name the host lacks loads dry silently: the host warned.
    HostTuning,
    /// A connected client with no host table: before the first tuning
    /// installs, or after a demotion dropped it. The volumes build silently
    /// with every fluid dry, since the client may never resolve names against
    /// its own registry; gravity and push still hold for anything resolving
    /// meanwhile. The host's table re-resolves the fluids when it arrives.
    AwaitingHost,
}

/// Build the level's environment volume set and log its one-time build cost.
/// Resolving against this machine's own manifest: a fluid name the table lacks
/// loads dry with a warning, while that volume's gravity and push still apply.
/// `fluid_names` is the table, in index order. Shared by the headless driver
/// and every install or rebuild that resolves against the own manifest.
pub(crate) fn build_level_environment_volumes(
    world: &LevelWorld,
    fluid_names: &[String],
) -> EnvironmentVolumes {
    for record in &world.environment_volumes.volumes {
        if !record.fluid.is_empty() && fluid_index(fluid_names, &record.fluid).is_none() {
            log::warn!(
                "[Loader] fluid_volume entity {} names undeclared fluid '{}'; it loads dry",
                record.source_entity,
                record.fluid,
            );
        }
    }
    build_resolved(world, fluid_names)
}

/// The volume set for `source`, as every App install and rebuild takes it.
fn environment_volumes_for(
    world: &LevelWorld,
    fluid_names: &[String],
    source: FluidTableSource,
) -> EnvironmentVolumes {
    match source {
        FluidTableSource::OwnManifest => build_level_environment_volumes(world, fluid_names),
        FluidTableSource::HostTuning => build_resolved(world, fluid_names),
        FluidTableSource::AwaitingHost => build_resolved(world, &[]),
    }
}

/// Build without diagnosing names; logs the build cost.
fn build_resolved(world: &LevelWorld, fluid_names: &[String]) -> EnvironmentVolumes {
    let started = Instant::now();
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
    /// The active fluid table, without its source; tests read it.
    #[cfg(test)]
    pub(crate) fn active_environment_fluids(&self) -> FluidMovementTable {
        self.active_fluid_table().0
    }

    /// The fluid table this machine resolves level fluid names against, in
    /// index order, and where it came from. Offline or hosting: the session
    /// manifest's declarations. A connected client: the host's table from the
    /// installed tuning payload, none (every fluid dry) before one installs,
    /// and never its own registry, because admission gates on mod id, not
    /// version. Every install and rebuild reads this one source.
    fn active_fluid_table(&self) -> (FluidMovementTable, FluidTableSource) {
        let Some(session) = self.session.as_ref() else {
            return (FluidMovementTable::default(), FluidTableSource::OwnManifest);
        };
        match session.net_endpoint.as_ref() {
            Some(endpoint @ crate::netcode::NetEndpoint::Client { .. }) => {
                match endpoint.client_host_environment() {
                    Some(environment) => (environment.fluids.clone(), FluidTableSource::HostTuning),
                    None => (
                        FluidMovementTable::default(),
                        FluidTableSource::AwaitingHost,
                    ),
                }
            }
            _ => (
                FluidMovementTable::from_registry(
                    &session.scripting.script_ctx.data_registry.borrow().fluids,
                ),
                FluidTableSource::OwnManifest,
            ),
        }
    }

    /// Install `world`'s environment volumes against the active fluid table.
    /// The level install calls this before the parity digest and any tick.
    pub(crate) fn install_environment_volumes(&mut self, world: &LevelWorld) {
        let (fluids, source) = self.active_fluid_table();
        self.environment_volumes = environment_volumes_for(world, fluids.names(), source);
        self.environment_fluids = fluids;
        self.environment_fluid_source = source;
        self.install_planar_candidates(world);
    }

    /// Re-read the active fluid table and, when its names or source differ
    /// from the ones the installed volumes resolved against, re-resolve the
    /// installed level's fluid volumes, so a newly declared fluid stops
    /// loading dry, and a client's dry fluids resolve once the host's table
    /// arrives. Source-agnostic: a host manifest reload, a client's tuning
    /// install or a client's demotion all land here. A viscosity-only change
    /// replaces the table without a rebuild. No level leaves only the table
    /// updated and the drift cleared. Every call re-resolves the fluid surfaces'
    /// drift, after any rebuild, since a viscosity or a terminal velocity alone
    /// changes it.
    ///
    /// A renamed table moves fluid indices, so every swimming pawn's fluid is
    /// remapped by name first: its exit edge then names the fluid it is in.
    /// A pawn whose fluid left the table keeps its stale index; its next tick
    /// resolves the fluid gone and leaves swim.
    pub(crate) fn refresh_environment_fluids(&mut self) {
        let (fluids, source) = self.active_fluid_table();
        let renamed = fluids.names() != self.environment_fluids.names();
        let resourced = source != self.environment_fluid_source;
        if renamed {
            self.remap_swimming_pawn_fluids(&fluids);
        }
        self.environment_fluids = fluids;
        self.environment_fluid_source = source;
        if (renamed || resourced)
            && let Some(world) = self.level.as_ref()
        {
            self.environment_volumes =
                environment_volumes_for(world, self.environment_fluids.names(), source);
        }
        self.resolve_fluid_surface_drift();
        if let Some(world) = self.level.as_ref() {
            let reflecting = self.planar_reflecting_volumes(world);
            self.planar_render.install(world, &reflecting);
        }
    }

    /// Point every `Swim { fluid }` pawn at `next`'s index for the name its
    /// fluid had in the current table.
    fn remap_swimming_pawn_fluids(&self, next: &FluidMovementTable) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let previous = self.environment_fluids.names();
        let mut registry = session.scripting.script_ctx.registry.borrow_mut();
        registry.for_each_with_kind_mut(ComponentKind::PlayerMovement, |_, value| {
            let ComponentValue::PlayerMovement(movement) = value else {
                return;
            };
            let MovementState::Swim { fluid } = &mut movement.movement_state else {
                return;
            };
            let remapped = previous
                .get(usize::from(*fluid))
                .and_then(|name| fluid_index(next.names(), name));
            if let Some(remapped) = remapped {
                *fluid = remapped;
            }
        });
    }

    /// A level whose installed volumes include a fluid (one that resolved
    /// against the active table, so not dry), installed for a local player
    /// whose movement descriptor has no `movement.swim` block: warn once, at
    /// install, naming the descriptor. That player wades, walking fluid floors
    /// under gravity, which is a legitimate wade-only design. Runs after the boot
    /// pawn spawns, so it covers single-player and the listen host; a connected
    /// client's pawn arrives later from the host baseline and is not checked.
    pub(crate) fn warn_if_local_player_cannot_swim(&self) {
        let volumes = &self.environment_volumes;
        let has_fluid = (0..volumes.len()).any(|index| {
            volumes
                .volume(index)
                .is_some_and(|volume| volume.fluid.is_some())
        });
        if !has_fluid {
            return;
        }
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let registry = session.scripting.script_ctx.registry.borrow();
        let Some(pawn) = registry.local_player_pawn() else {
            return;
        };
        let Ok(movement) = registry.get_component::<PlayerMovementComponent>(pawn) else {
            return;
        };
        if movement.swim.is_some() {
            return;
        }
        let descriptor = registry
            .get_component::<DescriptorProvenance>(pawn)
            .map(|provenance| provenance.canonical_name.as_str())
            .unwrap_or("player");
        log::warn!(
            "[Movement] player descriptor '{descriptor}' has no movement.swim block; this level has fluid volumes, so the player wades through them and cannot swim"
        );
    }
}
