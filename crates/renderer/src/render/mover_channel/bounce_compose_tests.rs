// GPU proofs of the mover channel's indirect compose: the real whole-load SH
// compose, fed a bounce table and mover t′, writes the lerp of a mover's two
// absolute static bounce states in place of id 34 at the probes it affects,
// several movers adding their lerped differences, the sum clamped per texel at
// zero before id-27 deltas add. A streamed bounce block composes the same
// bytes from its pooled cells, and nothing while a cell is not resident.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)
//
// The harness self-skips without a BC-capable adapter (id 34's whole-load
// path allocates its BC6H dummies) and says so; set `POSTRETRO_REQUIRE_GPU`
// to make a missing adapter fail the test.
//
// Fixture: one L0 brick, probe `p` at stored slot `p` (tile origin
// `(6·(p % 8), 6·(p / 8))`). Id 34 reads 0.25 in every channel; id 27's one
// animated light adds 0.125 × base colour (2, 1, 0.5), so +0.25 red.

use postretro_level_format::delta_sh_volumes::{
    AFFINITY_FACTOR, DEFAULT_DELTA_PROBE_F16_STRIDE, DeltaShVolumesSection, PROBES_PER_CELL,
};
use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
use postretro_level_format::mover_probe_channel::{
    BounceEntryParts, MOVER_IRRADIANCE_TILE_F16_COUNT, MoverBounceEntry, MoverChannelBlock,
    MoverLightRampRecord, MoverPayloadPlacement, MoverProbeChannelParts, MoverProbeChannelSection,
    MoverStreamedRow,
};
use postretro_level_format::sh_volume::{
    AnimationDescriptor, OCTAHEDRAL_PROBE_STRIDE, OctahedralShProbe, OctahedralShVolumeSection,
};
use postretro_render_cpu::frame_uniforms::{LightTermMask, UNIFORM_SIZE};
use wgpu::util::DeviceExt;

use super::{ClusterTermRows, MoverChannelInputs, MoverChannelResources};
use crate::render::MoverLightState;
use crate::render::direct_sh_promotion_split_test::{ATLAS, TILE, copy_out, read, red};
use crate::render::gpu_test_harness::{GpuCtx, bc_gpu_or_skip};
use crate::render::pipeline_layout::uniform_bind_group_layout_entries;
use crate::render::sh_compose::{ShComposePipelines, ShComposeResources};
use crate::render::sh_residency::ShAllocationLedger;
use crate::render::sh_volume::{ShVolumeResources, ShVolumeSections};
use crate::render::uploads::{StagedUploads, StagingPool, UploadQueue};

const GRID: [u32; 3] = [4, 4, 4];
const TILES_PER_ROW: u32 = 8;
const TOLERANCE: f32 = 1.0e-3;
const F16_ONE: u16 = 0x3C00;
const F16_QUARTER: u16 = 0x3400;
const F16_EIGHTH: u16 = 0x3000;
/// Id 34's value in every channel.
const BASE: f32 = 0.25;
/// Id 27's red contribution: 0.125 × base colour red 2.0.
const DELTA_RED: f32 = 0.25;

fn f16(value: f32) -> u16 {
    // Exact for the binary fractions these fixtures use.
    let bits = value.abs().to_bits();
    let sign = if value < 0.0 { 0x8000u16 } else { 0 };
    if value == 0.0 {
        return sign;
    }
    let exponent = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mantissa = ((bits >> 13) & 0x3ff) as u16;
    sign | ((exponent as u16) << 10) | mantissa
}

fn base_section() -> OctahedralShVolumeSection {
    let mut atlas = Vec::with_capacity((ATLAS[0] * ATLAS[1] * 8) as usize);
    for _ in 0..ATLAS[0] * ATLAS[1] {
        for half in [F16_QUARTER, F16_QUARTER, F16_QUARTER, F16_ONE] {
            atlas.extend_from_slice(&half.to_le_bytes());
        }
    }
    OctahedralShVolumeSection {
        grid_origin: [0.0; 3],
        cell_size: [1.0; 3],
        grid_dimensions: GRID,
        probe_stride: OCTAHEDRAL_PROBE_STRIDE,
        tile_dimension: TILE,
        tile_border: 1,
        atlas_dimensions: ATLAS,
        layer_count: 1,
        tiles_per_layer: TILES_PER_ROW * TILES_PER_ROW,
        atlas_tiles_per_row: TILES_PER_ROW,
        probes: vec![
            OctahedralShProbe {
                validity: 1,
                mean_distance: F16_ONE,
                mean_sq_distance: F16_ONE,
                density_level: 0,
                node_scale: 0,
            };
            (GRID[0] * GRID[1] * GRID[2]) as usize
        ],
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        compact_atlas: atlas,
        animation_descriptors: vec![AnimationDescriptor {
            period: 1.0,
            base_color: [2.0, 1.0, 0.5],
            start_active: 1,
            ..AnimationDescriptor::default()
        }],
        slot_for_map_light: Vec::new(),
    }
}

/// Id 27: one animated light (descriptor 0) over every probe.
fn indirect_delta() -> DeltaShVolumesSection {
    DeltaShVolumesSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: [1, 1, 1],
        tile_dimension: TILE,
        tile_border: 1,
        animation_descriptor_indices: vec![0],
        valid_probe_masks: vec![u64::MAX],
        cell_levels: vec![0],
        affinity_offsets: vec![0, 1],
        affinity_lights: vec![0],
        delta_subblocks: vec![F16_EIGHTH; PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE],
    }
}

/// One mover's bounce states: S^b₀ = `s0` at the probes of `masks[0]`, S^b₁
/// = `s1` at the probes of `masks[1]`, in every channel of every texel. An
/// affected probe outside one terminus's mask stores no state there and
/// composes id 34.
struct Bounce {
    mover_id: u32,
    masks: [u64; 2],
    s0: f32,
    s1: f32,
}

fn channel_section(bounces: &[Bounce], streamed: bool) -> MoverProbeChannelSection {
    let parts = MoverProbeChannelParts {
        affinity_dims: [1, 1, 1],
        direct: Vec::new(),
        bounce: bounces
            .iter()
            .map(|bounce| BounceEntryParts {
                cell: 0,
                entry: MoverBounceEntry {
                    mover_id: bounce.mover_id,
                    stored_probe_masks: bounce.masks,
                },
                // Probe after probe, x-fastest: S^b₀ where stored, then S^b₁.
                tiles: (0..64u32)
                    .flat_map(|local| {
                        [(bounce.masks[0], bounce.s0), (bounce.masks[1], bounce.s1)]
                            .into_iter()
                            .filter(move |&(mask, _)| mask & (1 << local) != 0)
                    })
                    .flat_map(|(_, value)| {
                        std::iter::repeat_n(f16(value), MOVER_IRRADIANCE_TILE_F16_COUNT)
                    })
                    .collect(),
            })
            .collect(),
        depth: Vec::new(),
    };
    let mut section = MoverProbeChannelSection::assemble(
        parts,
        |_| u64::MAX,
        |mover_id| {
            Ok::<_, ()>(MoverLightRampRecord {
                mover_id,
                start: 0.0,
                end: 1.0,
            })
        },
    )
    .unwrap()
    .expect("the fixture holds bounce entries");
    if streamed {
        section.bounce_placement = MoverPayloadPlacement::Streamed;
    }
    section
}

struct Scene {
    sh: ShVolumeResources,
    compose: ShComposeResources,
    uniform_bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
}

fn build_scene(ctx: &GpuCtx, section: Option<&MoverProbeChannelSection>, streamed: bool) -> Scene {
    let device = &ctx.device;
    let mut ledger = ShAllocationLedger::new();
    let base = base_section();
    let delta = indirect_delta();
    let mut channel = MoverChannelResources::new(
        device,
        &MoverChannelInputs {
            section,
            entity_shadow_lights: &[],
            static_terms: None,
            animated_terms: None,
            animation_descriptor_indices: &[],
            indirect_descriptor_indices: &delta.animation_descriptor_indices,
        },
        streamed,
        |_, _| None,
        &mut ledger,
    )
    .expect("the fixture channel lays out");
    if streamed {
        // A pool sized for the whole map: the legacy compose this fixture
        // runs never rebinds, so the table must not grow under it. Growth is
        // the streamed passes' (`rebind_mover_channel`) and `pool.rs`'s.
        channel.size_streamed_pools(device, 1.0);
    }
    let sh = ShVolumeResources::new(
        device,
        &ctx.queue,
        ShVolumeSections {
            sh: Some(&base),
            depth_tiles: None,
            stream_base_present: false,
            stream_base_probes: None,
            stream_animation_descriptors: None,
            indirect_delta_present: true,
            direct: None,
            direct_delta: None,
            animated_direct_delta: None,
            billboard_direct_scatter: None,
            animated_billboard_direct_scatter_delta: None,
        },
        1,
        false,
        channel,
        &mut ledger,
    );
    let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("mover bounce uniforms layout"),
        entries: &uniform_bind_group_layout_entries(),
    });
    let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("mover bounce uniforms"),
        contents: &[0u8; UNIFORM_SIZE],
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("mover bounce uniforms"),
        layout: &uniform_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buffer.as_entire_binding(),
        }],
    });
    let compose = ShComposeResources::new(
        device,
        &sh,
        sh.mover_channel.bounce_buffer(),
        Some(&base),
        Some(&delta),
        true,
        true,
        &ShComposePipelines::new(device, &uniform_layout),
        &mut ledger,
    );
    Scene {
        sh,
        compose,
        uniform_bind_group,
        uniform_buffer,
    }
}

fn state(mover_id: u32, t: f32) -> MoverLightState {
    MoverLightState { mover_id, t }
}

/// The total atlas after composing with `states`. The compose rebuilds the
/// whole atlas, as the legacy pass does on any mover-state change.
fn compose(ctx: &GpuCtx, scene: &mut Scene, states: &[MoverLightState]) -> Vec<u8> {
    compose_masked(ctx, scene, states, LightTermMask::ALL)
}

fn compose_masked(
    ctx: &GpuCtx,
    scene: &mut Scene,
    states: &[MoverLightState],
    mask: LightTermMask,
) -> Vec<u8> {
    let mut uniforms = [0u8; UNIFORM_SIZE];
    uniforms[88..92].copy_from_slice(&mask.bits().to_ne_bytes());
    ctx.queue.write_buffer(&scene.uniform_buffer, 0, &uniforms);
    let uploads = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    scene.sh.mover_channel.set_states(states);
    scene.sh.mover_channel.upload_t(&uploads);
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    // Force the dispatch: the trigger itself is proven by the unit tests.
    scene.compose.dispatch_if_needed(
        &mut encoder,
        &scene.uniform_bind_group,
        true,
        true,
        mask,
        None,
    );
    scene.sh.mover_channel.commit_composed_bounce_t();
    let total = copy_out(
        &ctx.device,
        &mut encoder,
        scene.sh.total_atlas_storage_view.texture(),
    );
    uploads.submit([encoder.finish()]);
    read(&ctx.device, &total)
}

/// Every texel of probe `p`'s tile reads `expected` red.
fn assert_tile(atlas: &[u8], p: u32, expected: f32, what: &str) {
    let origin = (TILE * (p % TILES_PER_ROW), TILE * (p / TILES_PER_ROW));
    for y in 0..TILE {
        for x in 0..TILE {
            let found = red(atlas, origin.0 + x, origin.1 + y);
            assert!(
                (found - expected).abs() <= TOLERANCE,
                "{what}: probe {p} texel ({x}, {y}) read {found}, expected {expected}"
            );
        }
    }
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

/// C3: at the probe a mover affects, id 34 becomes the lerp of its bounce
/// states (S^b₀ at t′ = 0, S^b₁ at 1, their mix at 0.5) and id 27's delta
/// still adds on top; with no state handed over, or no section at all, the
/// atlas is today's byte for byte; no texel outside the probe moves.
#[test]
fn c3_bounce_replaces_id_34_with_the_lerp_of_its_states_and_deltas_still_add() {
    let Some(ctx) =
        bc_gpu_or_skip("c3_bounce_replaces_id_34_with_the_lerp_of_its_states_and_deltas_still_add")
    else {
        return;
    };
    let today = compose(&ctx, &mut build_scene(&ctx, None, false), &[]);
    assert_tile(&today, 0, BASE + DELTA_RED, "today, no section");

    let section = channel_section(
        &[Bounce {
            mover_id: 7,
            masks: [0b1, 0b1],
            s0: 0.5,
            s1: 0.125,
        }],
        false,
    );
    let mut scene = build_scene(&ctx, Some(&section), false);
    let stateless = compose(&ctx, &mut scene, &[]);
    assert_eq!(stateless, today, "no mover state composes today's atlas");
    for (t, expected) in [(0.0, 0.5), (1.0, 0.125), (0.5, 0.3125)] {
        let atlas = compose(&ctx, &mut scene, &[state(7, t)]);
        assert_tile(&atlas, 0, expected + DELTA_RED, &format!("t′ = {t}"));
        assert_eq!(
            outside_probe_0(&atlas),
            outside_probe_0(&today),
            "t′ = {t}: an unaffected probe composes id 34 exactly"
        );
    }
    // A state for another mover changes nothing.
    let other = compose(&ctx, &mut scene, &[state(9, 0.0)]);
    assert_eq!(other, today);
    // B11 (GPU): id 27 composes unchanged on top. Without its animated term
    // the probe reads the bounce state alone; with it, that plus the same
    // delta every probe gets, whatever the mover does.
    let mut static_only = LightTermMask::ALL;
    static_only.set_enabled(LightTermMask::INDIRECT_ANIMATED, false);
    let atlas = compose_masked(&ctx, &mut scene, &[state(7, 0.0)], static_only);
    assert_tile(&atlas, 0, 0.5, "bounce state alone, id 27 off");
    assert_tile(&atlas, 1, BASE, "id 34 alone, id 27 off");
}

/// Two movers at one probe: the first's lerp replaces id 34, the second adds
/// its lerped difference from id 34 (the brief's base + Σ deltas).
#[test]
fn two_movers_at_one_probe_add_their_lerped_differences_from_id_34() {
    let Some(ctx) =
        bc_gpu_or_skip("two_movers_at_one_probe_add_their_lerped_differences_from_id_34")
    else {
        return;
    };
    let section = channel_section(
        &[
            Bounce {
                mover_id: 7,
                masks: [0b11, 0b11],
                s0: 0.5,
                s1: 0.125,
            },
            Bounce {
                mover_id: 9,
                masks: [0b1, 0b1],
                s0: 0.25,
                s1: 1.0,
            },
        ],
        false,
    );
    let mut scene = build_scene(&ctx, Some(&section), false);
    for (t7, t9) in [(0.0, 1.0), (1.0, 0.0), (0.5, 0.5)] {
        let atlas = compose(&ctx, &mut scene, &[state(7, t7), state(9, t9)]);
        let lerp7 = 0.5 + (0.125 - 0.5) * t7;
        let lerp9 = 0.25 + (1.0 - 0.25) * t9;
        assert_tile(
            &atlas,
            0,
            lerp7 + (lerp9 - BASE) + DELTA_RED,
            &format!("probe 0 at t′ = ({t7}, {t9})"),
        );
        // Probe 1 is mover 7's alone.
        assert_tile(&atlas, 1, lerp7 + DELTA_RED, "probe 1, mover 7 alone");
    }
    // Only mover 9 handed over: its lerp alone replaces id 34 at probe 0.
    let atlas = compose(&ctx, &mut scene, &[state(9, 1.0)]);
    assert_tile(&atlas, 0, 1.0 + DELTA_RED, "mover 9 alone");
    assert_tile(&atlas, 1, BASE + DELTA_RED, "probe 1 with no state");
}

/// The summed base clamps per texel at zero, before id 27's delta adds: a
/// negative state, or a sum the second mover drives below zero, composes
/// 0 + delta, never a negative base.
#[test]
fn the_summed_base_clamps_per_texel_at_zero_before_deltas_add() {
    let Some(ctx) = bc_gpu_or_skip("the_summed_base_clamps_per_texel_at_zero_before_deltas_add")
    else {
        return;
    };
    let section = channel_section(
        &[
            Bounce {
                mover_id: 7,
                masks: [0b1, 0b1],
                s0: -0.5,
                s1: 0.5,
            },
            Bounce {
                mover_id: 9,
                masks: [0b10, 0b10],
                s0: 0.25,
                s1: -1.0,
            },
            Bounce {
                mover_id: 11,
                masks: [0b10, 0b10],
                s0: 0.5,
                s1: 0.5,
            },
        ],
        false,
    );
    let mut scene = build_scene(&ctx, Some(&section), false);
    let atlas = compose(&ctx, &mut scene, &[state(7, 0.0)]);
    assert_tile(&atlas, 0, DELTA_RED, "a negative state clamps to zero");
    // Probe 1: mover 9 at t′ = 1 gives −1.0, mover 11 adds 0.5 − 0.25:
    // −0.75 clamps to zero.
    let atlas = compose(&ctx, &mut scene, &[state(9, 1.0), state(11, 0.0)]);
    assert_tile(&atlas, 1, DELTA_RED, "a negative sum clamps to zero");
    // Clamping is per texel of the base, not of the result: a positive sum
    // passes through unchanged.
    let atlas = compose(
        &ctx,
        &mut scene,
        &[state(7, 1.0), state(9, 0.0), state(11, 0.0)],
    );
    assert_tile(&atlas, 0, 0.5 + DELTA_RED, "probe 0 positive");
    assert_tile(&atlas, 1, 0.25 + 0.25 + DELTA_RED, "probe 1 positive");
}

/// Task 11c: a terminus an affected probe stores no state for composes id
/// 34's own texel there. One-sided at terminus 0, the probe reads S^b₀ at
/// t′ = 0, the exact mix toward id 34 at 0.5, and today's atlas byte for
/// byte at 1; one-sided at terminus 1 likewise. A second mover at the probe
/// adds a zero difference at its unstored terminus, so the first mover's
/// lerp composes alone there.
#[test]
fn an_unstored_terminus_composes_id_34_and_adds_no_difference() {
    let Some(ctx) = bc_gpu_or_skip("an_unstored_terminus_composes_id_34_and_adds_no_difference")
    else {
        return;
    };
    let today = compose(&ctx, &mut build_scene(&ctx, None, false), &[]);
    let section = channel_section(
        &[
            Bounce {
                mover_id: 7,
                masks: [0b01, 0b10],
                s0: 0.5,
                s1: 0.125,
            },
            Bounce {
                mover_id: 9,
                masks: [0b10, 0b00],
                s0: 1.0,
                s1: 0.0,
            },
        ],
        false,
    );
    let mut scene = build_scene(&ctx, Some(&section), false);
    // Mover 7 alone: probe 0 stores S^b₀ only, probe 1 S^b₁ only.
    for (t, probe_0, probe_1) in [
        (0.0, 0.5, BASE),
        (0.5, 0.5 + (BASE - 0.5) * 0.5, BASE + (0.125 - BASE) * 0.5),
        (1.0, BASE, 0.125),
    ] {
        let atlas = compose(&ctx, &mut scene, &[state(7, t)]);
        assert_tile(
            &atlas,
            0,
            probe_0 + DELTA_RED,
            &format!("probe 0 at t′ = {t}"),
        );
        assert_tile(
            &atlas,
            1,
            probe_1 + DELTA_RED,
            &format!("probe 1 at t′ = {t}"),
        );
    }
    let open = compose(&ctx, &mut scene, &[state(7, 1.0)]);
    assert_eq!(
        outside_probe_1(&open),
        outside_probe_1(&today),
        "at its unstored terminus probe 0 composes today's texels exactly"
    );
    // Mover 9 at probe 1 stores S^b₀ only: at t′ = 1 it adds nothing, so
    // mover 7's lerp composes alone; at t′ = 0 it adds S^b₀ − id 34.
    for t7 in [0.0, 0.5, 1.0] {
        let lerp7 = BASE + (0.125 - BASE) * t7;
        let atlas = compose(&ctx, &mut scene, &[state(7, t7), state(9, 1.0)]);
        assert_tile(
            &atlas,
            1,
            lerp7 + DELTA_RED,
            &format!("probe 1, mover 9 at its unstored terminus, t7 = {t7}"),
        );
        let atlas = compose(&ctx, &mut scene, &[state(7, t7), state(9, 0.0)]);
        assert_tile(
            &atlas,
            1,
            lerp7 + (1.0 - BASE) + DELTA_RED,
            &format!("probe 1, mover 9 at S^b₀, t7 = {t7}"),
        );
    }
}

/// Every texel outside probe 1's tile, byte for byte.
fn outside_probe_1(atlas: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for y in 0..ATLAS[1] {
        for x in 0..ATLAS[0] {
            if (TILE..2 * TILE).contains(&x) && y < TILE {
                continue;
            }
            let at = ((y * ATLAS[0] + x) * 8) as usize;
            bytes.extend_from_slice(&atlas[at..at + 8]);
        }
    }
    bytes
}

/// B11/C10 (GPU): a streamed bounce block composes nothing at a cell until
/// its payload arrives, then exactly what the resident build composes; the
/// eviction frees the payload and the cell composes id 34 again; the
/// re-arrival composes the same bytes.
#[test]
fn a_streamed_bounce_cell_composes_the_resident_bytes_only_while_resident() {
    let Some(ctx) =
        bc_gpu_or_skip("a_streamed_bounce_cell_composes_the_resident_bytes_only_while_resident")
    else {
        return;
    };
    // Probe 2 stores S^b₀ only: the pooled payload holds stored tiles alone.
    let bounces = [Bounce {
        mover_id: 7,
        masks: [0b101, 0b001],
        s0: 0.5,
        s1: 0.125,
    }];
    let resident = channel_section(&bounces, false);
    let streamed = channel_section(&bounces, true);
    let states = [state(7, 0.5)];
    let expected = compose(
        &ctx,
        &mut build_scene(&ctx, Some(&resident), false),
        &states,
    );
    let today = compose(&ctx, &mut build_scene(&ctx, None, false), &[]);
    assert_ne!(expected, today);

    let mut scene = build_scene(&ctx, Some(&streamed), true);
    assert!(scene.sh.mover_channel.pools_cell(0));
    assert_eq!(scene.sh.mover_channel.pool_bytes().1, 0, "nothing resident");
    assert_eq!(
        compose(&ctx, &mut scene, &states),
        today,
        "a cell with no resident payload composes id 34"
    );

    // The row id 50 carries for cell 0: the resident build's tiles.
    let tile_bytes: Vec<u8> = resident
        .bounce_tiles
        .iter()
        .flat_map(|half| half.to_le_bytes())
        .collect();
    let payload_bytes = tile_bytes.len() as u64;
    arrive(&ctx, &mut scene, &tile_bytes);
    let (capacity, live) = scene.sh.mover_channel.pool_bytes();
    assert_eq!(live, payload_bytes, "the resident cell fills its payload");
    assert!(capacity >= live);
    assert_eq!(
        compose(&ctx, &mut scene, &states),
        expected,
        "a resident cell composes the resident build's bytes"
    );

    let uploads = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let mut batch = StagedUploads::from_scratch(Vec::new(), 0);
    scene
        .sh
        .mover_channel
        .release_cell(&mut batch, 0)
        .expect("the cell releases");
    submit(&ctx, &uploads, batch);
    assert_eq!(
        scene.sh.mover_channel.pool_bytes().1,
        0,
        "eviction frees the payload"
    );
    assert_eq!(compose(&ctx, &mut scene, &states), today);

    arrive(&ctx, &mut scene, &tile_bytes);
    assert_eq!(
        compose(&ctx, &mut scene, &states),
        expected,
        "the re-arrival composes the same bytes"
    );
    assert!(
        streamed.payload_held(MoverChannelBlock::Direct),
        "the fixture streams only its bounce block"
    );
}

fn submit(ctx: &GpuCtx, uploads: &UploadQueue, batch: StagedUploads) {
    let mut pool = StagingPool::default();
    batch.submit(&mut pool, &ctx.device, uploads);
    uploads.submit([]);
}

/// Cell 0's id-57 row arrives with its first cluster: placed in the pool,
/// written, then its base word published.
fn arrive(ctx: &GpuCtx, scene: &mut Scene, tile_bytes: &[u8]) {
    let rows = ClusterTermRows {
        channel_rows: vec![MoverStreamedRow {
            cell: 0,
            direct: &[],
            bounce: tile_bytes,
        }],
        ..ClusterTermRows::default()
    };
    let uploads = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let writes = scene
        .sh
        .mover_channel
        .stage_cluster(&ctx.device, &uploads, &rows, &[0])
        .expect("the row places");
    assert!(!writes.grew, "a whole-map pool holds the cell");
    let mut batch = StagedUploads::from_scratch(Vec::new(), 0);
    scene
        .sh
        .mover_channel
        .stage_writes(&mut batch, &writes)
        .expect("the cell writes");
    submit(ctx, &uploads, batch);
    scene.sh.mover_channel.commit_cluster(&writes);
}

/// A pooled table grows by replacement, and only the streamed passes rebind
/// to the new one (`rebind_mover_channel`). The whole-load composes, which a
/// streamed level never dispatches, bind an empty table in a pooled table's
/// place, so once the pool grows nothing but in-flight work holds its old
/// buffer. A table that never pools is the one they bind.
#[test]
fn the_whole_load_composes_never_bind_a_pooled_table_that_grows() {
    let Some(ctx) = bc_gpu_or_skip("the_whole_load_composes_never_bind_a_pooled_table_that_grows")
    else {
        return;
    };
    let device = &ctx.device;
    let bounces = [Bounce {
        mover_id: 7,
        masks: [0b101, 0b001],
        s0: 0.5,
        s1: 0.125,
    }];
    let resident = channel_section(&bounces, false);
    let streamed = channel_section(&bounces, true);
    let delta = indirect_delta();
    let mut ledger = ShAllocationLedger::new();
    let mut channel = |section, streaming| {
        MoverChannelResources::new(
            device,
            &MoverChannelInputs {
                section: Some(section),
                entity_shadow_lights: &[],
                static_terms: None,
                animated_terms: None,
                animation_descriptor_indices: &[],
                indirect_descriptor_indices: &delta.animation_descriptor_indices,
            },
            streaming,
            |_, _| None,
            &mut ledger,
        )
        .expect("the fixture channel lays out")
    };

    let whole = channel(&resident, false);
    let [direct, bounce] = whole.whole_load_tables(device);
    assert!(direct == *whole.buffer() && bounce == *whole.bounce_buffer());

    let mut pooled = channel(&streamed, true);
    // An empty pool, so the first resident cell grows it.
    pooled.size_streamed_pools(device, 0.0);
    assert_eq!(pooled.pool_bytes(), (0, 0));
    let first = pooled.bounce_buffer().clone();
    let [direct, bounce] = pooled.whole_load_tables(device);
    assert!(
        direct == *pooled.buffer(),
        "the resident direct block keeps its table"
    );
    assert!(bounce != first, "the pooled bounce table is not bound");

    let tile_bytes: Vec<u8> = resident
        .bounce_tiles
        .iter()
        .flat_map(|half| half.to_le_bytes())
        .collect();
    let rows = ClusterTermRows {
        channel_rows: vec![MoverStreamedRow {
            cell: 0,
            direct: &[],
            bounce: &tile_bytes,
        }],
        ..ClusterTermRows::default()
    };
    let uploads = UploadQueue::new(device, ctx.queue.clone(), true);
    let writes = pooled
        .stage_cluster(device, &uploads, &rows, &[0])
        .expect("the row places");
    assert!(writes.grew, "the empty pool grows for the cell");
    let grown = pooled.bounce_buffer().clone();
    assert!(grown != first, "growth replaces the table");
    assert!(
        bounce != first && bounce != grown,
        "the whole-load composes hold neither the old table nor the grown one"
    );
    assert!(pooled.pool_bytes().0 >= tile_bytes.len() as u64);
}
