// GPU readback of forward.wgsl's promoted-light union subtraction through
// `shadowmask_union_harness`.
// See: context/lib/rendering_pipeline.md §4 (Static specular ownership)

use super::gpu_test_harness::{GpuCtx, bc_gpu_or_skip, gpu_or_skip};
use super::shadowmask_union_fixtures::{Case, E, PRE_CHANGE_DROPPED, influence_bytes, promoted};
use super::shadowmask_union_harness::{MaskPool, render, render_sampled};

use crate::lighting::lightmap::test_fixtures::{FixtureTexels, block_fixture};
use crate::lighting::lightmap::{StaticPool, plan_static_pool, upload_static_pool};

fn assert_close(actual: [f32; 4], expected: [f32; 4], what: &str) {
    for c in 0..4 {
        assert!(
            (actual[c] - expected[c]).abs() <= 1.0e-6 + expected[c].abs() * 1.0e-5,
            "{what}: lane {c} is {actual:?}, expected {expected:?}"
        );
    }
}

/// A mask with one baked-dark channel, so the production skip and mode 6's
/// diagnostic both run.
const MASK: [f32; 4] = [0.9, 0.55, 0.0, 0.7];

// On a face every promoted light owns, the ownership union reproduces the
// pre-change union exactly, with each light's owner position equal to the
// map-wide slot it held before. Owners ascend (0, 1, 3) while records arrive
// as 3, 1, 0, and spec light 3's position 2 holds a baked-dark channel.
#[test]
fn union_matches_the_pre_change_union_where_every_promoted_light_owns_the_face() {
    let Some(ctx) =
        gpu_or_skip("union_matches_the_pre_change_union_where_every_promoted_light_owns_the_face")
    else {
        return;
    };
    let promoted = promoted();
    // Owners [0, 1, 3]: spec light 3 at position 2, 1 at 1, 0 at 0.
    let influence = influence_bytes(&promoted, &[2.0, 1.0, 0.0]);
    for (sdf_shadow_mode, mask) in [(0, MASK), (6, MASK), (0, [0.9, 0.55, 0.35, 0.7])] {
        let case = Case {
            promoted_count: promoted.len() as u32,
            sdf_shadow_mode,
            mask_present: true,
            mask,
            owners: [0, 1, 3, E],
            block: 0,
        };
        let out = render(&ctx, case, &influence);
        let mut subtracted = false;
        for (column, (current, pre_change)) in out.iter().enumerate() {
            assert!(pre_change[3] >= 0.0, "column {column} rendered");
            assert_close(
                *current,
                *pre_change,
                &format!("mode {sdf_shadow_mode} mask {mask:?} column {column}"),
            );
            subtracted |= current[..3].iter().any(|&c| c > 1.0e-4);
        }
        assert!(
            subtracted,
            "mode {sdf_shadow_mode}: the fixture must subtract"
        );
    }
}

// A promoted light that does not own the face subtracts nothing there.
// The current union equals the pre-change union with that light unslotted.
#[test]
fn union_subtracts_nothing_for_a_promoted_light_that_does_not_own_the_face() {
    let Some(ctx) =
        gpu_or_skip("union_subtracts_nothing_for_a_promoted_light_that_does_not_own_the_face")
    else {
        return;
    };
    let promoted = promoted();
    let base = Case {
        promoted_count: promoted.len() as u32,
        sdf_shadow_mode: 0,
        mask_present: true,
        mask: [0.9, 0.55, 0.35, 0.7],
        owners: [E; 4],
        block: 0,
    };

    // Only spec light 1 owns this face, at position 0.
    let only_light_1 = render(
        &ctx,
        Case {
            owners: [1, E, E, E],
            ..base
        },
        &influence_bytes(&promoted, &[PRE_CHANGE_DROPPED, 0.0, PRE_CHANGE_DROPPED]),
    );
    // The same lights, all slotted as before ownership: lights 3 and 0 would
    // have subtracted here.
    let all_slotted = render(
        &ctx,
        Case {
            owners: [1, E, E, E],
            ..base
        },
        &influence_bytes(&promoted, &[2.0, 0.0, 1.0]),
    );
    for (column, (current, pre_change)) in only_light_1.iter().enumerate() {
        assert_close(
            *current,
            *pre_change,
            &format!("column {column}: only the owner subtracts"),
        );
        assert!(
            all_slotted[column].1[..3]
                .iter()
                .zip(&current[..3])
                .any(|(slotted, owned)| slotted > &(owned + 1.0e-4)),
            "column {column}: the non-owners must have had something to subtract"
        );
    }

    // A face no promoted light owns subtracts nothing at all.
    let unowned = render(&ctx, base, &influence_bytes(&promoted, &[2.0, 0.0, 1.0]));
    for (column, (current, pre_change)) in unowned.iter().enumerate() {
        assert_eq!(current[..3], [0.0; 3], "column {column}");
        assert_eq!(current[3], 1.0, "column {column}: no light covers it");
        assert!(
            pre_change[..3].iter().any(|&c| c > 1.0e-4),
            "column {column}"
        );
    }
}

// With the mask-present bit clear — no id 42, a refused pool, or placeholder
// mode — no promoted light subtracts, even where it owns the face.
#[test]
fn union_subtracts_nothing_without_a_bound_mask() {
    let Some(ctx) = gpu_or_skip("union_subtracts_nothing_without_a_bound_mask") else {
        return;
    };
    let promoted = promoted();
    let out = render(
        &ctx,
        Case {
            promoted_count: promoted.len() as u32,
            sdf_shadow_mode: 0,
            mask_present: false,
            mask: [1.0; 4],
            owners: [0, 1, 3, E],
            block: 0,
        },
        &influence_bytes(&promoted, &[2.0, 1.0, 0.0]),
    );
    for (column, (current, _)) in out.iter().enumerate() {
        assert_eq!(current[..3], [0.0; 3], "column {column}");
        assert_eq!(current[3], 1.0, "column {column}");
    }
}

/// Six blocks through the real pool plan: each odd block stores no blob for the
/// all-zero plane its even neighbour stores. Plane A nonzero on blocks 0 and 1,
/// plane B on 2 and 3, neither on 4 and 5.
fn absent_plane_pool(ctx: &GpuCtx) -> (wgpu::Texture, Vec<u8>) {
    const EDGE: u32 = 8;
    let extents = [(EDGE, EDGE); 6];
    let nonzero_plane = |block: usize| [Some(0u32), Some(1), None][block / 2];
    let mask = |block: usize, channel: u32, _: u32, _: u32| match nonzero_plane(block) {
        Some(plane) if plane == channel / 2 => 120 + channel as u8 * 30,
        _ => 0,
    };
    let presence: Vec<[bool; 2]> = (0..extents.len())
        .map(|block| [0, 1].map(|plane| block % 2 == 0 || nonzero_plane(block) == Some(plane)))
        .collect();
    let fixture = block_fixture(
        &extents,
        2,
        &FixtureTexels {
            irradiance: &|_, _, _| [0.0; 4],
            direction: &|_, _, _| [128, 255],
            shadowmask: Some(&mask),
        },
    )
    .with_planes(&presence);
    let StaticPool::Blocks(plan) = plan_static_pool(
        Some(&fixture.index),
        fixture.shadowmask.as_ref(),
        &fixture.payloads,
        8192,
        256,
    ) else {
        panic!("the pool plans");
    };
    let textures = upload_static_pool(&ctx.device, &ctx.queue, &plan, &fixture.payloads);
    (
        textures.shadowmask.expect("planes need the mask pool"),
        plan.table_bytes(),
    )
}

// An absent plane drives the union subtraction and owner specular exactly as the
// all-zero plane it replaces, through the real sampler: on an unowned face, an
// owned face, and owners on channels 0 and 1 with plane B absent, where plane A
// still subtracts and shines.
#[test]
fn absent_plane_samples_as_stored_zero() {
    let Some(ctx) = bc_gpu_or_skip("absent_plane_samples_as_stored_zero") else {
        return;
    };
    let (texture, table) = absent_plane_pool(&ctx);
    let pool = MaskPool {
        texture: &texture,
        table: &table,
    };
    let promoted = promoted();
    let influence = influence_bytes(&promoted, &[]);
    let render_block = |owners: [u16; 4], block: u32| {
        render_sampled(
            &ctx,
            Case {
                promoted_count: promoted.len() as u32,
                sdf_shadow_mode: 0,
                mask_present: true,
                mask: [0.0; 4],
                owners,
                block: block + 1,
            },
            &influence,
            &pool,
        )
    };
    let lit = |columns: &[([f32; 4], [f32; 4])]| {
        (
            columns
                .iter()
                .any(|(union, _)| union[..3].iter().any(|&c| c > 1.0e-4)),
            columns
                .iter()
                .any(|(_, specular)| specular[..3].iter().any(|&c| c > 1.0e-4)),
        )
    };

    for (what, owners, stored, absent, expect_lit) in [
        ("unowned, plane B absent", [E; 4], 0, 1, false),
        ("unowned, plane A absent", [E; 4], 2, 3, false),
        ("owned, both planes absent", [0, 1, 3, E], 4, 5, false),
        (
            "owners on 0 and 1, plane B absent",
            [0, 1, E, E],
            0,
            1,
            true,
        ),
        (
            "owners on 0, 1 and 2, plane B absent",
            [0, 1, 3, E],
            0,
            1,
            true,
        ),
        ("owner on 0, plane A absent", [0, E, E, E], 2, 3, false),
        (
            "owners on 2 and 3 of plane B, plane A absent",
            [0, 1, 2, 3],
            2,
            3,
            true,
        ),
    ] {
        let with_blob = render_block(owners, stored);
        let without = render_block(owners, absent);
        assert_eq!(with_blob, without, "{what}");
        assert_eq!(
            lit(&without),
            (expect_lit, expect_lit),
            "{what}: the fixture must show what it claims"
        );
    }
}
