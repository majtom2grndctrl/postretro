//! SPIKE (cell-rate spike, never merged): adapter-backed honesty gates for the
//! pixel-style reflection cell atlas, through the production capture recorder.
//! See: context/lib/rendering_pipeline.md §7.3a (Reflection → Style)

use glam::{IVec2, Mat4, Vec2, Vec3};
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex, EnvironmentVolumeRecord,
};
use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1, REFLECTION_PROBE_LIGHTS_PER_PROBE,
    ReflectionProbeBinding, ReflectionProbeLightShare, ReflectionProbeRecord,
    ReflectionProbesSection,
};
use postretro_level_format::sh_volume::AnimationDescriptor;
use postretro_level_loader::ShDrainBatch;
use postretro_render_cpu::fluid_surface::motion::displayed_offset;
use postretro_render_cpu::fluid_surface::reflection_cells::{
    FluidReflectionCellLevers, cell_texels_per_pixel,
};
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;
use postretro_render_cpu::fluid_surface::{FluidShaderVariant, FluidSurfaceLook};
use postretro_render_cpu::sh_compose::f16_bits_to_f32;
use postretro_render_cpu::sh_volume::{
    ANIMATION_DESCRIPTOR_SIZE, build_animation_buffers_from_descriptors, f32_to_f16_bits,
};
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};

use super::fluid_reflection_cells::{lever_constants, lever_pipeline_index};
use super::fluid_surface::fluid_variant_constants;
use super::{ClearColor, FluidReflectionCellStats, FluidSurfaceDraw, Renderer, ShSampleRegionSets};

const SIDE: u32 = 64;
/// Texels per metre on the pool: half-metre cells, large on screen.
const TEXELS_PER_METRE: f32 = 2.0;
const POOL_HALF: f32 = 9.0;
/// The capture recorder's animation clock (`capture_through` passes it).
const CAPTURE_TIME: f32 = 1.0;
const WHITE: ClearColor = ClearColor {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};
const ALL: VisibleCells = VisibleCells::DrawAll;
const CAPTURE_POINT: Vec3 = Vec3::new(0.0, 0.002, 2.0);
const PROBE_DIMENSION: u32 = 64;

fn renderer() -> Option<Renderer> {
    match Renderer::new_offscreen(SIDE, SIDE) {
        Ok(renderer) => Some(renderer),
        Err(error) if error.to_string().contains("requires a GPU adapter") => {
            eprintln!("[FluidCellSpike] skipped: no adapter ({error:#})");
            None
        }
        Err(error) => panic!("adapter present but renderer initialization failed: {error:#}"),
    }
}

/// A horizontal pool at height 0, outward +Y, `±POOL_HALF` metres.
fn pool() -> EnvironmentVolumeRecord {
    let corner = |x: f32, z: f32| EnvironmentSurfaceVertex {
        position: [x, 0.0, z],
        uv: [x * TEXELS_PER_METRE, z * TEXELS_PER_METRE],
    };
    let h = POOL_HALF;
    EnvironmentVolumeRecord {
        source_entity: 0,
        priority: 0,
        gravity: None,
        push: None,
        fluid: "water".to_string(),
        brushes: Vec::new(),
        surface_faces: vec![EnvironmentSurfaceFace {
            texture_index: 0,
            vertices: vec![corner(-h, -h), corner(-h, h), corner(h, h), corner(h, -h)],
        }],
    }
}

/// The room the distance tile holds, engine metres: walls at x ±10, z −10,
/// a near wall at z 5, ceiling 6, floor −1. Each plane is (axis, coordinate).
const ROOM: [(usize, f32); 6] = [
    (0, -10.0),
    (0, 10.0),
    (2, -10.0),
    (2, 5.0),
    (1, 6.0),
    (1, -1.0),
];

fn room_distance(capture: Vec3, dir: Vec3) -> f32 {
    ROOM.iter()
        .filter_map(|&(axis, at)| {
            let t = (at - capture[axis]) / dir[axis];
            (dir[axis].abs() > 1.0e-6 && t > 0.0).then_some(t)
        })
        .fold(f32::INFINITY, f32::min)
}

fn room_distance_tile(capture: Vec3, dimension: u32) -> Vec<u8> {
    use postretro_level_format::octahedral::{
        irradiance_interior_texel_direction, irradiance_tile_source_texel,
    };
    (0..dimension)
        .flat_map(|y| (0..dimension).map(move |x| (x, y)))
        .flat_map(|(x, y)| {
            let [ix, iy] = irradiance_tile_source_texel(x, y, dimension, 1);
            let dir = Vec3::from(irradiance_interior_texel_direction(ix, iy, dimension, 1));
            let t = room_distance(capture, dir);
            f32_to_f16_bits(1.0 / (t * dir.abs().element_sum())).to_le_bytes()
        })
        .collect()
}

/// Smooth stripes along both tile axes, so a lookup a fraction of a texel
/// off reads another colour and nothing reaches the bloom threshold.
fn striped_tile(dimension: u32) -> Vec<u8> {
    let stripe = |texel: u32| {
        let phase = (texel as f32 + 0.5) / dimension as f32 * 2.0 * std::f32::consts::TAU;
        f32_to_f16_bits(0.4 + 0.4 * phase.sin())
    };
    (0..dimension)
        .flat_map(|row| (0..dimension).map(move |column| (column, row)))
        .flat_map(|(column, row)| [stripe(column), stripe(row), f32_to_f16_bits(0.2), 0x3C00])
        .flat_map(|half| half.to_le_bytes())
        .collect()
}

/// One probe over the pool, box `box_min..box_max`, its one stored light
/// at reference 2 with a different share per lobe: the light's descriptor
/// (1, on) halves its share, so the live ratio varies with the lookup.
fn section(box_min: [f32; 3], box_max: [f32; 3]) -> ReflectionProbesSection {
    let mut lights = [ReflectionProbeLightShare::UNUSED; REFLECTION_PROBE_LIGHTS_PER_PROBE];
    lights[0] = ReflectionProbeLightShare {
        roster_index: 0,
        reference: [2.0; 3],
        shares: [
            [0.9, 0.8, 0.7],
            [0.1, 0.2, 0.3],
            [0.5, 0.5, 0.5],
            [0.3, 0.6, 0.2],
            [0.7, 0.1, 0.9],
        ],
    };
    ReflectionProbesSection {
        tile_dimension: PROBE_DIMENSION,
        tile_border: 1,
        atlas_format: IRRADIANCE_FORMAT_RGBA16F,
        probes: vec![ReflectionProbeRecord {
            source_entity: 0,
            box_min,
            box_max,
            capture_point: CAPTURE_POINT.to_array(),
            layer: 0,
            lights,
        }],
        bindings: vec![ReflectionProbeBinding {
            volume: 0,
            face: 0,
            probe: 0,
        }],
        tiles: striped_tile(PROBE_DIMENSION),
        distance_format: REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1,
        distances: room_distance_tile(CAPTURE_POINT, PROBE_DIMENSION),
    }
}

fn full_box() -> ReflectionProbesSection {
    section([-10.0, -1.0, -10.0], [10.0, 6.0, 10.0])
}

/// A box whose x faces sit off the half-metre cell grid, so pool cells
/// straddle its rim.
fn rim_box() -> ReflectionProbesSection {
    section([-2.6, -1.0, -10.0], [2.6, 6.0, 10.0])
}

fn mirror() -> FluidSurfaceLook {
    FluidSurfaceLook {
        surface_reflectivity: 50.0,
        has_bound_face: true,
        ..FluidSurfaceLook::surface_only(0.0)
    }
}

fn water() -> FluidSurfaceLook {
    FluidSurfaceLook {
        surface_reflectivity: 1.0,
        has_bound_face: true,
        ..FluidSurfaceLook::surface_only(0.6)
    }
}

fn draw(look: FluidSurfaceLook) -> FluidSurfaceDraw {
    FluidSurfaceDraw {
        volume_index: 0,
        params: look.pack(),
    }
}

fn flat_descriptor() -> [u8; ANIMATION_DESCRIPTOR_SIZE] {
    let (bytes, _, _) = build_animation_buffers_from_descriptors(&[AnimationDescriptor {
        base_color: [1.0; 3],
        start_active: 1,
        ..AnimationDescriptor::default()
    }]);
    bytes[..ANIMATION_DESCRIPTOR_SIZE]
        .try_into()
        .expect("one descriptor record")
}

/// A renderer with `section` installed over the pool, drawing `look`, the
/// stored light's descriptor on.
fn scene(section: &ReflectionProbesSection, look: FluidSurfaceLook) -> Option<Renderer> {
    let mut renderer = renderer()?;
    renderer
        .full_mut()
        .sh_volume_resources
        .animation
        .write_record_for_test(0, &flat_descriptor());
    renderer.install_fluid_surfaces(std::slice::from_ref(&pool()), Some(section));
    renderer.set_fluid_surface_draws(&[draw(look)]);
    Some(renderer)
}

#[derive(Debug, Clone, Copy)]
struct Pose {
    name: &'static str,
    camera: Vec3,
    target: Vec3,
}

impl Pose {
    fn view_proj(&self) -> Mat4 {
        glam::camera::rh::proj::directx::perspective(60f32.to_radians(), 1.0, 0.1, 100.0)
            * glam::camera::rh::view::look_at_mat4(self.camera, self.target, Vec3::Y)
    }
}

const TOP_DOWN: Pose = Pose {
    name: "top-down",
    camera: Vec3::new(0.0, 2.0, 0.0),
    target: Vec3::new(0.0, 0.0, 1.5),
};
const AT_RIM: Pose = Pose {
    name: "probe-box rim",
    camera: Vec3::new(1.5, 3.0, 0.0),
    target: Vec3::new(2.6, 0.0, 1.0),
};
const OBLIQUE: Pose = Pose {
    name: "oblique",
    camera: Vec3::new(0.0, 0.6, -8.5),
    target: Vec3::new(0.5, 0.0, 0.0),
};

fn capture(renderer: &mut Renderer, pose: Pose) -> Vec<u8> {
    renderer
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
            pose.view_proj(),
            pose.camera,
            &[],
            &[],
            CAPTURE_TIME,
            WHITE,
            true,
            ShDrainBatch::default(),
        )
        .unwrap()
        .frame
        .unwrap()
}

/// [`capture`] with the stats armed for that frame.
fn capture_with_stats(renderer: &mut Renderer, pose: Pose) -> (Vec<u8>, FluidReflectionCellStats) {
    renderer.arm_fluid_reflection_cell_stats();
    let pixels = capture(renderer, pose);
    let stats = renderer
        .take_fluid_reflection_cell_stats()
        .expect("an armed frame reports stats");
    (pixels, stats)
}

fn set_levers(renderer: &mut Renderer, cells: bool, frustum_cull: bool, ratio_one: bool) {
    renderer.set_fluid_reflection_cell_levers(FluidReflectionCellLevers {
        cells,
        frustum_cull,
        ratio_one,
    });
}

/// World point of pixel (`column`, `row`)'s centre on the plane y = 0.
fn pixel_on_water(view_proj: Mat4, column: u32, row: u32) -> Option<Vec3> {
    let step = 2.0 / SIDE as f32;
    let ndc = Vec2::new(
        (column as f32 + 0.5) * step - 1.0,
        1.0 - (row as f32 + 0.5) * step,
    );
    let inverse = view_proj.inverse();
    let near = inverse.project_point3(ndc.extend(0.0));
    let far = inverse.project_point3(ndc.extend(1.0));
    let dir = far - near;
    if dir.y >= 0.0 {
        return None;
    }
    let point = near + dir * (-near.y / dir.y);
    (point.x.abs() < POOL_HALF && point.z.abs() < POOL_HALF).then_some(point)
}

/// The pixel's texel coordinate, and whether it lies within `margin` texels
/// of a cell boundary (where the CPU's reprojection and the fragment's
/// interpolated UV may floor to neighbouring cells).
fn pixel_texel(point: Vec3, margin: f32) -> (Vec2, bool) {
    let t = Vec2::new(point.x, point.z) * TEXELS_PER_METRE;
    let f = t - t.floor();
    let boundary = f.min_element() < margin || f.max_element() > 1.0 - margin;
    (t, boundary)
}

const AXIS_U: Vec3 = Vec3::new(1.0 / TEXELS_PER_METRE, 0.0, 0.0);
const AXIS_V: Vec3 = Vec3::new(0.0, 0.0, 1.0 / TEXELS_PER_METRE);

#[derive(Debug, Default)]
struct Parity {
    worst: u8,
    worst_at: (u32, u32),
    /// Pixels off by more than 2/255 in any channel.
    over: usize,
    /// Of those, pixels whose cell straddles its fade threshold.
    over_fade: usize,
    /// Of those, pixels on a cell boundary (CPU classification unsure).
    over_boundary: usize,
    /// The worst pixel neither explanation covers.
    worst_unexplained: u8,
    /// Water pixels whose cell straddles its fade threshold, of `water`.
    fade_split: usize,
    water: usize,
    /// Largest analytic texels per pixel over the visible cells.
    max_tpp: f32,
}

/// Compare the per-fragment and cell-rate frames pixel by pixel, classifying
/// each mismatch: a cell whose displayed offset changes between its centre's
/// analytic texels-per-pixel and the pixel's own (±5 % for the quad
/// derivative) straddles its fade threshold, which the per-fragment path
/// splits and the cell path does not.
fn parity(per_fragment: &[u8], cells: &[u8], pose: Pose) -> Parity {
    let view_proj = pose.view_proj();
    let extent = Vec2::splat(SIDE as f32);
    let mut out = Parity::default();
    for row in 0..SIDE {
        for column in 0..SIDE {
            let at = ((row * SIDE + column) * 4) as usize;
            let deviation = (0..3)
                .map(|c| per_fragment[at + c].abs_diff(cells[at + c]))
                .max()
                .unwrap_or(0);
            if deviation > out.worst {
                out.worst = deviation;
                out.worst_at = (column, row);
            }
            let Some(point) = pixel_on_water(view_proj, column, row) else {
                if deviation > 2 {
                    out.over += 1;
                    out.worst_unexplained = out.worst_unexplained.max(deviation);
                }
                continue;
            };
            out.water += 1;
            let (t, boundary) = pixel_texel(point, 0.02);
            let cell = t.floor().as_ivec2();
            let centre = Vec3::new(
                (cell.x as f32 + 0.5) / TEXELS_PER_METRE,
                0.0,
                (cell.y as f32 + 0.5) / TEXELS_PER_METRE,
            );
            let tpp_centre = cell_texels_per_pixel(view_proj, extent, centre, AXIS_U, AXIS_V);
            let tpp_pixel = cell_texels_per_pixel(view_proj, extent, point, AXIS_U, AXIS_V);
            out.max_tpp = out.max_tpp.max(tpp_centre);
            let offset = |tpp: f32| displayed_offset(cell, CAPTURE_TIME, tpp);
            let fade = [0.95, 1.0, 1.05]
                .iter()
                .any(|k| offset(tpp_pixel * k) != offset(tpp_centre));
            if fade {
                out.fade_split += 1;
            }
            if deviation > 2 {
                out.over += 1;
                if fade {
                    out.over_fade += 1;
                } else if boundary {
                    out.over_boundary += 1;
                } else {
                    out.worst_unexplained = out.worst_unexplained.max(deviation);
                }
            }
        }
    }
    out
}

/// Gate A: parity. A pixel-style mirror over a striped probe with a near
/// wall in its distance tile and a live light whose share differs per lobe,
/// rendered per fragment and from the cell atlas, matches within 2/255 at
/// poses whose cells all sit far from the fade band (top-down, and with
/// cells straddling a probe-box rim), and at an oblique pose every pixel
/// past 2/255 lies on a cell straddling its fade threshold, the one
/// expected divergence: the per-fragment path decides the fade per
/// fragment (quad derivatives), the cell path once at the centre
/// (analytic). Both with the live ratio and with it forced to 1. Not
/// vacuous: the cell pass evaluates cells (stats) and the ratio lever
/// moves the frame.
#[test]
fn cell_rate_reflection_matches_per_fragment_within_two_levels() {
    struct Case {
        pose: Pose,
        section: fn() -> ReflectionProbesSection,
        look: fn() -> FluidSurfaceLook,
        exact: bool,
    }
    let cases = [
        Case {
            pose: TOP_DOWN,
            section: full_box,
            look: mirror,
            exact: true,
        },
        Case {
            pose: TOP_DOWN,
            section: full_box,
            look: water,
            exact: true,
        },
        Case {
            pose: AT_RIM,
            section: rim_box,
            look: mirror,
            exact: true,
        },
        Case {
            pose: OBLIQUE,
            section: full_box,
            look: mirror,
            exact: false,
        },
    ];
    // The rim pose sees cells straddling the rim box's x = 2.6 face (the
    // cell 2.5..3.0 m), which the pixel style pulls inside the box.
    let straddling = (0..SIDE * SIDE)
        .filter_map(|i| pixel_on_water(AT_RIM.view_proj(), i % SIDE, i / SIDE))
        .filter(|point| (2.5..3.0).contains(&point.x))
        .count();
    assert!(straddling > 50, "rim cells in view: {straddling} px");
    for case in cases {
        let Some(mut renderer) = scene(&(case.section)(), (case.look)()) else {
            return;
        };
        let mut ratio_frames = Vec::new();
        for ratio_one in [false, true] {
            set_levers(&mut renderer, false, false, ratio_one);
            let per_fragment = capture(&mut renderer, case.pose);
            set_levers(&mut renderer, true, false, ratio_one);
            let (cells, stats) = capture_with_stats(&mut renderer, case.pose);
            let result = parity(&per_fragment, &cells, case.pose);
            eprintln!(
                "[FluidCellSpike] gate A {} (reflectivity {}, ratio_one {ratio_one}): worst \
                 {} at {:?}, {} px > 2/255 of {} water px ({} on fade-split cells, {} on cell \
                 boundaries), worst unexplained {}, fade-split water px {}, max cell tpp \
                 {:.3}; stats {stats:?}",
                case.pose.name,
                (case.look)().surface_reflectivity,
                result.worst,
                result.worst_at,
                result.over,
                result.water,
                result.over_fade,
                result.over_boundary,
                result.worst_unexplained,
                result.fade_split,
                result.max_tpp,
            );
            assert!(stats.cells_evaluated > 0, "the cell pass ran: {stats:?}");
            assert_eq!(u64::from(stats.cells_evaluated), stats.cells_dispatched);
            assert!(stats.reflecting_fragments > 0, "{stats:?}");
            assert!(result.water * 2 > (SIDE * SIDE) as usize, "the pool fills the view");
            if case.exact {
                assert_eq!(result.fade_split, 0, "{}: no cell near its fade", case.pose.name);
                assert!(
                    result.worst <= 2,
                    "{}: cells match per-fragment within 2/255: {} at {:?}",
                    case.pose.name,
                    result.worst,
                    result.worst_at
                );
            } else {
                assert!(result.fade_split > 0, "the oblique pose crosses the fade band");
                assert!(
                    result.worst_unexplained <= 2,
                    "{}: every mismatch past 2/255 sits on a fade-split cell: {result:?}",
                    case.pose.name
                );
            }
            ratio_frames.push(cells);
        }
        let moved = ratio_frames[0]
            .iter()
            .zip(&ratio_frames[1])
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        eprintln!("[FluidCellSpike] gate A {}: the ratio lever moves the frame by {moved}", case.pose.name);
        // Water's Fresnel is small looking down; the mirror shows the ratio whole.
        if (case.look)().surface_reflectivity >= 50.0 {
            assert!(moved > 10, "the ratio lever moves the frame: {moved}");
        }
        renderer.release_level_resources();
    }
}

/// Gate B: one texel per cell. The atlas filled with a per-texel unique
/// pattern (no cell pass), a mirror at zero opacity draws each pixel as its
/// cell's texel alone: every pixel off a cell boundary shows exactly the
/// texel of the cell the CPU reprojects it into, so every fragment of one
/// cell reads one texel and no other cell's.
#[test]
fn every_fragment_of_a_cell_reads_its_one_atlas_texel() {
    let Some(mut renderer) = scene(&full_box(), mirror()) else {
        return;
    };
    set_levers(&mut renderer, true, false, false);
    let layout = renderer
        .full()
        .fluid_surface
        .cell_atlas_layout()
        .expect("installed")
        .clone();
    assert_eq!(layout.faces.len(), 1);
    let face = layout.faces[0];
    let side = (2.0 * POOL_HALF * TEXELS_PER_METRE) as u32;
    assert_eq!(face.extent, [side, side]);
    let colour = |x: u32, y: u32| -> [f32; 3] {
        [
            0.05 + 0.75 * (x % 24) as f32 / 23.0,
            0.05 + 0.75 * (y % 24) as f32 / 23.0,
            0.05 + 0.25 * ((x / 24 + 2 * (y / 24)) % 4) as f32 / 3.0,
        ]
    };
    let quantized = |value: f32| f16_bits_to_f32(f32_to_f16_bits(value));
    let texels: Vec<u8> = (0..layout.height)
        .flat_map(|y| (0..layout.width).map(move |x| (x, y)))
        .flat_map(|(x, y)| {
            let [r, g, b] = colour(x, y);
            [r, g, b, 1.0].map(f32_to_f16_bits)
        })
        .flat_map(|half| half.to_le_bytes())
        .collect();
    renderer.write_fluid_cell_atlas_for_test(&texels);
    renderer.set_fluid_surface_draws(&[draw(mirror())]);
    let pixels = capture(&mut renderer, TOP_DOWN);

    let view_proj = TOP_DOWN.view_proj();
    let origin = IVec2::new(
        (face.atlas_origin & 0xFFFF) as i32,
        (face.atlas_origin >> 16) as i32,
    );
    let (mut checked, mut worst, mut cells) = (0, 0u8, std::collections::HashSet::new());
    for row in 0..SIDE {
        for column in 0..SIDE {
            let Some(point) = pixel_on_water(view_proj, column, row) else {
                continue;
            };
            let (t, boundary) = pixel_texel(point, 0.05);
            if boundary {
                continue;
            }
            let local = t.floor().as_ivec2() - IVec2::new(face.cell_min_x, face.cell_min_y);
            let texel = origin + local;
            let want = colour(texel.x as u32, texel.y as u32).map(|c| srgb(quantized(c)));
            let at = ((row * SIDE + column) * 4) as usize;
            for c in 0..3 {
                worst = worst.max(pixels[at + c].abs_diff(want[c]));
            }
            checked += 1;
            cells.insert(local);
        }
    }
    eprintln!(
        "[FluidCellSpike] gate B: {checked} pixels off cell boundaries over {} cells, worst \
         deviation from their cell's texel {worst}",
        cells.len()
    );
    renderer.release_level_resources();
    assert!(checked * 2 > (SIDE * SIDE) as usize && cells.len() > 50);
    assert!(worst <= 1, "each pixel shows its own cell's texel: off by {worst}");
}

fn srgb(linear: f32) -> u8 {
    let c = linear.clamp(0.0, 1.0);
    let encoded = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}

const LOOK_LEFT: Pose = Pose {
    name: "left",
    camera: Vec3::new(0.0, 2.0, 0.0),
    target: Vec3::new(-6.0, 0.0, 3.0),
};
const LOOK_RIGHT: Pose = Pose {
    name: "right",
    camera: Vec3::new(0.0, 2.0, 0.0),
    target: Vec3::new(6.0, 0.0, 3.0),
};

/// Gate C: no stale cells. Pose A for three frames, then pose B, matches a
/// fresh renderer's pose-B frame exactly, cells on, with and without the
/// frustum cull. Not vacuous: at pose A the cull skips cells pose B reads.
#[test]
fn a_pose_change_reads_no_cell_left_from_the_previous_pose() {
    for frustum_cull in [false, true] {
        let Some(mut moved) = scene(&full_box(), mirror()) else {
            return;
        };
        set_levers(&mut moved, true, frustum_cull, false);
        let (_, stats_a) = capture_with_stats(&mut moved, LOOK_LEFT);
        for _ in 0..2 {
            capture(&mut moved, LOOK_LEFT);
        }
        let after_move = capture(&mut moved, LOOK_RIGHT);

        let Some(mut fresh) = scene(&full_box(), mirror()) else {
            return;
        };
        set_levers(&mut fresh, true, frustum_cull, false);
        let fresh_b = capture(&mut fresh, LOOK_RIGHT);

        let worst = after_move
            .iter()
            .zip(&fresh_b)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        eprintln!(
            "[FluidCellSpike] gate C (frustum_cull {frustum_cull}): pose A evaluated {} of {} \
             cells; pose B after A vs fresh pose B worst deviation {worst}",
            stats_a.cells_evaluated, stats_a.cells_dispatched,
        );
        if frustum_cull {
            assert!(
                u64::from(stats_a.cells_evaluated) < stats_a.cells_dispatched,
                "the cull skips cells at pose A: {stats_a:?}"
            );
        } else {
            assert_eq!(u64::from(stats_a.cells_evaluated), stats_a.cells_dispatched);
        }
        assert_eq!(worst, 0, "pose B after A equals a fresh pose B");
        moved.release_level_resources();
        fresh.release_level_resources();
    }
}

/// Gate D: lever-off identity. With the levers at their default the two
/// pipelines take exactly main's override constants and no lever pipeline
/// is built; a smooth-style frame with cells on is bit-identical to smooth
/// with cells off, and runs no cell pass.
#[test]
fn levers_off_keep_mains_pipelines_and_smooth_ignores_the_cells_lever() {
    assert_eq!(
        fluid_variant_constants(FluidShaderVariant::ReflectionFree),
        [("FLUID_REFLECTIONS", 0.0)]
    );
    assert_eq!(
        fluid_variant_constants(FluidShaderVariant::Reflecting),
        [("FLUID_REFLECTIONS", 1.0)]
    );
    assert!(lever_constants(false, false).is_empty());
    assert_eq!(lever_pipeline_index(false, false), None);
    let source: String = include_str!("fluid_surface.rs")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(source.contains(
        "create_fluid_pipeline(device,&pipeline_layout,&shader,depth_format,variant,\
         &fluid_variant_constants(variant),)"
    ));

    let Some(mut renderer) = scene(&full_box(), mirror()) else {
        return;
    };
    assert_eq!(
        renderer.fluid_reflection_cell_levers(),
        Some(FluidReflectionCellLevers::default())
    );
    assert_eq!(renderer.full().fluid_surface.lever_pipelines_built(), 0);
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    renderer.set_fluid_surface_draws(&[draw(mirror())]);
    let off = capture(&mut renderer, TOP_DOWN);
    set_levers(&mut renderer, true, false, false);
    let (on, stats) = capture_with_stats(&mut renderer, TOP_DOWN);
    eprintln!("[FluidCellSpike] gate D smooth with cells on: stats {stats:?}");
    assert!(!stats.pixel_style);
    assert_eq!(stats.tiles_dispatched, 0, "no cell pass in smooth style");
    assert_eq!(off, on, "smooth is bit-identical with the cells lever on");
    renderer.release_level_resources();
}
