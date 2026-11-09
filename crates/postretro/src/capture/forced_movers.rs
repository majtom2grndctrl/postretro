// Capture's forced mover travel: a scene holds named movers at a travel
// fraction by setting their phase, so every input a frame derives from phase
// agrees: drawn geometry, shadow casters, occluder bounds, the portal seal and
// the mover travel state the game would hand the renderer.
// See: context/lib/rendering_pipeline.md §7.8 · §4 (Mover state lighting)

use anyhow::{Context, Result, bail};
use postretro_entities::{EntityRegistry, KinematicMoverComponent, Transform};
use postretro_level_loader::LevelWorld;
use postretro_physics::kinematic_mover::{
    MoverPathArcLengths, mover_is_docked_closed, set_mover_phase_at_path_fraction,
};

use super::scene::ForcedMover;
use crate::render::MoverTravel;
use crate::runtime_movers::{KinematicMoverRenderCollector, mover_entity};

/// The movers a scene forced, by mover id, in scene order.
#[derive(Debug, Default)]
pub(super) struct ForcedCaptureMovers {
    mover_ids: Vec<u32>,
}

impl ForcedCaptureMovers {
    /// Rest each forced mover at its travel fraction: its phase, then its
    /// transform from that phase, as a tick would leave them. Fails naming a
    /// mover the level lacks or a fraction its path cannot hold.
    pub(super) fn apply(
        registry: &mut EntityRegistry,
        world: &LevelWorld,
        forced: &[ForcedMover],
    ) -> Result<Self> {
        let mut mover_ids = Vec::with_capacity(forced.len());
        for force in forced {
            let mover_id = world
                .kinematic_geometry
                .movers
                .iter()
                .find(|mover| mover.name == force.name)
                .map(|mover| mover.mover_id)
                .with_context(|| {
                    format!("force_movers: no kinematic mover named `{}`", force.name)
                })?;
            let Some(entity) = mover_entity(registry, mover_id) else {
                bail!(
                    "force_movers: mover `{}` (id {mover_id}) did not spawn",
                    force.name
                );
            };
            let mut mover = registry
                .get_component::<KinematicMoverComponent>(entity)
                .with_context(|| format!("force_movers: mover `{}` component", force.name))?
                .clone();
            let lengths = MoverPathArcLengths::from_waypoints(&mover.waypoints);
            let position = set_mover_phase_at_path_fraction(&mut mover, force.t, &lengths)
                .with_context(|| format!("force_movers: mover `{}`", force.name))?;
            let mut transform =
                *registry
                    .get_component::<Transform>(entity)
                    .with_context(|| {
                        format!("force_movers: mover `{}` has no transform", force.name)
                    })?;
            transform.position = position;
            registry
                .set_component(entity, mover)
                .with_context(|| format!("force_movers: set mover `{}` phase", force.name))?;
            registry
                .set_component(entity, transform)
                .with_context(|| format!("force_movers: set mover `{}` transform", force.name))?;
            mover_ids.push(mover_id);
        }
        Ok(Self { mover_ids })
    }

    /// The blocked-portal input for the forced movers alone, by the live
    /// frame's rule: a mover docked closed at its first waypoint seals its
    /// portals. Movers a scene does not force seal nothing, as before.
    pub(super) fn blocked_portals(
        &self,
        registry: &EntityRegistry,
        world: &LevelWorld,
    ) -> Vec<bool> {
        let mut blocked = vec![false; world.portals.len()];
        for &mover_id in &self.mover_ids {
            let Some(mover) = mover_entity(registry, mover_id).and_then(|entity| {
                registry
                    .get_component::<KinematicMoverComponent>(entity)
                    .ok()
            }) else {
                continue;
            };
            if !mover_is_docked_closed(mover) {
                continue;
            }
            for &portal in &mover.sealed_portal_ids {
                if let Some(slot) = blocked.get_mut(portal as usize) {
                    *slot = true;
                }
            }
        }
        blocked
    }

    /// The forced movers' travel states, collected as the game collects them
    /// (travel fraction from phase, then the author ramp). Movers a scene
    /// does not force hand over no state and compose ungated, as before.
    pub(super) fn mover_travel(
        &self,
        registry: &EntityRegistry,
        world: &LevelWorld,
        collector: &mut KinematicMoverRenderCollector,
    ) -> Vec<MoverTravel> {
        collector.collect_mover_travel(registry, Some(world));
        collector
            .mover_travel()
            .iter()
            .filter(|state| self.mover_ids.contains(&state.mover_id))
            .copied()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;
    use postretro_level_loader::{KinematicGeometry, LoadedKinematicWaypoint, MoverLightRamp};

    use super::*;
    use crate::runtime_movers::tests::{mover, single_cell_world};
    use crate::runtime_movers::{ENGINE_AUTO_CLOSE_MS, spawn_loaded_kinematic_movers};

    /// Two 2 m movers on +x: `door` (id 1, ramp 0.5–1) and `lift` (id 2).
    fn world() -> LevelWorld {
        let mut geometry = KinematicGeometry::default();
        for (id, name, ramp) in [
            (
                1,
                "door",
                MoverLightRamp {
                    start: 0.5,
                    end: 1.0,
                },
            ),
            (2, "lift", MoverLightRamp::IDENTITY),
        ] {
            let mut loaded = mover(0);
            loaded.mover_id = id;
            loaded.name = name.to_string();
            loaded.path = format!("{name}0");
            loaded.origin = Vec3::ZERO;
            loaded.light_ramp = ramp;
            geometry.movers.push(loaded);
            geometry.waypoints.extend([
                LoadedKinematicWaypoint {
                    name: format!("{name}0"),
                    next: format!("{name}1"),
                    origin: Vec3::ZERO,
                },
                LoadedKinematicWaypoint {
                    name: format!("{name}1"),
                    next: String::new(),
                    origin: Vec3::new(2.0, 0.0, 0.0),
                },
            ]);
        }
        single_cell_world(geometry)
    }

    fn force(name: &str, t: f32) -> ForcedMover {
        ForcedMover {
            name: name.to_string(),
            t,
        }
    }

    #[test]
    fn forced_movers_rest_at_their_fraction_and_alone_hand_over_a_ramped_state() {
        let world = world();
        let mut registry = EntityRegistry::new();
        spawn_loaded_kinematic_movers(&mut registry, &world, ENGINE_AUTO_CLOSE_MS).unwrap();
        let forced =
            ForcedCaptureMovers::apply(&mut registry, &world, &[force("door", 0.75)]).unwrap();

        let door = mover_entity(&registry, 1).unwrap();
        let position = registry.get_component::<Transform>(door).unwrap().position;
        assert!(
            position.abs_diff_eq(Vec3::new(1.5, 0.0, 0.0), 1.0e-5),
            "{position}"
        );
        let lift = mover_entity(&registry, 2).unwrap();
        assert_eq!(
            registry.get_component::<Transform>(lift).unwrap().position,
            Vec3::ZERO,
            "an unforced mover keeps its seeded pose"
        );

        let mut collector = KinematicMoverRenderCollector::new();
        let states = forced.mover_travel(&registry, &world, &mut collector);
        assert_eq!(
            states.len(),
            1,
            "only the forced mover hands over: {states:?}"
        );
        assert_eq!(states[0].mover_id, 1);
        assert!(
            (states[0].t - 0.5).abs() <= 1.0e-5,
            "t 0.75 under ramp 0.5–1"
        );
        assert_eq!(
            forced.blocked_portals(&registry, &world).len(),
            world.portals.len()
        );
    }

    #[test]
    fn a_forced_mover_the_level_lacks_fails_by_name() {
        let world = world();
        let mut registry = EntityRegistry::new();
        spawn_loaded_kinematic_movers(&mut registry, &world, ENGINE_AUTO_CLOSE_MS).unwrap();
        let error = ForcedCaptureMovers::apply(&mut registry, &world, &[force("gate", 0.0)])
            .expect_err("no mover named gate");
        assert!(
            format!("{error:#}").contains("no kinematic mover named `gate`"),
            "{error:#}"
        );
    }
}
