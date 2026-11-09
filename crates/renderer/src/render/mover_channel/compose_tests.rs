// GPU proofs of the mover channel's direct compose: the real whole-load
// passes, fed a channel table and mover t′, write each gated light's term as
// the lerp of its terminus states, in both direct atlases.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)
//
// The harness self-skips without an adapter and says so; set
// `POSTRETRO_REQUIRE_GPU` to make a missing adapter fail the test.
//
// Fixture (`direct_sh_promotion_split_test`): one L0 brick, probe `p` at
// stored slot `p`. At probe 0's texel (0, 0) the base reads 0.25 red, id 41's
// selected light (selection 0) contributes T = 0.25 there, and id 45's
// animated light 0.125 × base colour red 2.0.

use postretro_level_format::mover_probe_channel::{MoverLightNamespace, MoverProbeChannelSection};
use postretro_render_cpu::frame_uniforms::{LightTermMask, UNIFORM_SIZE};
use wgpu::util::DeviceExt;

use super::tests::{FixtureRecord, fixture_section, record};
use super::{MoverChannelInputs, MoverChannelResources, TermFamily, WholeTermRows};
use crate::render::MoverLightState;
use crate::render::animated_direct_sh_compose::AnimatedDirectShDebugOverride;
use crate::render::direct_sh_compose::{
    DirectShComposeDebugOverrides, DirectShComposeFrameInputs, DirectShComposePipelines,
    DirectShComposeResources, DirectShComposeTimestampWrites, DirectShDebugOverride,
};
use crate::render::direct_sh_promotion_split_test::{
    ATLAS, TILE, animated_delta, animation_descriptor, base_section, copy_out,
    probe_indirection_words, read, red, static_delta,
};
use crate::render::direct_sh_resources::{DirectAtlasLayout, DirectShResources};
use crate::render::gpu_test_harness::{GpuCtx, gpu_or_skip};
use crate::render::pipeline_layout::uniform_bind_group_layout_entries;
use crate::render::renderer_types::{PromotedBakedLightState, PromotedShadowPoolKind};
use crate::render::sh_residency::ShAllocationLedger;
use crate::render::sh_volume::AnimatedLightBuffers;
use crate::render::uploads::UploadQueue;

/// The AlphaLights index of id 41's selected light (selection slot 0).
const SELECTED_LIGHT: u32 = 5;
const TOLERANCE: f32 = 1.0e-3;
/// f16 0.25: id 41's tile, copied as an own tile where a record carries one.
const F16_QUARTER: u16 = 0x3400;

struct Scene {
    direct: DirectShResources,
    compose: DirectShComposeResources,
    channel: MoverChannelResources,
    weights: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
}

fn build_scene(ctx: &GpuCtx, section: Option<&MoverProbeChannelSection>, animated: bool) -> Scene {
    let device = &ctx.device;
    let base = base_section();
    let delta = static_delta();
    let animated_section = animated.then(animated_delta);
    let mut ledger = ShAllocationLedger::new();
    let direct = DirectShResources::new(
        device,
        &ctx.queue,
        Some(&base),
        Some(&delta),
        animated_section.as_ref(),
        Some(DirectAtlasLayout::from_direct_section(&base)),
        true,
        &mut ledger,
    );
    let static_family = TermFamily {
        name: "id 41",
        valid_probe_masks: &delta.valid_probe_masks,
        cell_levels: &delta.cell_levels,
        affinity_offsets: &delta.affinity_offsets,
        affinity_lights: &delta.affinity_lights,
    };
    let animated_family = animated_section.as_ref().map(|section| TermFamily {
        name: "id 45",
        valid_probe_masks: &section.valid_probe_masks,
        cell_levels: &section.cell_levels,
        affinity_offsets: &section.affinity_offsets,
        affinity_lights: &section.affinity_lights,
    });
    let static_rows = WholeTermRows::new(static_family, &delta.delta_subblocks);
    let animated_rows = animated_section
        .as_ref()
        .zip(animated_family)
        .map(|(section, family)| WholeTermRows::new(family, &section.delta_subblocks));
    let channel = MoverChannelResources::new(
        device,
        &MoverChannelInputs {
            section,
            entity_shadow_lights: &[SELECTED_LIGHT],
            static_terms: Some(static_family),
            animated_terms: animated_family,
            animation_descriptor_indices: animated_section
                .as_ref()
                .map_or(&[][..], |section| &section.animation_descriptor_indices),
        },
        false,
        |cell, animated| {
            if animated {
                animated_rows.as_ref()?.row(cell)
            } else {
                static_rows.row(cell)
            }
        },
        &mut ledger,
    )
    .expect("the fixture channel lays out");
    let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("mover channel uniforms layout"),
        entries: &uniform_bind_group_layout_entries(),
    });
    let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("mover channel uniforms"),
        contents: &[0u8; UNIFORM_SIZE],
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("mover channel uniforms"),
        layout: &uniform_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buffer.as_entire_binding(),
        }],
    });
    let weights = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("mover channel selection weights"),
        contents: &0.0_f32.to_ne_bytes(),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    let animation = AnimatedLightBuffers::for_test(device, animation_descriptor());
    let compose = DirectShComposeResources::new(
        device,
        &direct,
        &animation,
        &probe_indirection_words(),
        Some(&delta),
        animated_section.as_ref(),
        &weights,
        channel.buffer(),
        &DirectShComposePipelines::new(device, &uniform_layout),
        true,
        &mut ledger,
    );
    Scene {
        direct,
        compose,
        channel,
        weights,
        uniform_bind_group,
        uniform_buffer,
    }
}

/// One frame's inputs: mover states, the static selection weight, and the
/// animated light's promotion weight.
#[derive(Clone, Copy)]
struct Frame<'a> {
    states: &'a [MoverLightState],
    static_weight: f32,
    animated_weight: f32,
}

fn frame(states: &[MoverLightState]) -> Frame<'_> {
    Frame {
        states,
        static_weight: 0.0,
        animated_weight: 0.0,
    }
}

fn state(mover_id: u32, t: f32) -> MoverLightState {
    MoverLightState { mover_id, t }
}

/// `(promotion-free, handoff)` atlas bytes after one compose of `frame`.
fn compose(ctx: &GpuCtx, scene: &mut Scene, frame: Frame<'_>) -> (Vec<u8>, Vec<u8>) {
    let mask = LightTermMask::ALL;
    let mut uniforms = [0u8; UNIFORM_SIZE];
    uniforms[88..92].copy_from_slice(&mask.bits().to_ne_bytes());
    ctx.queue.write_buffer(&scene.uniform_buffer, 0, &uniforms);
    ctx.queue
        .write_buffer(&scene.weights, 0, &frame.static_weight.to_ne_bytes());
    let animated_states = [PromotedBakedLightState {
        weight: frame.animated_weight,
        pool_kind: Some(PromotedShadowPoolKind::Spot),
        ..PromotedBakedLightState::default()
    }];
    let uploads = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    scene.channel.set_states(frame.states);
    scene.channel.upload_t(&uploads);
    let mover_state_changed = scene.channel.t_changed_since_composed();
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    let encoded = scene.compose.dispatch_if_needed(
        &uploads,
        &mut encoder,
        DirectShComposeFrameInputs {
            uniform_bind_group: &scene.uniform_bind_group,
            active: true,
            light_term_mask: mask,
            debug_overrides: DirectShComposeDebugOverrides {
                promotion: DirectShDebugOverride::default(),
                animated: AnimatedDirectShDebugOverride::default(),
            },
            animated_promotion_states: &animated_states,
            mover_state_changed,
            timestamp_writes: DirectShComposeTimestampWrites {
                promotion: None,
                animated: None,
            },
        },
    );
    assert!(encoded, "an active compose encodes");
    scene.channel.commit_composed_t();
    let promotion_free = copy_out(&ctx.device, &mut encoder, scene.direct.atlas_view.texture());
    let handoff = copy_out(
        &ctx.device,
        &mut encoder,
        scene.direct.handoff_atlas_view.texture(),
    );
    uploads.submit([encoder.finish()]);
    (
        read(&ctx.device, &promotion_free),
        read(&ctx.device, &handoff),
    )
}

/// Probe 0's red at texel (0, 0).
fn probe_0(atlas: &[u8]) -> f32 {
    red(atlas, 0, 0)
}

fn assert_near(found: f32, expected: f32, what: &str) {
    assert!(
        (found - expected).abs() <= TOLERANCE,
        "{what}: read {found}, expected {expected}"
    );
}

/// Every texel outside probe 0's tile, byte for byte.
fn outside_probe_0(atlas: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for y in 0..ATLAS[1] {
        for x in 0..ATLAS[0] {
            if x < TILE && y < TILE {
                continue;
            }
            let at = ((y * ATLAS[0] + x) * 8) as usize;
            bytes.extend_from_slice(&atlas[at..at + 8]);
        }
    }
    bytes
}

/// The selected static light at probe 0, gated by `movers` (each closing it
/// fully at terminus 0 and opening it at terminus 1), its term from id 41 or
/// carried as its own tile.
fn selected_light(movers: &[u32], own_tile: bool) -> FixtureRecord {
    record(
        0,
        MoverLightNamespace::StaticLight,
        SELECTED_LIGHT,
        own_tile.then_some(F16_QUARTER),
        0b1,
        movers.iter().map(|&mover| (mover, 0, 255)).collect(),
    )
}

fn gpu(test: &str) -> Option<GpuCtx> {
    gpu_or_skip(test)
}

/// C1: gated only at terminus 0, the light composes S₀ at t′ = 0, ungated at
/// t′ = 1 and their mix at 0.5; a second light the mover does not gate there
/// (a bake-only light under another mover, held open) never moves, and no
/// texel outside the gated probe moves.
#[test]
fn a_single_terminus_gate_composes_s0_ungated_and_their_mix() {
    let Some(ctx) = gpu("a_single_terminus_gate_composes_s0_ungated_and_their_mix") else {
        return;
    };
    let section = fixture_section(
        [1, 1, 1],
        &[
            selected_light(&[7], false),
            record(
                0,
                MoverLightNamespace::BakeOnlyStaticLight,
                0,
                Some(0x3000),
                0b1,
                vec![(9, 0, 255)],
            ),
        ],
    );
    let mut scene = build_scene(&ctx, Some(&section), false);
    let (ungated, _) = compose(&ctx, &mut scene, frame(&[]));
    for (t, expected) in [(0.0, 0.0), (1.0, 0.25), (0.5, 0.125)] {
        let (free, handoff) = compose(&ctx, &mut scene, frame(&[state(7, t), state(9, 1.0)]));
        assert_near(
            probe_0(&free),
            expected,
            &format!("promotion-free at t′ {t}"),
        );
        assert_near(
            probe_0(&handoff),
            expected,
            &format!("handoff at t′ {t}, w 0"),
        );
        assert_eq!(
            outside_probe_0(&free),
            outside_probe_0(&ungated),
            "no texel outside the gated probe moves at t′ {t}"
        );
    }
    assert_near(probe_0(&ungated), 0.25, "no state is ungated");
}

/// C2: stripping the section composes exactly what supplying no mover state
/// does, and both equal today's atlases.
#[test]
fn a_stripped_section_and_no_state_both_compose_todays_atlases() {
    let Some(ctx) = gpu("a_stripped_section_and_no_state_both_compose_todays_atlases") else {
        return;
    };
    for animated in [false, true] {
        let section = fixture_section([1, 1, 1], &[selected_light(&[7], false)]);
        let mut stripped = build_scene(&ctx, None, animated);
        let mut stateless = build_scene(&ctx, Some(&section), animated);
        for weight in [0.0, 0.5] {
            let inputs = Frame {
                states: &[],
                static_weight: weight,
                animated_weight: weight,
            };
            assert_eq!(
                compose(&ctx, &mut stripped, inputs),
                compose(&ctx, &mut stateless, inputs),
                "id 45 {animated}, weight {weight}: both atlases byte for byte"
            );
        }
    }
}

/// C4 and C12's GPU half: two movers gating one light at one probe compose
/// the product of their lerped visibilities, never brighter than either
/// alone and never negative; with one held at 0.3, the shared probe reads
/// the held mover's t′ beside the travelling one's.
#[test]
fn two_movers_on_one_light_compose_their_product() {
    let Some(ctx) = gpu("two_movers_on_one_light_compose_their_product") else {
        return;
    };
    let section = fixture_section([1, 1, 1], &[selected_light(&[7, 9], false)]);
    let mut scene = build_scene(&ctx, Some(&section), false);
    let read = |scene: &mut Scene, t7: f32, t9: f32| {
        probe_0(&compose(&ctx, scene, frame(&[state(7, t7), state(9, t9)])).0)
    };
    let pair = read(&mut scene, 0.5, 0.5);
    assert_near(pair, 0.25 * 0.25, "Q = 0.5 × 0.5");
    let alone = read(&mut scene, 0.5, 1.0);
    assert!(pair <= alone + TOLERANCE && pair >= 0.0);
    for t in [0.0, 0.25, 0.5, 1.0] {
        assert_near(read(&mut scene, 0.3, t), 0.25 * 0.3 * t, "held at 0.3");
    }
}

/// C5: a gated animated-baked light composes its curve × its gated term, the
/// handoff under its (1 − w) arm; Pass B also carries a gated static light
/// into the promotion-free atlas.
#[test]
fn a_gated_animated_light_composes_its_curve_times_its_gated_term() {
    let Some(ctx) = gpu("a_gated_animated_light_composes_its_curve_times_its_gated_term") else {
        return;
    };
    let animated_light = record(
        0,
        MoverLightNamespace::AnimatedBaked,
        0,
        None,
        0b1,
        vec![(7, 0, 255)],
    );
    let section = fixture_section([1, 1, 1], &[animated_light]);
    let mut scene = build_scene(&ctx, Some(&section), true);
    // Curve 1.0 × base colour red 2.0 × id 45's 0.125.
    let full = 0.25;
    for (t, w) in [(0.0, 0.0), (0.5, 0.0), (1.0, 0.0), (0.5, 0.5), (1.0, 0.5)] {
        let (free, handoff) = compose(
            &ctx,
            &mut scene,
            Frame {
                states: &[state(7, t)],
                static_weight: 0.0,
                animated_weight: w,
            },
        );
        assert_near(probe_0(&free), 0.25 + full * t, &format!("free at t′ {t}"));
        assert_near(
            probe_0(&handoff),
            0.25 + full * t * (1.0 - w),
            &format!("handoff at t′ {t}, w {w}"),
        );
    }

    let both = fixture_section(
        [1, 1, 1],
        &[
            selected_light(&[7], false),
            record(
                0,
                MoverLightNamespace::AnimatedBaked,
                0,
                None,
                0b1,
                vec![(7, 0, 255)],
            ),
        ],
    );
    let mut scene = build_scene(&ctx, Some(&both), true);
    for t in [0.0, 0.5, 1.0] {
        let (free, _) = compose(&ctx, &mut scene, frame(&[state(7, t)]));
        assert_near(
            probe_0(&free),
            0.25 * t + full * t,
            &format!("both at t′ {t}"),
        );
    }
}

/// C6: in the handoff atlas a promoted gated static light contributes
/// (1 − w) × its gated term, nothing at w = 1; the promotion-free atlas
/// keeps its full gated term at every w.
#[test]
fn the_handoff_crossfades_the_gated_term_and_the_promotion_free_atlas_keeps_it() {
    let Some(ctx) =
        gpu("the_handoff_crossfades_the_gated_term_and_the_promotion_free_atlas_keeps_it")
    else {
        return;
    };
    let section = fixture_section([1, 1, 1], &[selected_light(&[7], false)]);
    let mut scene = build_scene(&ctx, Some(&section), false);
    for t in [0.0, 0.5, 1.0] {
        for w in [0.0, 0.5, 1.0] {
            let (free, handoff) = compose(
                &ctx,
                &mut scene,
                Frame {
                    states: &[state(7, t)],
                    static_weight: w,
                    animated_weight: 0.0,
                },
            );
            // Base without the light reads 0; its gated term is 0.25 t′.
            assert_near(probe_0(&free), 0.25 * t, &format!("free at t′ {t}, w {w}"));
            assert_near(
                probe_0(&handoff),
                (1.0 - w) * 0.25 * t,
                &format!("handoff at t′ {t}, w {w}"),
            );
        }
    }
}

/// C15: a gated light with no surviving id-41 entry carries its own tile and
/// composes S₀ at t′ = 0 exactly as an id-41-backed light does.
#[test]
fn an_own_tile_light_composes_as_an_id_41_backed_one() {
    let Some(ctx) = gpu("an_own_tile_light_composes_as_an_id_41_backed_one") else {
        return;
    };
    let borrowed = fixture_section([1, 1, 1], &[selected_light(&[7], false)]);
    let own = fixture_section([1, 1, 1], &[selected_light(&[7], true)]);
    let mut borrowed = build_scene(&ctx, Some(&borrowed), false);
    let mut own = build_scene(&ctx, Some(&own), false);
    for t in [0.0, 0.5] {
        let states = [state(7, t)];
        assert_eq!(
            compose(&ctx, &mut borrowed, frame(&states)),
            compose(&ctx, &mut own, frame(&states)),
            "t′ {t}: both atlases byte for byte"
        );
    }
}

/// B19's compose half: a direct-only section composes (every row above); a
/// bounce-only one lays out no direct record and composes today's atlases.
#[test]
fn a_bounce_only_section_composes_todays_direct_atlases() {
    use postretro_level_format::mover_probe_channel::{MoverBounceEntry, MoverChannelCell};
    let Some(ctx) = gpu("a_bounce_only_section_composes_todays_direct_atlases") else {
        return;
    };
    let mut bounce_only = fixture_section([1, 1, 1], &[]);
    bounce_only.cells = vec![MoverChannelCell {
        cell: 0,
        valid_probe_mask: u64::MAX,
    }];
    bounce_only.direct_offsets = vec![0, 0];
    bounce_only.bounce_offsets = vec![0, 1];
    bounce_only.bounce_entries = vec![MoverBounceEntry {
        mover_id: 7,
        affected_probe_mask: 1,
    }];
    bounce_only.bounce_tiles = vec![0; 2 * 108];
    let mut stripped = build_scene(&ctx, None, false);
    let mut scene = build_scene(&ctx, Some(&bounce_only), false);
    assert_eq!(scene.channel.layout().record_count(), 0);
    assert_eq!(
        compose(&ctx, &mut stripped, frame(&[])),
        compose(&ctx, &mut scene, frame(&[state(7, 0.0)])),
    );
}

/// C11's legacy half: a t′ change alone dispatches the whole-load compose
/// once no other input is active, and its trigger commits only with the
/// encode, so a frame that encodes nothing leaves it for the next one.
#[test]
fn the_legacy_compose_dispatches_on_a_t_change_alone_until_it_encodes() {
    let Some(ctx) = gpu("the_legacy_compose_dispatches_on_a_t_change_alone_until_it_encodes")
    else {
        return;
    };
    let section = fixture_section([1, 1, 1], &[selected_light(&[7], false)]);
    let mut scene = build_scene(&ctx, Some(&section), false);
    let uploads = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let step = |scene: &mut Scene, states: &[MoverLightState], record: bool| {
        scene.channel.set_states(states);
        scene.channel.upload_t(&uploads);
        let changed = scene.channel.t_changed_since_composed();
        if !record {
            return None;
        }
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        let encoded = scene.compose.dispatch_if_needed(
            &uploads,
            &mut encoder,
            DirectShComposeFrameInputs {
                uniform_bind_group: &scene.uniform_bind_group,
                active: false,
                light_term_mask: LightTermMask::ALL,
                debug_overrides: DirectShComposeDebugOverrides {
                    promotion: DirectShDebugOverride::default(),
                    animated: AnimatedDirectShDebugOverride::default(),
                },
                animated_promotion_states: &[],
                mover_state_changed: changed,
                timestamp_writes: DirectShComposeTimestampWrites {
                    promotion: None,
                    animated: None,
                },
            },
        );
        if encoded {
            scene.channel.commit_composed_t();
        }
        uploads.submit([encoder.finish()]);
        Some(encoded)
    };
    // The load's copy-through, then a settled frame.
    assert_eq!(step(&mut scene, &[], true), Some(true));
    assert_eq!(step(&mut scene, &[], true), Some(false));
    // t′ moves on a frame that records nothing, then holds.
    assert_eq!(step(&mut scene, &[state(7, 0.5)], false), None);
    assert_eq!(step(&mut scene, &[state(7, 0.5)], true), Some(true));
    assert_eq!(step(&mut scene, &[state(7, 0.5)], true), Some(false));
}

/// B13: the channel table is priced in the SH allocation ledger while its
/// level is installed, and leaves with it.
#[test]
fn the_channel_table_is_ledgered_while_installed_and_gone_after_unload() {
    use crate::render::fluid_baked_frame_test_fixtures::sh_section;
    use crate::render::gpu_test_harness::offscreen_renderer_or_skip;
    use crate::render::{LevelGeometry, LevelGeometryShStorage};
    let Some(mut renderer) = offscreen_renderer_or_skip(
        "the_channel_table_is_ledgered_while_installed_and_gone_after_unload",
        16,
        16,
    ) else {
        return;
    };
    let sh = sh_section([0.0; 3], [1.0; 3], [4, 4, 4], |_| glam::Vec3::splat(0.5));
    let section = fixture_section(
        [1, 1, 1],
        &[record(
            0,
            MoverLightNamespace::BakeOnlyStaticLight,
            0,
            Some(F16_QUARTER),
            0b11,
            vec![(7, 0, 255)],
        )],
    );
    let expected = super::layout::MoverChannelLayout::build(&MoverChannelInputs {
        section: Some(&section),
        entity_shadow_lights: &[],
        static_terms: None,
        animated_terms: None,
        animation_descriptor_indices: &[],
    })
    .unwrap()
    .initial_words()
    .len() as u64
        * 4;
    let bvh = postretro_render_data::geometry::BvhTree {
        nodes: Vec::new(),
        leaves: Vec::new(),
        root_node_index: 0,
    };
    let table_bytes = |renderer: &crate::render::Renderer| {
        renderer.sh_residency_report().map_or(0, |report| {
            report
                .allocations
                .iter()
                .filter(|allocation| allocation.name == "mover_channel_table")
                .map(|allocation| allocation.bytes)
                .sum::<u64>()
        })
    };
    renderer
        .install_level_geometry(
            &LevelGeometry {
                vertices: &[],
                vertex_spec_owners: &[],
                indices: &[],
                bvh: &bvh,
                lights: &[],
                light_influences: &[],
                sh_volume: Some(&sh),
                sh_storage: LevelGeometryShStorage::Legacy,
                probe_depth_tiles: None,
                lightmap: None,
                lightmap_streaming: None,
                chunk_light_list: None,
                animated_light_chunks: None,
                animated_light_weight_maps: None,
                delta_sh_volumes: None,
                direct_sh_volume: None,
                direct_sh_delta_volumes: None,
                animated_direct_sh_delta_volumes: None,
                billboard_direct_scatter_volume: None,
                animated_billboard_direct_scatter_delta_volumes: None,
                mover_probe_channel: Some(&section),
                entity_shadow_lights: &[],
                shadowmask_atlas: None,
                sdf_atlas: None,
                lightmap_mode: postretro_level_loader::LightmapMode::Shadowed,
                cell_draw_index: None,
                kinematic_geometry: None,
                cells: &[],
                texture_materials: &[],
            },
            Default::default(),
        )
        .expect("level resources install");
    assert!(expected > 0);
    assert_eq!(table_bytes(&renderer), expected);
    renderer.release_level_resources();
    assert_eq!(table_bytes(&renderer), 0, "unload frees the table");
}
