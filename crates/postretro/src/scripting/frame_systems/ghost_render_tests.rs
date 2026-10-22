// Ghost draw collection: mapped-cell culling, holder + weapon groups at the mapped
// transform, fading ghosts after despawn, and the per-ghost flash clamp's wiring.
// See: context/lib/rendering_pipeline.md §7.9, §7.8 · context/lib/entity_model.md §7d

use glam::{Mat4, Quat, Vec3};
use postretro_entities::components::mesh::{AttachmentBinding, MeshAttachment, MeshComponent};
use postretro_entities::{EntityRegistry, SlotTable, SlotValue, Transform};
use postretro_foundation::SeamlessMapping;
use postretro_level_loader::{CellData, CellLocatorChild, CellLocatorNodeData, LevelWorld};
use postretro_render_cpu::flash_clamp::INTENSITY_RATE_PER_SECOND;
use postretro_render_cpu::flash_limiter::{
    FLASH_LIMITER_SLOT, HITCH_CEILING_SECONDS, LimiterFrameInput, pack_limiter_frame,
};
use postretro_visibility::VisibleCells;

use super::*;
use crate::ghost_links::{GhostCategory, GhostLook};

/// Cell 0 holds the real copy (x >= 0), cell 1 the other copy (x < 0).
const REAL_CELL: usize = 0;
const MAPPED_CELL: usize = 1;
const SOURCE: Vec3 = Vec3::new(12.0, 0.0, 2.0);
const STYLE: GhostLook = GhostLook {
    tint: [0.6, 0.8, 1.0],
    opacity: 0.5,
    saturation: 0.25,
};
const SOCKET: Vec3 = Vec3::new(0.3, 1.2, 0.1);

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
    LevelWorld::new_visibility_only(
        vec![
            cell(Vec3::new(0.0, -100.0, -100.0), Vec3::splat(100.0)),
            cell(Vec3::splat(-100.0), Vec3::new(0.0, 100.0, 100.0)),
        ],
        Vec::new(),
        CellLocatorChild::Node(0),
        vec![CellLocatorNodeData {
            plane_normal: Vec3::X,
            plane_distance: 0.0,
            front: CellLocatorChild::Cell(REAL_CELL),
            back: CellLocatorChild::Cell(MAPPED_CELL),
        }],
        Vec::new(),
        false,
    )
    .unwrap()
}

/// Landmark in the real copy, target in the other, a quarter turn apart.
fn mapping() -> SeamlessMapping {
    SeamlessMapping::new(
        Vec3::new(10.0, 0.0, 0.0),
        0.0,
        Vec3::new(-20.0, 0.0, 0.0),
        std::f32::consts::FRAC_PI_2,
    )
}

/// A pawn with a rigidly mounted third-person weapon.
fn spawn_pawn(registry: &mut EntityRegistry, at: Vec3) -> EntityId {
    let id = registry.spawn(Transform {
        position: at,
        ..Default::default()
    });
    let mut mesh = MeshComponent::stateless("models/pawn.gltf".to_string());
    mesh.attachments = vec![MeshAttachment {
        socket: "hand".to_string(),
        model: "models/gun.gltf".to_string(),
        binding: AttachmentBinding::Rigid(Mat4::from_translation(SOCKET)),
    }];
    registry.set_component(id, mesh).unwrap();
    id
}

fn source_pose() -> Transform {
    Transform {
        position: SOURCE,
        rotation: Quat::from_rotation_y(0.4),
        scale: Vec3::ONE,
    }
}

/// `members` each carrying the look `look` gives it, as membership hands them
/// to the draw.
fn with_looks(
    members: &[GhostMember],
    look: impl Fn(&GhostMember) -> GhostLook,
) -> Vec<GhostMember> {
    members
        .iter()
        .map(|member| GhostMember {
            look: look(member),
            ..*member
        })
        .collect()
}

fn member(entity: EntityId, membership: f32, frozen: bool) -> GhostMember {
    let mapping = mapping();
    let source = source_pose();
    GhostMember {
        link: 0,
        entity,
        category: GhostCategory::Players,
        membership,
        frozen,
        mapping,
        source,
        mapped: Transform {
            position: mapping.point(source.position),
            rotation: mapping.rotation() * source.rotation,
            scale: source.scale,
        },
        look: STYLE,
    }
}

struct Fixture {
    world: LevelWorld,
    tables: MeshClipTables,
    collector: GhostRenderCollector,
}

impl Fixture {
    fn new() -> Self {
        Self {
            world: two_copy_world(),
            tables: MeshClipTables::new(),
            collector: GhostRenderCollector::new(),
        }
    }

    fn collect_with(
        &mut self,
        registry: &EntityRegistry,
        members: &[GhostMember],
        visible: &VisibleCells,
        look: impl Fn(&GhostMember) -> GhostLook,
        limiter: Option<LimiterFrameUniform>,
    ) {
        let socket_poses = SocketPoseResolver::empty();
        let members = with_looks(members, look);
        self.collector.collect(
            registry,
            &members,
            &GhostDrawFrame {
                world: &self.world,
                visible,
                anim_time: 0.0,
                camera_pos: Vec3::ZERO,
                tables: &self.tables,
                socket_poses: &socket_poses,
                force_resample_model: &|_| false,
                limiter,
            },
        );
    }

    fn collect(&mut self, registry: &EntityRegistry, members: &[GhostMember]) {
        self.collect_with(registry, members, &VisibleCells::DrawAll, |_| STYLE, None);
    }

    /// The emitted holder's presented opacity, 0 when nothing drew.
    fn presented(&self) -> (f32, [f32; 3]) {
        match self.collector.instances().first().map(|i| i.presentation) {
            Some(MeshPresentation::Ghost(look)) => (look.opacity, look.tint),
            Some(other) => panic!("ghost collector emitted {other:?}"),
            None => (0.0, [0.0; 3]),
        }
    }
}

/// The real draw's holder transform for the source pose: what the ghost maps.
fn real_holder_transform() -> Mat4 {
    let source = source_pose();
    Mat4::from_scale_rotation_translation(source.scale, source.rotation, source.position)
}

#[test]
fn ghost_draws_when_its_mapped_cell_is_visible_whatever_the_real_cell() {
    let mut registry = EntityRegistry::new();
    let pawn = spawn_pawn(&mut registry, SOURCE);
    let mut fixture = Fixture::new();
    let members = [member(pawn, 1.0, false)];
    assert_eq!(
        fixture.world.locate_cell(members[0].mapped.position),
        MAPPED_CELL,
        "fixture: the mapping lands in the other copy"
    );

    for (visible, draws) in [
        (vec![REAL_CELL as u32, MAPPED_CELL as u32], true),
        (vec![MAPPED_CELL as u32], true),
        (vec![REAL_CELL as u32], false),
        (vec![], false),
    ] {
        fixture.collect_with(
            &registry,
            &members,
            &VisibleCells::Culled(visible.clone()),
            |_| STYLE,
            None,
        );
        assert_eq!(
            fixture.collector.instances().len(),
            if draws { 2 } else { 0 },
            "visible cells {visible:?}"
        );
    }
}

#[test]
fn pawn_ghost_group_carries_its_weapon_attachment_at_the_mapped_transform() {
    let mut registry = EntityRegistry::new();
    let pawn = spawn_pawn(&mut registry, SOURCE);
    let mut fixture = Fixture::new();

    fixture.collect(&registry, &[member(pawn, 0.5, false)]);

    let instances = fixture.collector.instances();
    assert_eq!(instances.len(), 2, "holder then its weapon, one group");
    let mapped = mapping().matrix();
    let holder = mapped * real_holder_transform();
    let weapon = holder * Mat4::from_translation(SOCKET);
    assert!(instances[0].transform.abs_diff_eq(holder, 1e-5));
    assert!(instances[1].transform.abs_diff_eq(weapon, 1e-5));
    assert_eq!(instances[0].model.as_str(), "models/pawn.gltf");
    assert_eq!(instances[1].model.as_str(), "models/gun.gltf");
    let seed = pawn.to_raw();
    assert_eq!(
        instances[0].palette_cache_key,
        MeshPaletteCacheKey::Ghost {
            link: 0,
            holder: seed,
            attachment_index: None
        }
    );
    assert_eq!(
        instances[1].palette_cache_key,
        MeshPaletteCacheKey::Ghost {
            link: 0,
            holder: seed,
            attachment_index: Some(0)
        }
    );
    let expected = MeshPresentation::Ghost(GhostInstanceLook {
        opacity: STYLE.opacity * 0.5,
        saturation: STYLE.saturation,
        tint: STYLE.tint,
    });
    for instance in instances {
        assert_eq!(
            instance.presentation, expected,
            "one look for the whole group"
        );
        assert!(instance.forward_visible);
        assert!(!instance.dynamic_shadow_visible, "a ghost casts no shadow");
    }
}

#[test]
fn fading_ghost_re_emits_its_cached_group_after_despawn_and_never_its_live_pose() {
    let mut registry = EntityRegistry::new();
    let pawn = spawn_pawn(&mut registry, SOURCE);
    let mut fixture = Fixture::new();
    fixture.collect(&registry, &[member(pawn, 1.0, false)]);
    let live: Vec<Mat4> = fixture
        .collector
        .instances()
        .iter()
        .map(|instance| instance.transform)
        .collect();

    // The entity walks out of the volume: a fading ghost holds its last
    // in-volume group even though the entity still exists elsewhere.
    registry
        .set_component(
            pawn,
            Transform {
                position: Vec3::new(50.0, 0.0, 0.0),
                ..Default::default()
            },
        )
        .unwrap();
    let mut fading = member(pawn, 0.6, true);
    fading.source.position = Vec3::new(50.0, 0.0, 0.0);
    fixture.collect(&registry, &[fading]);
    let held: Vec<Mat4> = fixture
        .collector
        .instances()
        .iter()
        .map(|instance| instance.transform)
        .collect();
    assert_eq!(held, live);

    // Then it despawns mid-fade (P20): the cached group still draws, posed as
    // last sampled, at the fading opacity.
    registry.despawn(pawn).unwrap();
    fixture.collect(&registry, &[member(pawn, 0.3, true)]);
    let instances = fixture.collector.instances();
    assert_eq!(instances.len(), 2);
    assert_eq!(
        instances.iter().map(|i| i.transform).collect::<Vec<_>>(),
        live
    );
    assert!(instances.iter().all(|i| !i.resample && i.capture.is_none()));
    let (opacity, _) = fixture.presented();
    assert!((opacity - STYLE.opacity * 0.3).abs() < 1e-6);

    // Faded out: the ghost leaves the member set and its group is dropped, so a
    // stale frozen key could never draw it again.
    fixture.collect(&registry, &[]);
    assert!(fixture.collector.instances().is_empty());
    fixture.collect(&registry, &[member(pawn, 0.3, true)]);
    assert!(fixture.collector.instances().is_empty());
}

/// 0 ↔ 1 at `hz`, starting low.
fn square(hz: f32, t: f32) -> f32 {
    if (t * hz).fract() >= 0.5 { 1.0 } else { 0.0 }
}

/// Presented membership behind the ramp: linear at the limiter's rate cap.
fn ramp(presented: f32, target: f32, dt: f32) -> f32 {
    let step = INTENSITY_RATE_PER_SECOND * dt.min(HITCH_CEILING_SECONDS);
    presented + (target - presented).clamp(-step, step)
}

fn limiter(dt: f32, enabled: bool) -> LimiterFrameUniform {
    pack_limiter_frame(
        LimiterFrameInput {
            elapsed_seconds: dt,
        },
        enabled,
        false,
    )
}

/// Present a 10 Hz arm/disarm strobe of a style at opacity 1 for two seconds
/// at 60 Hz through the collector; return each frame's presented opacity and
/// the membership-times-opacity it was handed.
fn strobe(limiter_frame: Option<LimiterFrameUniform>) -> Vec<(f32, f32)> {
    let mut registry = EntityRegistry::new();
    let pawn = spawn_pawn(&mut registry, SOURCE);
    let mut fixture = Fixture::new();
    let opaque = GhostLook {
        opacity: 1.0,
        ..STYLE
    };
    let dt = 1.0 / 60.0;
    let mut membership = 0.0;
    (0..120)
        .map(|n| {
            membership = ramp(membership, square(10.0, n as f32 * dt), dt);
            fixture.collect_with(
                &registry,
                &[member(pawn, membership, false)],
                &VisibleCells::DrawAll,
                |_| opaque,
                limiter_frame,
            );
            (fixture.presented().0, membership)
        })
        .collect()
}

/// Counted transitions in the worst one-second window, WCAG-style from the
/// last extremum: the judge `render-cpu`'s clamp tests use, restated for a
/// series that reaches the screen through the ghost collector.
fn worst_transitions(series: &[f32], dt: f32) -> usize {
    let mut times = Vec::new();
    let (mut ext, mut peak, mut dir, mut counted) = (series[0], series[0], 0.0f32, false);
    for (n, &v) in series.iter().enumerate().skip(1) {
        let beyond = (v - peak) * dir;
        if dir == 0.0 {
            if v != peak {
                dir = (v - peak).signum();
                peak = v;
            }
        } else if beyond > 0.0 {
            peak = v;
        } else if -beyond >= 0.1 {
            ext = peak;
            peak = v;
            dir = -dir;
            counted = false;
        }
        if !counted && (v - ext).abs() >= 0.1 && v.min(ext) < 0.8 {
            counted = true;
            times.push(n as f32 * dt);
        }
    }
    (0..times.len())
        .map(|i| {
            times[i..]
                .iter()
                .take_while(|&&t| t < times[i] + 1.0)
                .count()
        })
        .max()
        .unwrap_or(0)
}

#[test]
fn ghost_pack_routes_presented_opacity_through_the_keyed_clamp() {
    let dt = 1.0 / 60.0;
    // Limiter on: a 10 Hz arm/disarm strobe presents at most 3 flashes (6
    // transitions) in any second, and the clamp actually held some frames.
    let limited = strobe(Some(limiter(dt, true)));
    let presented: Vec<f32> = limited.iter().map(|&(presented, _)| presented).collect();
    assert!(worst_transitions(&presented, dt) <= 6, "{presented:?}");
    assert!(
        limited
            .iter()
            .any(|&(presented, handed)| presented < handed - 0.05)
    );

    // Limiter off by the player, and capture (no limiter): unclamped, the
    // membership ramp still applies, and the strobe exceeds the budget.
    for unlimited in [strobe(Some(limiter(dt, false))), strobe(None)] {
        for &(presented, handed) in &unlimited {
            assert_eq!(presented, handed);
        }
        let presented: Vec<f32> = unlimited.iter().map(|&(presented, _)| presented).collect();
        assert!(worst_transitions(&presented, dt) > 6);
    }
}

#[test]
fn red_tint_strobe_desaturates_the_drawn_tint_over_budget_only_with_the_limiter() {
    let mut registry = EntityRegistry::new();
    let pawn = spawn_pawn(&mut registry, SOURCE);
    let dt = 1.0 / 60.0;
    let red_or_white = |n: usize| -> GhostLook {
        GhostLook {
            tint: if square(10.0, n as f32 * dt) > 0.5 {
                [1.0, 0.0, 0.0]
            } else {
                [1.0, 1.0, 1.0]
            },
            opacity: 1.0,
            saturation: 1.0,
        }
    };
    for enabled in [true, false] {
        let mut fixture = Fixture::new();
        let mut last_red_tint = None;
        for n in 0..120 {
            fixture.collect_with(
                &registry,
                &[member(pawn, 1.0, false)],
                &VisibleCells::DrawAll,
                |_| red_or_white(n),
                Some(limiter(dt, enabled)),
            );
            if red_or_white(n).tint == [1.0, 0.0, 0.0] {
                last_red_tint = Some(fixture.presented().1);
            }
        }
        let tint = last_red_tint.expect("the strobe presents red frames");
        if enabled {
            let share = tint[0] / (tint[0] + tint[1] + tint[2]);
            assert!(
                share < 0.8,
                "over budget the red tint desaturates: {tint:?}"
            );
        } else {
            assert_eq!(tint, [1.0, 0.0, 0.0], "limiter off: the tint passes");
        }
    }
}

#[test]
fn ghost_limiter_frame_reads_the_fail_safe_flag_and_the_ramp_frame_time() {
    let mut slots = SlotTable::new();
    let on = ghost_limiter_frame(&slots, 0.02);
    assert!(on.enabled, "the default leaves the limiter on");
    slots
        .get_mut(FLASH_LIMITER_SLOT)
        .expect("the engine declares the limiter slot")
        .write_value(None);
    assert!(
        ghost_limiter_frame(&slots, 0.02).enabled,
        "an absent value leaves the limiter on"
    );
    assert_eq!(on.dt_window, 0.02);
    assert!(!on.init, "the keyed clamp starts fresh on its own");

    slots
        .get_mut(FLASH_LIMITER_SLOT)
        .unwrap()
        .write_value(Some(SlotValue::Boolean(false)));
    let off = ghost_limiter_frame(&slots, 0.5);
    assert!(!off.enabled, "only an explicit false turns it off");
    assert_eq!(off.dt_window, 0.5);
    assert_eq!(
        off.dt_rate, HITCH_CEILING_SECONDS,
        "a hitch clamps the rate step"
    );
}

#[test]
fn zero_opacity_ghost_draws_nothing_and_meshless_members_are_skipped() {
    let mut registry = EntityRegistry::new();
    let pawn = spawn_pawn(&mut registry, SOURCE);
    let meshless = registry.spawn(Transform::default());
    let mut fixture = Fixture::new();

    fixture.collect(
        &registry,
        &[member(pawn, 0.0, false), member(meshless, 1.0, false)],
    );

    assert!(fixture.collector.instances().is_empty());
}

// A model-bodied projectile ghosts through this collector like any meshed
// member: its mesh at the mapped transform, presented as a ghost. A sprite
// body ghosts through the billboard ghost collector instead.
#[test]
fn a_model_bodied_projectile_ghosts_through_the_skinned_ghost_path() {
    let mut registry = EntityRegistry::new();
    let shot = registry.spawn(Transform {
        position: SOURCE,
        ..Default::default()
    });
    registry
        .set_component(shot, MeshComponent::stateless("models/rocket.glb".into()))
        .unwrap();
    let mut fixture = Fixture::new();
    let ghost = GhostMember {
        category: GhostCategory::Projectiles,
        ..member(shot, 1.0, false)
    };
    fixture.collect(&registry, &[ghost]);
    let instances = fixture.collector.instances();
    assert_eq!(instances.len(), 1);
    assert!(matches!(
        instances[0].presentation,
        MeshPresentation::Ghost(_)
    ));
    assert!(
        instances[0]
            .transform
            .abs_diff_eq(mapping().matrix() * real_holder_transform(), 1e-4)
    );
}

// AC 24a wiring: a ghost's tint strobing white and black at 10 Hz at steady
// opacity is limited through its tint-luminance channel and rests dark over
// budget; with the limiter off it passes through.
#[test]
fn a_white_black_tint_strobe_is_limited_through_the_luminance_channel() {
    let mut registry = EntityRegistry::new();
    let pawn = spawn_pawn(&mut registry, SOURCE);
    let dt = 1.0 / 60.0;
    let tint_at = |n: usize| -> [f32; 3] { [square(10.0, n as f32 * dt); 3] };
    for enabled in [true, false] {
        let mut fixture = Fixture::new();
        let mut luminance = Vec::new();
        for n in 0..180 {
            fixture.collect_with(
                &registry,
                &[member(pawn, 1.0, false)],
                &VisibleCells::DrawAll,
                |_| GhostLook {
                    tint: tint_at(n),
                    opacity: 1.0,
                    saturation: 1.0,
                },
                Some(limiter(dt, enabled)),
            );
            let (opacity, tint) = fixture.presented();
            assert_eq!(opacity, 1.0, "opacity steady");
            luminance.push(tint[0] * 0.2126 + tint[1] * 0.7152 + tint[2] * 0.0722);
        }
        if enabled {
            assert!(worst_transitions(&luminance, dt) <= 6, "{luminance:?}");
            let tail_white = (120..180).filter(|&n| tint_at(n)[0] == 1.0).count();
            let held = (120..180)
                .filter(|&n| tint_at(n)[0] == 1.0 && luminance[n] <= 0.05)
                .count();
            assert!(held * 2 > tail_white, "rests at its pre-flash luminance");
        } else {
            assert!(worst_transitions(&luminance, dt) > 6);
        }
    }
}
