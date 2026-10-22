// Membership ramp: start values, rate cap, reversal, freezing and the output list.
// See: context/lib/rendering_pipeline.md §7.9, §7.8 · context/lib/entity_model.md §7d

use glam::{Mat4, Quat, Vec3};
use postretro_entities::EntityRegistry;

use super::*;

const FRAME: f32 = 1.0 / 240.0;

fn mapping() -> SeamlessMapping {
    SeamlessMapping::new(Vec3::ZERO, 0.0, Vec3::new(100.0, 0.0, 0.0), 0.5)
}

fn pose(x: f32) -> Transform {
    Transform {
        position: Vec3::new(x, 1.0, 0.0),
        rotation: Quat::from_rotation_y(0.3),
        scale: Vec3::ONE,
    }
}

/// A table past its install frame, so new keys start at 0.
fn running_table() -> MembershipTable {
    let mut table = MembershipTable::default();
    table.begin_frame();
    table.finish_frame(0.0, |_| Some(mapping()));
    table
}

fn frame(table: &mut MembershipTable, marked: &[(GhostKey, Transform)], step: f32) {
    table.begin_frame();
    for (key, source) in marked {
        table.mark_target(*key, GhostCategory::Players, *source);
    }
    table.finish_frame(step, |_| Some(mapping()));
}

fn key(entities: &mut EntityRegistry, link: u16) -> GhostKey {
    GhostKey {
        link,
        entity: entities.spawn(Default::default()),
    }
}

fn membership(table: &MembershipTable, key: GhostKey) -> Option<f32> {
    table
        .members()
        .iter()
        .find(|member| member.link == key.link && member.entity == key.entity)
        .map(|member| member.membership)
}

#[test]
fn ramp_step_is_the_limiter_rate_cap_over_presented_time_bounded_by_the_hitch_ceiling() {
    assert!((ramp_step(0.01) - 0.04).abs() < 1e-6);
    // A one-second hitch steps only the 1/30 s ceiling.
    assert!((ramp_step(1.0) - 4.0 / 30.0).abs() < 1e-6);
    assert_eq!(ramp_step(-1.0), 0.0);
    assert_eq!(ramp_step(f32::NAN), 0.0);
    assert_eq!(ramp_step(f32::INFINITY), 0.0);
}

#[test]
fn new_ghost_presents_its_first_frame_at_zero_then_ramps_at_the_rate_cap() {
    let mut entities = EntityRegistry::new();
    let ghost = key(&mut entities, 0);
    let mut table = running_table();
    let step = ramp_step(FRAME);
    frame(&mut table, &[(ghost, pose(1.0))], step);
    assert_eq!(
        membership(&table, ghost),
        Some(0.0),
        "P21: first frame at 0"
    );
    for expected in 1..=3 {
        frame(&mut table, &[(ghost, pose(1.0))], step);
        let presented = membership(&table, ghost).unwrap();
        assert!((presented - expected as f32 * step).abs() < 1e-6);
    }
    for _ in 0..200 {
        frame(&mut table, &[(ghost, pose(1.0))], step);
    }
    assert_eq!(
        membership(&table, ghost),
        Some(1.0),
        "the ramp caps at full"
    );
}

#[test]
fn install_frame_starts_every_ghost_at_its_target() {
    let mut entities = EntityRegistry::new();
    let ghost = key(&mut entities, 0);
    let mut table = MembershipTable::default();
    frame(&mut table, &[(ghost, pose(1.0))], ramp_step(FRAME));
    assert_eq!(membership(&table, ghost), Some(1.0), "P26: full on frame 1");
    let later = key(&mut entities, 0);
    frame(&mut table, &[(ghost, pose(1.0)), (later, pose(2.0))], 0.0);
    assert_eq!(
        membership(&table, later),
        Some(0.0),
        "only the first evaluation settles"
    );
}

#[test]
fn reversal_mid_ramp_moves_from_the_presented_value() {
    let mut entities = EntityRegistry::new();
    let ghost = key(&mut entities, 0);
    let mut table = running_table();
    let step = 0.1;
    frame(&mut table, &[(ghost, pose(1.0))], step);
    for _ in 0..5 {
        frame(&mut table, &[(ghost, pose(1.0))], step);
    }
    let peak = membership(&table, ghost).unwrap();
    assert!((peak - 0.5).abs() < 1e-6);
    frame(&mut table, &[], step);
    frame(&mut table, &[], step);
    let faded = membership(&table, ghost).unwrap();
    assert!((faded - 0.3).abs() < 1e-6, "P4: the fade starts at 0.5");
    frame(&mut table, &[(ghost, pose(1.0))], step);
    assert!(
        (membership(&table, ghost).unwrap() - 0.4).abs() < 1e-6,
        "P4: re-entry mid-fade ramps up from 0.3, never resets"
    );
}

#[test]
fn ghost_whose_target_ends_freezes_at_its_last_in_volume_pose_and_drops_at_zero() {
    let mut entities = EntityRegistry::new();
    let ghost = key(&mut entities, 3);
    let mut table = running_table();
    let step = 0.25;
    frame(&mut table, &[(ghost, pose(1.0))], step);
    frame(&mut table, &[(ghost, pose(2.0))], step);
    frame(&mut table, &[(ghost, pose(3.0))], step);
    let live = table.members()[0];
    assert!(!live.frozen);
    assert_eq!(live.source, pose(3.0));
    // The member left the volume: it is never marked again.
    frame(&mut table, &[], step);
    let frozen = table.members()[0];
    assert!(frozen.frozen);
    assert_eq!(frozen.source, pose(3.0), "P1: the last in-volume pose");
    assert!((frozen.mapped.position - mapping().point(pose(3.0).position)).length() < 1e-5);
    frame(&mut table, &[], step);
    assert!(
        table.members().is_empty(),
        "dropped once membership reaches 0"
    );
    frame(&mut table, &[(ghost, pose(4.0))], step);
    assert_eq!(
        membership(&table, ghost),
        Some(0.0),
        "a returning member after the drop is a new ghost"
    );
}

#[test]
fn output_is_ordered_by_link_and_maps_the_source_pose_like_the_mapping_matrix() {
    let mut entities = EntityRegistry::new();
    let a = key(&mut entities, 2);
    let b = GhostKey {
        link: 0,
        entity: a.entity,
    };
    let c = key(&mut entities, 0);
    let mut table = MembershipTable::default();
    frame(
        &mut table,
        &[(a, pose(1.0)), (c, pose(2.0)), (b, pose(1.0))],
        0.0,
    );
    let order: Vec<_> = table
        .members()
        .iter()
        .map(|member| (member.link, member.entity))
        .collect();
    assert_eq!(order, vec![(0, a.entity), (0, c.entity), (2, a.entity)]);
    for member in table.members() {
        let model = Mat4::from_scale_rotation_translation(
            member.source.scale,
            member.source.rotation,
            member.source.position,
        );
        let mapped = Mat4::from_scale_rotation_translation(
            member.mapped.scale,
            member.mapped.rotation,
            member.mapped.position,
        );
        assert!(
            (member.mapping.matrix() * model).abs_diff_eq(mapped, 1e-4),
            "a draw composing the mapping onto the source model matches the mapped pose"
        );
    }
}
