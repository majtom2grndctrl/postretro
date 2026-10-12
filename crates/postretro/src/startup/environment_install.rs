// Level install of the environment volumes: the query set built once from the loaded records.
// See: context/lib/boot_sequence.md §3 (Level Install Order)

use std::cell::Ref;
use std::time::Instant;

use postretro_collision::EnvironmentVolumes;
use postretro_combat_model::FluidMovementTable;
use postretro_entities::provenance::DescriptorProvenance;
use postretro_foundation::{FluidDescriptor, PlayerMovementComponent};
use postretro_level_loader::LevelWorld;

use crate::App;

/// Build the level's environment volume set and log its one-time build cost.
/// Shared by the windowed install, the headless driver and every fluid-table
/// change (a host manifest reload, a client's tuning install), so all resolve
/// against the same set. `fluid_names` is the active fluid table the volumes'
/// names resolve against, in index order; a name it lacks loads dry with a
/// warning, while that volume's gravity and push still apply.
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
    /// index order. Offline or hosting: the session manifest's declarations.
    /// A connected client: the host's table from the installed tuning payload,
    /// empty (every fluid dry) before one installs, and never its own registry,
    /// because admission gates on mod id, not version. Every install and
    /// rebuild reads this one source.
    pub(crate) fn active_environment_fluids(&self) -> FluidMovementTable {
        let Some(session) = self.session.as_ref() else {
            return FluidMovementTable::default();
        };
        match session.net_endpoint.as_ref() {
            Some(endpoint @ crate::netcode::NetEndpoint::Client { .. }) => endpoint
                .client_host_environment()
                .map(|environment| environment.fluids.clone())
                .unwrap_or_default(),
            _ => FluidMovementTable::from_registry(
                &session.scripting.script_ctx.data_registry.borrow().fluids,
            ),
        }
    }

    /// Re-read the active fluid table and, when its names differ from the ones
    /// the installed volumes resolved against, re-resolve the installed level's
    /// fluid volumes, so a newly declared fluid stops loading dry. Source-
    /// agnostic: a host manifest reload, a client's tuning install or a
    /// client's demotion all land here. A viscosity-only change replaces the
    /// table without a rebuild. No level leaves only the table updated.
    pub(crate) fn refresh_environment_fluids(&mut self) {
        let fluids = self.active_environment_fluids();
        let renamed = fluids.names() != self.environment_fluids.names();
        self.environment_fluids = fluids;
        if !renamed {
            return;
        }
        let Some(world) = self.level.as_ref() else {
            return;
        };
        self.environment_volumes =
            build_level_environment_volumes(world, self.environment_fluids.names());
    }

    /// This machine's presentation for fluid `fluid` (an index into the active
    /// table, as a resolved environment carries it): the LOCAL manifest's
    /// declaration of that fluid's name, so a client shows its own tint, muffle,
    /// surface opacity and sounds for the host's fluid. `None` when the local
    /// manifest does not declare it: that machine shows none of them.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "E24 tasks 10 and 11 consume the presentation lookup"
        )
    )]
    pub(crate) fn fluid_presentation(&self, fluid: u16) -> Option<Ref<'_, FluidDescriptor>> {
        let session = self.session.as_ref()?;
        Ref::filter_map(
            session.scripting.script_ctx.data_registry.borrow(),
            |registry| {
                self.environment_fluids
                    .local_presentation(fluid, &registry.fluids)
            },
        )
        .ok()
    }

    /// The presentation for installed volume `volume` (map order, as
    /// `postretro_collision::fluid_at_eye` reports it) through the fluid it
    /// resolved to. `None` for a dry volume or one the local manifest lacks.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "E24 tasks 10 and 11 consume the presentation lookup"
        )
    )]
    pub(crate) fn fluid_presentation_for_volume(
        &self,
        volume: usize,
    ) -> Option<Ref<'_, FluidDescriptor>> {
        let fluid = self.environment_volumes.volume(volume)?.fluid?;
        self.fluid_presentation(fluid)
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
