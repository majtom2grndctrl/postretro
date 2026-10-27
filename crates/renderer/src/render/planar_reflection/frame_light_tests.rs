//! Adapter-backed admission proof: a light only the mirror reaches shadows the reflection and
//! nothing else (AC 23).
//! See: context/lib/rendering_pipeline.md §7.3a (Pool rank)

use super::frame_test_fixtures::*;
use super::frame_tests::{MOVER, mover_geometry};
use crate::render::fluid_reflection_frame_test_fixtures::{
    self as fixture, FRAME, View, WorldQuad, pixels, rgb,
};
use crate::render::fluid_surface_frame_test::look_draw;
use crate::render::{ClearColor, Renderer, ShSampleRegionSets};
use glam::{Mat4, UVec2, Vec3};
use postretro_level_loader::{
    CellData, FalloffModel, LightType, MapLight, ShDrainBatch, ShadowType,
};
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;
use postretro_render_data::influence::LightInfluence;
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};

/// The spot: above the pool and right of the camera, aimed at the back wall
/// past a small occluder, so the occluder's shadow falls on the wall left of
/// where the camera sees the occluder.
const SPOT_AT: Vec3 = Vec3::new(2.5, 2.2, 4.5);
const SPOT_RANGE: f32 = 10.0;

/// Main-view light reach far from the spot: its influence meets no
/// main-view cell, so with a mirror reach it ranks mirror-only.
const FAR_CELL: (Vec3, Vec3) = (Vec3::splat(50.0), Vec3::splat(51.0));
/// The mirror's light reach: every cell of the scene.
const MIRROR_CELLS: (Vec3, Vec3) = (Vec3::splat(-10.0), Vec3::splat(10.0));

fn spot() -> MapLight {
    MapLight {
        origin: SPOT_AT.as_dvec3().to_array(),
        light_type: LightType::Spot,
        intensity: 4.0,
        color: [1.0, 1.0, 1.0],
        falloff_model: FalloffModel::Linear,
        falloff_range: SPOT_RANGE,
        cone_angle_inner: 0.45,
        cone_angle_outer: 0.6,
        cone_direction: [-0.371_391, 0.0, 0.928_477],
        is_dynamic: true,
        casts_entity_shadows: false,
        animated_slot: None,
        tags: vec![],
        cell_index: 0,
        shadow_type: ShadowType::StaticLightMap,
    }
}

/// The octahedral encoding of the world normal −z, toward the spot: the lit
/// side of every quad here (the fixtures' default encodes +z).
const TOWARD_SPOT: [u16; 2] = [65535, 65535];

/// `quads` as one cell's world, under one BVH leaf, lit by `lights`, every
/// quad facing −z; each wound both ways, as [`fixture::install_world_quads`]
/// winds them.
fn install_lit_quads(renderer: &mut Renderer, quads: &[WorldQuad], lights: &[MapLight]) {
    use postretro_render_data::geometry::{
        BVH_NODE_FLAG_LEAF, BvhLeaf, BvhNode, BvhTree, WorldVertex,
    };
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let (mut low, mut high) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for quad in quads {
        let base = vertices.len() as u32;
        for corner in quad.corners {
            low = low.min(corner);
            high = high.max(corner);
            vertices.push(WorldVertex {
                position: corner.to_array(),
                base_uv: quad.base_uv,
                normal_oct: TOWARD_SPOT,
                tangent_packed: [0; 2],
                lightmap_uv: [0; 2],
                lightmap_block: 0,
                animated_block: 0,
            });
        }
        indices.extend([0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2].map(|index| base + index));
    }
    let (aabb_min, aabb_max) = ((low - 0.1).to_array(), (high + 0.1).to_array());
    let bvh = BvhTree {
        nodes: vec![
            BvhNode {
                aabb_min,
                skip_index: 2,
                aabb_max,
                left_child_or_leaf_index: 0,
                flags: 0,
            },
            BvhNode {
                aabb_min,
                skip_index: 2,
                aabb_max,
                left_child_or_leaf_index: 0,
                flags: BVH_NODE_FLAG_LEAF,
            },
        ],
        leaves: vec![BvhLeaf {
            aabb_min,
            aabb_max,
            material_bucket_id: 0,
            index_offset: 0,
            index_count: indices.len() as u32,
            cell_id: 0,
            chunk_range_start: 0,
            chunk_range_count: 0,
        }],
        root_node_index: 0,
    };
    let cells = [CellData {
        bounds_min: Vec3::from(aabb_min),
        bounds_max: Vec3::from(aabb_max),
        face_start: 0,
        face_count: quads.len() as u32,
        portal_ref_start: 0,
        portal_ref_count: 0,
        is_solid: false,
        is_exterior: false,
        is_drawable: true,
    }];
    let influences: Vec<LightInfluence> = lights
        .iter()
        .map(|light| LightInfluence {
            center: Vec3::new(
                light.origin[0] as f32,
                light.origin[1] as f32,
                light.origin[2] as f32,
            ),
            radius: light.falloff_range,
        })
        .collect();
    renderer.install_level_geometry(
        &crate::render::LevelGeometry {
            vertices: &vertices,
            vertex_spec_owners: &[],
            indices: &indices,
            bvh: &bvh,
            lights,
            light_influences: &influences,
            sh_volume: None,
            sh_storage: crate::render::LevelGeometryShStorage::Legacy,
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
            entity_shadow_lights: &[],
            shadowmask_atlas: None,
            sdf_atlas: None,
            lightmap_mode: postretro_level_loader::LightmapMode::Shadowed,
            cell_draw_index: None,
            kinematic_geometry: None,
            cells: &cells,
            texture_materials: &[],
        },
        Default::default(),
    );
}

/// The receiver: the magenta mover panel, doubled to 2.8 m × 3.6 m, standing
/// just before the back wall. A mover takes no baked direct light here, so
/// the spot's light and shadow on it read against the ambient floor alone
/// (world faces read the placeholder lightmap's full white).
fn panel() -> Mat4 {
    Mat4::from_translation(Vec3::new(0.3, 0.3, 7.9)) * Mat4::from_scale(Vec3::splat(2.0))
}

/// The scene: the base pool before a black back wall, the magenta panel the
/// spot lights, and a small black world occluder casting its shadow on the
/// panel, at very high with the planar layer at full scale and the base
/// pool's plane selected.
fn lit_renderer() -> Option<(Renderer, View)> {
    let mut renderer = fixture::ssr_renderer()?;
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    install_lit_quads(
        &mut renderer,
        &[
            fixture::upright(
                [-6.0, 6.0],
                [0.0, 6.0],
                fixture::POOL_Z[1],
                fixture::BLACK_UV,
            ),
            fixture::upright([0.2, 1.2], [1.6, 2.4], 6.5, fixture::BLACK_UV),
        ],
        &[spot()],
    );
    let device = renderer.device.clone();
    renderer
        .full_mut()
        .kinematic_brush
        .install_geometry(&device, Some(&mover_geometry()), 1);
    let movers = [crate::render::KinematicMoverInstance {
        mover_id: MOVER,
        transform: panel(),
    }];
    renderer.set_kinematic_mover_draws(&movers, &movers);
    renderer.set_ambient_floor(0.3);
    // Bloom is a post-pass over the main view: it would spread the lit panel
    // across the plane's edge and blur the off-plane comparison.
    renderer.full_mut().bloom.set_enabled(false);
    renderer.install_fluid_surfaces(
        &[fixture::base_pool()],
        Some(&fixture::green_probe(&[(0, 0)])),
    );
    renderer.set_fluid_surface_draws(&[look_draw(0, fixture::mirror())]);
    renderer.set_reflection_quality(FluidReflectionQuality::VeryHigh);
    renderer.set_planar_preset_override(Some(full_scale()));
    let view = fixture::base_view();
    select(&mut renderer, view, 0.0);
    Some((renderer, view))
}

/// One captured frame through `view` with the main view's light reach
/// `main_reach` (empty: every light reaches the main view) and the mirror's
/// `mirror_reach`, inside a validation error scope.
fn capture_lit(
    renderer: &mut Renderer,
    view: View,
    main_reach: &[(Vec3, Vec3)],
    mirror_reach: &[(Vec3, Vec3)],
) -> Vec<u8> {
    renderer.set_planar_mirror_light_reach(mirror_reach);
    let scope = renderer
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    let all = VisibleCells::DrawAll;
    let pixels = renderer
        .capture_frame_indirect(
            CameraCullVisibility {
                cells: &all,
                path: VisibilityPath::EmptyWorldFallback,
            },
            &[],
            main_reach,
            &[],
            ShSampleRegionSets {
                visible_cells: &all,
                fog_cells: &[],
                movers: &[],
            },
            None,
            view.view_proj,
            view.eye,
            &[],
            &[],
            1.0,
            ClearColor {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            },
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

/// Pixels near the selected plane's water, outset by 0.3 m so the layer's
/// filtered edge stays inside: the only pixels mirror lighting may change.
fn near_the_plane(view: View, pixel: UVec2) -> bool {
    on_extent(view, pixel, 0.0, fixture::POOL_X, -0.3).is_some()
}

/// The spot's shadow slot field in light record `index` of `records`.
fn spot_slot(records: &[u8], index: usize) -> u32 {
    let at =
        index * postretro_lighting::GPU_LIGHT_SIZE + postretro_lighting::SHADOW_SLOT_BYTE_OFFSET;
    u32::from_ne_bytes(records[at..at + 4].try_into().expect("a word"))
}

// AC 23. A spot the mirror alone reaches holds a shadow slot: every pixel off
// the selected plane matches the same frame with mirror lighting excluded,
// where the spot holds none, and the reflection shows the spot's shadow
// exactly as it does when the main view reaches the spot. The main view
// truly sees the spot here (the main reach is set apart from it on purpose),
// so a main view that read the mirror's slot would show the shadow on the
// panel: the main-view control frame proves the shadow is on screen.
#[test]
fn planar_gpu_mirror_only_light_shows_only_in_the_reflection() {
    let Some((mut renderer, view)) = lit_renderer() else {
        return;
    };

    let mirror_only = capture_lit(&mut renderer, view, &[FAR_CELL], &[MIRROR_CELLS]);
    let full = renderer.full();
    assert_eq!(
        full.spot_shadow_pool.slot_assignment,
        vec![0],
        "the mirror-only spot takes the free slot"
    );
    assert!(
        full.mirror_lighting.active(),
        "the mirror reads its own records"
    );
    assert!(full.mirror_lighting.is_mirror_only(0));
    assert_eq!(
        spot_slot(&full.last_lights_upload, 0),
        postretro_lighting::NO_SHADOW_SLOT,
        "the main view's records carry no mirror-only slot"
    );
    assert_eq!(spot_slot(&full.mirror_lighting.scratch, 0), 0);
    let occupancy = renderer.admission_occupancy().spot;
    assert_eq!((occupancy.main, occupancy.mirror_only), (0, 1));

    let excluded = capture_lit(&mut renderer, view, &[FAR_CELL], &[]);
    assert_eq!(
        renderer.full().spot_shadow_pool.slot_assignment,
        vec![postretro_lighting::NO_SHADOW_SLOT],
        "with mirror lighting excluded the spot reaches no view"
    );
    assert!(!renderer.full().mirror_lighting.active());

    let main_view = capture_lit(&mut renderer, view, &[], &[MIRROR_CELLS]);
    assert_eq!(renderer.full().spot_shadow_pool.slot_assignment, vec![0]);
    assert!(
        !renderer.full().mirror_lighting.active(),
        "a main-view light needs no mirror records"
    );
    assert_eq!(renderer.admission_occupancy().spot.main, 1);

    let (mut off_plane, mut on_plane) = (0, 0);
    let (mut main_shadowed, mut reflected_shadow) = (0, 0);
    for pixel in pixels() {
        if near_the_plane(view, pixel) {
            on_plane += 1;
            assert_eq!(
                rgb(&mirror_only, pixel),
                rgb(&main_view, pixel),
                "pixel {pixel}: the reflection shows the spot as a main-view light's"
            );
            if rgb(&mirror_only, pixel) != rgb(&excluded, pixel) {
                reflected_shadow += 1;
            }
        } else {
            off_plane += 1;
            assert_eq!(
                rgb(&mirror_only, pixel),
                rgb(&excluded, pixel),
                "pixel {pixel}: off the plane the mirror-only spot holds no slot"
            );
            if rgb(&main_view, pixel) != rgb(&excluded, pixel) {
                main_shadowed += 1;
            }
        }
    }
    eprintln!(
        "[PlanarProof] mirror-only spot: {off_plane} off-plane pixels match the excluded frame; \
         the main-view control shadows {main_shadowed} of them; {reflected_shadow} of \
         {on_plane} plane pixels show the shadow in the reflection"
    );
    assert!(
        off_plane > 200 && on_plane > 200,
        "{off_plane} / {on_plane} of {}",
        FRAME * FRAME
    );
    assert!(
        main_shadowed >= 8,
        "control: a main view reading the slot shows the shadow ({main_shadowed} pixels)"
    );
    assert!(
        reflected_shadow >= 8,
        "the spot's shadow shows in the reflection ({reflected_shadow} pixels)"
    );
    renderer.release_level_resources();
}
