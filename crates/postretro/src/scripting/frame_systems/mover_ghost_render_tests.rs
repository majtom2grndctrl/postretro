// Mover ghost collection: mapped-bounds culling, one instance per link with no mover-id
// dedupe, fading at the last in-volume pose, shadows untouched, and the clamp's wiring.
// See: context/lib/rendering_pipeline.md §7.9, §7.8 · context/lib/entity_model.md §7d

use glam::{Mat4, Quat, Vec3};
use postretro_entities::{
    EntityId, EntityRegistry, KinematicMoverComponent, KinematicMoverConfig, KinematicMoverMode,
    Transform,
};
use postretro_foundation::SeamlessMapping;
use postretro_level_loader::{
    CellData, CellLocatorChild, CellLocatorNodeData, KinematicGeometry, LevelWorld,
};
use postretro_render_cpu::flash_limiter::{LimiterFrameInput, pack_limiter_frame};
use postretro_render_cpu::ghost_instance::GHOST_LUMA;
use postretro_visibility::VisibleCells;

use super::*;

/// Cell 0 holds the real copy (x >= 0), cell 1 the other copy (x < 0).
const REAL_CELL: u32 = 0;
const MAPPED_CELL: u32 = 1;
/// The fixture mover (id 7): one triangle with local bounds [0,0,0]..[1,1,0].
const MOVER_ID: u32 = 7;
const SOURCE: Vec3 = Vec3::new(12.0, 0.0, 2.0);
const STYLE: GhostLook = GhostLook {
    tint: [0.6, 0.8, 1.0],
    opacity: 0.5,
    saturation: 0.25,
};

fn cell(min: Vec3, max: Vec3) -> CellData {
    CellData {
        bounds_min: min,
        bounds_max: max,
        face_start: 0,
        face_count: 0,
        portal_ref_start: 0,
        portal_ref_count: 0,
        is_solid: false,
        is_exterior: false,
        is_drawable: false,
    }
}

fn two_copy_world() -> LevelWorld {
    let mut world = LevelWorld::new_visibility_only(
        vec![
            cell(Vec3::new(0.0, -100.0, -100.0), Vec3::splat(100.0)),
            cell(Vec3::splat(-100.0), Vec3::new(0.0, 100.0, 100.0)),
        ],
        Vec::new(),
        CellLocatorChild::Node(0),
        vec![CellLocatorNodeData {
            plane_normal: Vec3::X,
            plane_distance: 0.0,
            front: CellLocatorChild::Cell(REAL_CELL as usize),
            back: CellLocatorChild::Cell(MAPPED_CELL as usize),
        }],
        Vec::new(),
        false,
    )
    .unwrap();
    world.kinematic_geometry = KinematicGeometry {
        movers: vec![crate::runtime_movers::tests::mover(1)],
        waypoints: Vec::new(),
    };
    world
}

/// Landmark in the real copy, targets in the other, one per link.
fn mapping(link: u16) -> SeamlessMapping {
    SeamlessMapping::new(
        Vec3::new(10.0, 0.0, 0.0),
        0.0,
        Vec3::new(-20.0, 0.0, f32::from(link) * 10.0),
        std::f32::consts::FRAC_PI_2,
    )
}

fn spawn_mover(registry: &mut EntityRegistry, at: Vec3) -> EntityId {
    let id = registry.spawn(Transform {
        position: at,
        ..Default::default()
    });
    registry
        .set_component(
            id,
            KinematicMoverComponent::new(
                MOVER_ID,
                KinematicMoverConfig {
                    waypoints: vec![at, at + Vec3::Y],
                    waypoint_names: vec!["closed".into(), "open".into()],
                    speed_mps: 1.0,
                    wait_ms: 0.0,
                    mode: KinematicMoverMode::PingPong,
                    started: false,
                    spin_axis: Vec3::ZERO,
                    initial_spin_rate_rad_s: 0.0,
                    spin_accel_rad_s2: 0.0,
                    carry_yaw: false,
                },
            ),
        )
        .unwrap();
    id
}

fn pose(at: Vec3) -> Transform {
    Transform {
        position: at,
        rotation: Quat::from_rotation_y(0.4),
        scale: Vec3::ONE,
    }
}

fn member(entity: EntityId, link: u16, source: Transform, frozen: bool) -> GhostMember {
    let mapping = mapping(link);
    GhostMember {
        link,
        entity,
        category: GhostCategory::Movers,
        membership: 1.0,
        frozen,
        mapping,
        source,
        mapped: Transform {
            position: mapping.point(source.position),
            rotation: mapping.rotation() * source.rotation,
            scale: source.scale,
        },
    }
}

fn expected_model(member: &GhostMember) -> Mat4 {
    member.mapping.matrix() * transform_matrix(member.source)
}

struct Fixture {
    world: LevelWorld,
    movers: KinematicMoverRenderCollector,
    ghosts: MoverGhostRenderCollector,
}

impl Fixture {
    fn new() -> Self {
        Self {
            world: two_copy_world(),
            movers: KinematicMoverRenderCollector::new(),
            ghosts: MoverGhostRenderCollector::new(),
        }
    }

    /// The frame-loop order: the real movers collect first, then their ghosts.
    fn collect(
        &mut self,
        registry: &EntityRegistry,
        members: &[GhostMember],
        visible: &VisibleCells,
        look: impl Fn(u16) -> GhostLook,
        limiter: Option<LimiterFrameUniform>,
    ) {
        self.movers.collect(registry, &self.world, visible, 1.0);
        self.ghosts.collect(
            registry,
            members,
            look,
            &MoverGhostFrame {
                world: &self.world,
                visible,
                movers: &self.movers,
                limiter,
            },
        );
    }
}

// Acceptance (AC14, movers): a mover ghost draws when its mapped bounds touch a
// visible cell and not otherwise, with the real mover's cell visible and not.
#[test]
fn mover_ghost_draws_when_its_mapped_cell_is_visible_whatever_the_real_cell() {
    let mut registry = EntityRegistry::new();
    let mover = spawn_mover(&mut registry, SOURCE);
    let members = [member(mover, 0, pose(SOURCE), false)];
    let mut fixture = Fixture::new();
    assert_eq!(
        fixture.world.locate_cell(members[0].mapped.position) as u32,
        MAPPED_CELL,
        "fixture: the mapping lands in the other copy"
    );
    for (visible, draws) in [
        (vec![REAL_CELL, MAPPED_CELL], true),
        (vec![MAPPED_CELL], true),
        (vec![REAL_CELL], false),
        (vec![], false),
    ] {
        fixture.collect(
            &registry,
            &members,
            &VisibleCells::Culled(visible.clone()),
            |_| STYLE,
            None,
        );
        assert_eq!(
            fixture.ghosts.instances().len(),
            usize::from(draws),
            "visible cells {visible:?}"
        );
    }
}

// P27: one mover in two links draws two instances at two mapped transforms; the
// ghost list never deduplicates by mover id. Ghosts never join the real mover's
// shadow casters or occluder bounds.
#[test]
fn a_mover_in_two_links_draws_two_ghost_instances_and_no_extra_shadow_casters() {
    let mut registry = EntityRegistry::new();
    let mover = spawn_mover(&mut registry, SOURCE);
    let members = [
        member(mover, 0, pose(SOURCE), false),
        member(mover, 1, pose(SOURCE), false),
    ];
    let mut fixture = Fixture::new();
    fixture.collect(&registry, &members, &VisibleCells::DrawAll, |_| STYLE, None);

    let ghosts = fixture.ghosts.instances();
    assert_eq!(ghosts.len(), 2);
    assert!(ghosts.iter().all(|ghost| ghost.mover_id == MOVER_ID));
    for (ghost, member) in ghosts.iter().zip(&members) {
        assert!(ghost.transform.abs_diff_eq(expected_model(member), 1e-5));
    }
    assert_ne!(ghosts[0].transform, ghosts[1].transform);
    assert_eq!(
        fixture.movers.shadow_instances().len(),
        1,
        "the real mover only"
    );
    assert_eq!(fixture.movers.occluder_aabbs().len(), 1);
    assert_eq!(fixture.movers.instances().len(), 1);
}

// A fading mover ghost (disarm, category off) draws at its last in-volume
// mapped transform while the real mover has moved on.
#[test]
fn a_fading_mover_ghost_draws_at_its_last_in_volume_pose_not_the_live_one() {
    let mut registry = EntityRegistry::new();
    let mover = spawn_mover(&mut registry, SOURCE + Vec3::Y * 3.0);
    let frozen = member(mover, 0, pose(SOURCE), true);
    let mut fixture = Fixture::new();
    fixture.collect(
        &registry,
        &[GhostMember {
            membership: 0.4,
            ..frozen
        }],
        &VisibleCells::DrawAll,
        |_| STYLE,
        None,
    );
    let ghost = fixture.ghosts.instances()[0];
    assert!(ghost.transform.abs_diff_eq(expected_model(&frozen), 1e-5));
    assert!((ghost.look.opacity - STYLE.opacity * 0.4).abs() < 1e-6);
    assert_eq!(ghost.look.tint, STYLE.tint);
    assert_eq!(ghost.look.saturation, STYLE.saturation);
}

// Only mover members draw here, and a ghost at zero opacity draws nothing.
#[test]
fn non_mover_members_and_zero_opacity_mover_ghosts_draw_nothing() {
    let mut registry = EntityRegistry::new();
    let mover = spawn_mover(&mut registry, SOURCE);
    let pawn = registry.spawn(Transform::default());
    let mut fixture = Fixture::new();
    fixture.collect(
        &registry,
        &[
            GhostMember {
                membership: 0.0,
                ..member(mover, 0, pose(SOURCE), false)
            },
            GhostMember {
                category: GhostCategory::Players,
                ..member(pawn, 0, pose(SOURCE), false)
            },
        ],
        &VisibleCells::DrawAll,
        |_| STYLE,
        None,
    );
    assert!(fixture.ghosts.instances().is_empty());
}

// AC 24a wiring: a mover ghost's tint strobing white and black at 10 Hz at
// steady opacity is limited through its tint-luminance channel; the limiter
// off passes it through.
#[test]
fn a_mover_ghost_tint_strobe_is_limited_through_its_luminance_channel() {
    let mut registry = EntityRegistry::new();
    let mover = spawn_mover(&mut registry, SOURCE);
    let members = [member(mover, 0, pose(SOURCE), false)];
    let dt = 1.0 / 60.0;
    for enabled in [true, false] {
        let mut fixture = Fixture::new();
        let mut changes = 0;
        let mut last = None;
        for n in 0..120 {
            let white = (n as f32 * dt * 10.0).fract() >= 0.5;
            let tint = if white { [1.0; 3] } else { [0.0; 3] };
            fixture.collect(
                &registry,
                &members,
                &VisibleCells::DrawAll,
                |_| GhostLook {
                    tint,
                    opacity: 1.0,
                    saturation: 1.0,
                },
                Some(pack_limiter_frame(
                    LimiterFrameInput {
                        elapsed_seconds: dt,
                    },
                    enabled,
                    false,
                )),
            );
            let look = fixture.ghosts.instances()[0].look;
            assert_eq!(look.opacity, 1.0, "opacity steady");
            let luminance: f32 = (0..3).map(|i| look.tint[i] * GHOST_LUMA[i]).sum();
            if last.is_some_and(|last: f32| (luminance - last).abs() >= 0.5) {
                changes += 1;
            }
            last = Some(luminance);
        }
        if enabled {
            assert!(
                changes <= 6,
                "limited: {changes} full swings in two seconds"
            );
        } else {
            assert!(changes > 20, "unlimited: {changes} swings");
        }
    }
}
