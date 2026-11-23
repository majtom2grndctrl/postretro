//! Fixtures for the live-lighting sums GPU gates: a world-less pool level
//! whose probes store animated lights, the per-fragment oracle pipeline
//! (the reflecting variant with the kept per-fragment ratio), frames at a
//! chosen clock, and the sums buffer read back beside its CPU reference.
//! See: context/lib/rendering_pipeline.md §7.3a (Live lighting)

use std::time::{Duration, Instant};

use glam::Vec3;
use postretro_level_format::environment_volumes::EnvironmentVolumeRecord;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_RGBA16F, f32_to_f16_bits};
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1, REFLECTION_PROBE_LIGHTS_PER_PROBE,
    REFLECTION_PROBE_LOBE_COUNT, ReflectionProbeFluidBinding, ReflectionProbeLightShare,
    ReflectionProbeRecord, ReflectionProbesSection, reflection_probe_zero_distances,
};
use postretro_level_format::sh_volume::{AnimationDescriptor, OctahedralShVolumeSection};
use postretro_level_loader::ShDrainBatch;
use postretro_render_cpu::fluid_surface::FluidGridLattice;
use postretro_render_cpu::fluid_surface::prepared::prepare_fluid_surfaces;
use postretro_render_cpu::fluid_surface::reflection::{
    FLUID_PROBE_SUMS_BYTES, FluidProbeGpu, FluidReflectionLights, reflection_probe_sums,
};
use postretro_render_cpu::sh_compose::AnimatedLightScaleDescriptor;
use postretro_render_cpu::sh_volume::{
    ANIMATION_DESCRIPTOR_SIZE, build_animation_buffers_from_descriptors,
};
use postretro_render_data::geometry::BvhTree;
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};

use super::fluid_baked_frame_test_fixtures::sh_section;
use super::fluid_reflection_frame_test_fixtures::{PROBE_TILE, View};
use super::level_build::{GeometryExtras, GeometryRest, PendingLevelSet};
use super::level_resource_set::LevelSetId;
use super::renderer_render_frame::FrameScene;
use super::{ClearColor, LevelGeometry, LevelGeometryShStorage, Renderer, ShSampleRegionSets};

const WHITE: ClearColor = ClearColor {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};
const ALL: VisibleCells = VisibleCells::DrawAll;

/// The SH volume every fixture level carries, only so its animation
/// descriptors are the level's roster: one probe whose lattice cuts no pool
/// face.
const SH_ORIGIN: [f32; 3] = [-10.0, -2.0, -10.0];
const SH_CELL: [f32; 3] = [20.0, 8.0, 20.0];

/// A flat light of `base_color`: no curve, so its value is `base_color`
/// while on and 0 while off.
pub(super) fn flat_light(base_color: [f32; 3], on: bool) -> AnimationDescriptor {
    AnimationDescriptor {
        base_color,
        start_active: u32::from(on),
        ..AnimationDescriptor::default()
    }
}

/// `light`'s 48-byte descriptor record as the light bridge writes it into
/// the descriptor mirror. Flat lights only: a curve's sample offsets are
/// the installed buffer's, not a fresh one's.
pub(super) fn descriptor_record(light: &AnimationDescriptor) -> [u8; ANIMATION_DESCRIPTOR_SIZE] {
    assert!(
        light.brightness.is_empty() && light.color.is_empty() && light.direction.is_empty(),
        "only flat lights are rewritten at runtime here"
    );
    let (bytes, _, _) = build_animation_buffers_from_descriptors(std::slice::from_ref(light));
    bytes[..ANIMATION_DESCRIPTOR_SIZE]
        .try_into()
        .expect("one descriptor record")
}

/// A stored light naming roster light `roster_index`, captured at
/// `reference`, holding `share` of the capture's light in every lobe but
/// the up lobe, which holds `up`.
pub(super) fn stored_light(
    roster_index: u32,
    reference: [f32; 3],
    share: f32,
    up: f32,
) -> ReflectionProbeLightShare {
    let mut shares = [[share; 3]; REFLECTION_PROBE_LOBE_COUNT];
    shares[0] = [up; 3];
    ReflectionProbeLightShare {
        roster_index,
        reference,
        shares,
    }
}

/// One probe over a box holding the whole pool scene, on layer `layer`,
/// storing `lights` (unused past their count). [`probe_section`] gives its
/// capture.
pub(super) fn probe(layer: u32, lights: &[ReflectionProbeLightShare]) -> ReflectionProbeRecord {
    assert!(lights.len() <= REFLECTION_PROBE_LIGHTS_PER_PROBE);
    let mut stored = [ReflectionProbeLightShare::UNUSED; REFLECTION_PROBE_LIGHTS_PER_PROBE];
    stored[..lights.len()].copy_from_slice(lights);
    ReflectionProbeRecord {
        source_entity: layer,
        box_min: [-10.0; 3],
        box_max: [10.0; 3],
        capture_point: [0.0, 0.002, 4.0],
        layer,
        lights: stored,
    }
}

/// Probe `index`'s capture colour: green-dominant and distinct per probe,
/// dim enough that a ratio up to 2 stays below the soft knee.
pub(super) fn probe_colour(index: usize) -> [f32; 3] {
    [
        0.05 + 0.04 * (index % 4) as f32,
        0.4,
        0.05 + 0.03 * (index % 5) as f32,
    ]
}

/// A section over `probes` (each on its own layer, in order), each capture
/// the uniform [`probe_colour`] of its index with zero distances (box
/// projection), bound to each `(volume, face, probe)` of `bindings`.
pub(super) fn probe_section(
    probes: Vec<ReflectionProbeRecord>,
    bindings: &[(u32, u32, u32)],
) -> ReflectionProbesSection {
    let tiles = (0..probes.len())
        .flat_map(|index| {
            let [r, g, b] = probe_colour(index).map(f32_to_f16_bits);
            let texel: Vec<u8> = [r, g, b, 0x3C00]
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect();
            texel.repeat((PROBE_TILE * PROBE_TILE) as usize)
        })
        .collect();
    ReflectionProbesSection {
        tile_dimension: PROBE_TILE,
        tile_border: 1,
        atlas_format: IRRADIANCE_FORMAT_RGBA16F,
        colour_mip_count: 1,
        world_bindings: Vec::new(),
        distances: reflection_probe_zero_distances(probes.len(), PROBE_TILE),
        probes,
        fluid_bindings: bindings
            .iter()
            .map(|&(volume, face, probe)| ReflectionProbeFluidBinding {
                volume,
                face,
                probe,
            })
            .collect(),
        tiles,
        distance_format: REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1,
    }
}

/// A world-less level: its animated-light roster, its fluid volumes and
/// their probes.
#[derive(Clone)]
pub(super) struct SumsLevel {
    pub roster: Vec<AnimationDescriptor>,
    pub volumes: Vec<EnvironmentVolumeRecord>,
    pub probes: ReflectionProbesSection,
}

impl SumsLevel {
    fn sh(&self) -> OctahedralShVolumeSection {
        let mut sh = sh_section(SH_ORIGIN, SH_CELL, [1, 1, 1], |_| Vec3::splat(0.2));
        sh.animation_descriptors = self.roster.clone();
        sh
    }

    /// Install and publish the level on the main thread: geometry (with the
    /// roster), then its fluid surfaces.
    pub fn install(&self, renderer: &mut Renderer) {
        with_sh_level(&self.sh(), |geometry| {
            renderer
                .install_level_geometry(geometry, Default::default())
                .expect("level resources install");
        });
        renderer.install_fluid_surfaces(&self.volumes, Some(&self.probes));
        assert_eq!(
            renderer.animated_compose_descriptor_count(),
            self.roster.len() as u32,
            "the level's roster installs"
        );
    }

    /// Build the level off the main thread into a resident, unpublished set,
    /// fluid surfaces included, as a level load does.
    pub fn build_off_main(&self, renderer: &mut Renderer) -> LevelSetId {
        let mut pending = self.begin_off_main(renderer);
        take(renderer, &mut pending)
    }

    /// Start [`Self::build_off_main`]'s build on the upload thread; the
    /// caller draws meanwhile and takes the set with [`take`].
    pub fn begin_off_main(&self, renderer: &mut Renderer) -> PendingLevelSet {
        let tiers = renderer.level_build_tiers();
        let (builder, pending) = renderer.begin_level_set_build(tiers);
        let lattice = FluidGridLattice::new(SH_ORIGIN, SH_CELL);
        let fluid = prepare_fluid_surfaces(
            &self.volumes,
            Some(&self.probes),
            &[(64, 64)],
            1,
            lattice.as_ref(),
        );
        let (sh, volumes, probes) = (self.sh(), self.volumes.clone(), self.probes.clone());
        builder.geometry_from(
            Box::new(move |visit| {
                with_sh_level(&sh, |geometry| {
                    visit(
                        geometry,
                        GeometryExtras {
                            volumes: &volumes,
                            reflection_probes: Some(&probes),
                            fog_pixel_scale: 0,
                            fog_cell_masks: None,
                        },
                    )
                })
            }),
            GeometryRest {
                gpu_lighting_payloads: Default::default(),
                sh_plan: None,
                lightmap_plan: None,
                fluid,
                planar_candidate: None,
            },
        );
        builder.finish();
        pending
    }

    /// Every installed probe's sums from the CPU reference, over `roster`
    /// (the descriptors as the GPU holds them) at animation time `time`.
    pub fn cpu_sums(
        &self,
        roster: &[AnimationDescriptor],
        time: f32,
    ) -> Vec<[Vec3; REFLECTION_PROBE_LOBE_COUNT]> {
        let descriptors = scale_descriptors(roster);
        let lights = FluidReflectionLights {
            descriptors: &descriptors,
            time,
        };
        self.probes
            .probes
            .iter()
            .map(|probe| reflection_probe_sums(&FluidProbeGpu::pack(probe).lights, &lights))
            .collect()
    }
}

/// `roster` as `animated_light_scale` reads it.
fn scale_descriptors(roster: &[AnimationDescriptor]) -> Vec<AnimatedLightScaleDescriptor<'_>> {
    roster
        .iter()
        .map(|light| AnimatedLightScaleDescriptor {
            period: light.period,
            phase: light.phase,
            base_color: light.base_color,
            brightness: &light.brightness,
            color: &light.color,
            is_active: light.start_active != 0,
        })
        .collect()
}

/// Poll `pending` until it reports, failing after a bound.
pub(super) fn take(renderer: &mut Renderer, pending: &mut PendingLevelSet) -> LevelSetId {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(outcome) = pending.try_take(renderer) {
            return outcome.expect("the off-main build succeeds");
        }
        assert!(
            Instant::now() < deadline,
            "the build did not report within 120 s"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// A level with no world geometry and `sh` as its SH volume, lent to
/// `use_geometry` to install or build.
fn with_sh_level<R>(
    sh: &OctahedralShVolumeSection,
    use_geometry: impl FnOnce(&LevelGeometry<'_>) -> R,
) -> R {
    let bvh = BvhTree {
        nodes: Vec::new(),
        leaves: Vec::new(),
        root_node_index: 0,
    };
    use_geometry(&LevelGeometry {
        vertices: &[],
        vertex_spec_owners: &[],
        vertex_emissive_volumes: &[],
        emissive_volumes: &[],
        area_glow_lights: &[],
        indices: &[],
        bvh: &bvh,
        lights: &[],
        light_influences: &[],
        sh_volume: Some(sh),
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
        mover_probe_channel: None,
        entity_shadow_lights: &[],
        shadowmask_atlas: None,
        sdf_atlas: None,
        lightmap_mode: postretro_level_loader::LightmapMode::default(),
        cell_draw_index: None,
        kinematic_geometry: None,
        cells: &[],
        texture_materials: &[],
    })
}

/// One captured frame through `view` at animation time `time`, inside a
/// validation error scope.
pub(super) fn frame_at(renderer: &mut Renderer, view: View, time: f32) -> Vec<u8> {
    let scope = renderer
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    let pixels = renderer
        .capture_frame_indirect(
            CameraCullVisibility {
                cells: &ALL,
                path: VisibilityPath::EmptyWorldFallback,
            },
            &[],
            &[],
            &[],
            ShSampleRegionSets {
                visible_cells: &ALL,
                fog_cells: &[],
                movers: &[],
            },
            None,
            view.view_proj,
            view.eye,
            &[],
            &[],
            time,
            WHITE,
            true,
            ShDrainBatch::default(),
        )
        .expect("capture drains")
        .frame
        .expect("capture records");
    let error = pollster::block_on(scope.pop());
    assert!(
        error.is_none(),
        "the frame raised a validation error: {error:?}"
    );
    pixels
}

/// One frame recorded as capture records it, but as `scene` and with
/// `between` run after the frame's uniform upload and before its render.
/// Leaves the HDR scene colour for [`super::fluid_reflection_frame_test_fixtures::read_scene`].
pub(super) fn frame_split_at_upload(
    renderer: &mut Renderer,
    view: View,
    time: f32,
    scene: FrameScene,
    between: impl FnOnce(&mut Renderer),
) {
    let scope = renderer
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    renderer.update_per_frame_uniforms(view.view_proj, view.eye, time);
    between(renderer);
    let mut encoder = renderer
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Fluid sums proof frame"),
        });
    renderer
        .record_scene_passes(
            &mut encoder,
            None,
            None,
            CameraCullVisibility {
                cells: &ALL,
                path: VisibilityPath::EmptyWorldFallback,
            },
            &[],
            &[],
            &[],
            ShSampleRegionSets {
                visible_cells: &ALL,
                fog_cells: &[],
                movers: &[],
            },
            None,
            view.view_proj,
            &[],
            &[],
            f64::from(time),
            WHITE,
            scene,
        )
        .expect("the frame records");
    renderer.queue.submit(std::iter::once(encoder.finish()));
    renderer.queue.complete_frame();
    renderer
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let error = pollster::block_on(scope.pop());
    assert!(
        error.is_none(),
        "the frame raised a validation error: {error:?}"
    );
}

/// Sums-pass dispatches recorded so far.
pub(super) fn dispatches(renderer: &Renderer) -> u32 {
    renderer.full().fluid_surface.probe_sums_dispatches()
}

/// The per-fragment oracle: the reflecting variant with the kept
/// per-fragment ratio, held while production draws.
pub(super) struct Oracle(Option<wgpu::RenderPipeline>);

impl Oracle {
    pub fn new(renderer: &Renderer) -> Self {
        Self(Some(
            renderer
                .full()
                .fluid_surface
                .build_per_fragment_oracle_pipeline(&renderer.device, super::DEPTH_FORMAT),
        ))
    }

    /// `frame` drawn with the per-fragment oracle in the production
    /// reflecting pipeline's place, which is restored after.
    pub fn before(
        &mut self,
        renderer: &mut Renderer,
        frame: impl FnOnce(&mut Renderer) -> Vec<u8>,
    ) -> Vec<u8> {
        let oracle = self.0.take().expect("the oracle is held between frames");
        let kept = renderer
            .full_mut()
            .fluid_surface
            .swap_reflecting_pipeline(oracle);
        let pixels = frame(renderer);
        self.0 = Some(
            renderer
                .full_mut()
                .fluid_surface
                .swap_reflecting_pipeline(kept),
        );
        pixels
    }

    /// `frame` drawn with the production pipeline, then drawn again, same
    /// pose and clock, by the per-fragment oracle: `(production, oracle)`.
    /// Production draws first, so a first-drawn case is first drawn there.
    pub fn pair(
        &mut self,
        renderer: &mut Renderer,
        mut frame: impl FnMut(&mut Renderer) -> Vec<u8>,
    ) -> (Vec<u8>, Vec<u8>) {
        let production = frame(renderer);
        let before = self.before(renderer, frame);
        (production, before)
    }
}

/// The largest per-channel RGB gap between two captures, and its pixel.
pub(super) fn worst_gap(a: &[u8], b: &[u8]) -> (u8, usize) {
    assert_eq!(a.len(), b.len());
    a.as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .enumerate()
        .map(|(pixel, (a, b))| {
            (
                (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0),
                pixel,
            )
        })
        .max()
        .unwrap_or((0, 0))
}

/// Pixels whose RGB differs by more than `threshold` in some channel.
pub(super) fn pixels_differing(a: &[u8], b: &[u8], threshold: u8) -> usize {
    a.as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .filter(|(a, b)| (0..3).any(|c| a[c].abs_diff(b[c]) > threshold))
        .count()
}

/// The sums buffer `buffer` as the GPU holds it: one `[f32; 4]` per lobe per
/// probe at the cap, probe-major.
pub(super) fn read_sums(renderer: &Renderer, buffer: &wgpu::Buffer) -> Vec<[f32; 4]> {
    let size = FLUID_PROBE_SUMS_BYTES as u64;
    let staging = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Fluid sums proof readback"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = renderer
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Fluid sums proof readback"),
        });
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
    renderer
        .queue
        .submit_unbatched([encoder.finish()], "fluid sums proof readback");
    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).ok();
    });
    renderer
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    rx.recv().expect("map callback").expect("map");
    let sums = slice
        .get_mapped_range()
        .expect("mapped")
        .as_chunks::<16>()
        .0
        .iter()
        .map(|lobe| {
            [0, 1, 2, 3].map(|c| f32::from_le_bytes(lobe[4 * c..4 * c + 4].try_into().unwrap()))
        })
        .collect();
    staging.unmap();
    sums
}

/// The active level's sums, read back.
pub(super) fn read_active_sums(renderer: &Renderer) -> Vec<[f32; 4]> {
    let buffer = renderer
        .fluid_surfaces()
        .expect("a fluid level is installed")
        .sums_buffer_for_test()
        .clone();
    read_sums(renderer, &buffer)
}

/// Fill the active level's sums with `value` in every lane, landing before
/// the next frame's commands.
pub(super) fn prefill_active_sums(renderer: &Renderer, value: f32) {
    let buffer = renderer
        .fluid_surfaces()
        .expect("a fluid level is installed")
        .sums_buffer_for_test();
    let bytes: Vec<u8> = std::iter::repeat_n(value.to_le_bytes(), FLUID_PROBE_SUMS_BYTES / 4)
        .flatten()
        .collect();
    renderer.queue.raw().write_buffer(buffer, 0, &bytes);
}

/// The largest deviation of the GPU's sums from `expected` over every
/// installed probe and lobe, relative to the larger of 1 and the expected
/// magnitude, with its (probe, lobe). The fourth lane must read 0.
pub(super) fn worst_sums_deviation(
    gpu: &[[f32; 4]],
    expected: &[[Vec3; REFLECTION_PROBE_LOBE_COUNT]],
) -> (f32, (usize, usize)) {
    let mut worst = (0.0f32, (0, 0));
    for (probe, lobes) in expected.iter().enumerate() {
        for (lobe, want) in lobes.iter().enumerate() {
            let got = gpu[probe * REFLECTION_PROBE_LOBE_COUNT + lobe];
            assert_eq!(got[3], 0.0, "probe {probe} lobe {lobe}: fourth lane");
            for channel in 0..3 {
                let deviation = (got[channel] - want[channel]).abs() / want[channel].abs().max(1.0);
                if deviation > worst.0 || deviation.is_nan() {
                    worst = (deviation, (probe, lobe));
                }
            }
        }
    }
    worst
}

/// The sums' relative tolerance: 1e-4 of the larger of 1 and the value.
pub(super) const SUMS_TOLERANCE: f32 = 1.0e-4;
