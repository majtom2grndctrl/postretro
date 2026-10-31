// Per-load renderer resource sets: release drops one set and creates nothing,
// a reinstall draws like the first install, and keyed resident sets build,
// publish and release without touching each other.
// See: context/plans/in-progress/level-load-pipeline/index.md (Per-load resources)

use super::renderer_textures::tests::{
    chain, install_one_triangle_with_sh, with_one_triangle, write_prm,
};
use super::*;
use crate::render::fluid_baked_frame_test_fixtures::{direct_section, sh_section};
use crate::render::gpu_test_harness::offscreen_renderer_or_skip;
use crate::render::{ClearColor, ShSampleRegionSets};
use glam::{Mat4, Vec3};
use postretro_level_format::prm::PrmFormat;
use postretro_level_loader::ShDrainBatch;
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};

const CLEAR: ClearColor = ClearColor {
    r: 0.05,
    g: 0.1,
    b: 0.2,
    a: 1.0,
};

/// Textures from one baked sidecar, then the one-triangle level with an SH
/// volume, so world textures, geometry and the SH family all bind real data.
fn install_fixture(renderer: &mut Renderer, root: &Path) {
    let key = [0x42u8; 32];
    write_prm(
        root,
        key,
        [
            Some(chain(PrmFormat::Rgba8UnormSrgb, 4, 4, 0x5A)),
            None,
            None,
            None,
        ],
    );
    let names = vec!["baked".to_string()];
    let keys = TextureCacheKeysSection { keys: vec![key] };
    let materials = vec![Material::Default];
    renderer.install_textures(&names, &keys, root, &materials);
    let sh = sh_section([-1.0; 3], [1.0; 3], [2, 2, 2], |_| Vec3::splat(0.25));
    let direct = direct_section(&sh, Vec3::splat(0.5));
    install_one_triangle_with_sh(renderer, &materials, Some(&sh), Some(&direct));
}

fn capture(renderer: &mut Renderer, render_world: bool) -> Vec<u8> {
    let cells = VisibleCells::Culled(vec![0]);
    renderer
        .capture_frame_indirect(
            CameraCullVisibility {
                cells: &cells,
                path: VisibilityPath::PrlPortal { walk_reach: 1 },
            },
            &[],
            &[],
            &[],
            ShSampleRegionSets {
                visible_cells: &cells,
                fog_cells: &[],
                movers: &[],
            },
            Some(0),
            Mat4::IDENTITY,
            Vec3::new(0.0, 0.0, 1.0),
            &[],
            &[],
            1.0,
            CLEAR,
            render_world,
            ShDrainBatch::default(),
        )
        .expect("capture drains")
        .frame
        .expect("capture records")
}

fn world_less(renderer: &mut Renderer, font: &mut postretro_ui::text::FontSystem) -> Vec<u8> {
    renderer
        .capture_world_less_frame(font, CLEAR, 1.0)
        .expect("world-less capture records")
}

fn hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

#[test]
fn release_creates_no_gpu_object_and_drops_the_level_set() {
    let test = "release_creates_no_gpu_object_and_drops_the_level_set";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let mut font = postretro_ui::text::build_font_system();
    let dir = tempfile::tempdir().unwrap();
    install_fixture(&mut renderer, dir.path());
    assert!(
        renderer.has_level_resources(),
        "the install publishes a set"
    );
    capture(&mut renderer, true);

    let before = gpu_creation_counts();
    renderer.release_level_resources();
    let released = gpu_creation_counts().since(before);
    assert_eq!(
        released.total(),
        0,
        "release creates no GPU object: {released:?}"
    );
    assert!(!renderer.has_level_resources(), "release drops the set");
    assert!(
        renderer.full().pending_textures.is_none(),
        "release keeps no staged materials"
    );

    let scope = renderer
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    let world = capture(&mut renderer, true);
    let empty = capture(&mut renderer, false);
    let ui = world_less(&mut renderer, &mut font);
    let error = pollster::block_on(scope.pop());
    assert!(
        error.is_none(),
        "a world-less frame after release raised a validation error: {error:?}"
    );
    assert_eq!(
        world, empty,
        "a world frame with no set draws as world-less"
    );
    assert_eq!(
        hash(&world),
        hash(&ui),
        "and as the windowed world-less frame"
    );
}

#[test]
fn install_after_release_draws_like_first_install() {
    let test = "install_after_release_draws_like_first_install";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    install_fixture(&mut renderer, dir.path());
    let first = capture(&mut renderer, true);
    renderer.release_level_resources();
    let released = capture(&mut renderer, true);
    assert_ne!(first, released, "the fixture draws over the clear");

    install_fixture(&mut renderer, dir.path());
    let second = capture(&mut renderer, true);
    assert_eq!(
        hash(&second),
        hash(&first),
        "the reinstalled level draws as the first install did"
    );
    eprintln!(
        "[LevelSetProof] install {:016x}, released {:016x}, reinstall {:016x}",
        hash(&first),
        hash(&released),
        hash(&second)
    );
}

/// With no set, every frame kind draws what a never-installed renderer's
/// world-less frame draws, before an install and after a release.
#[test]
fn world_less_frames_match_a_fresh_renderer_before_install_and_after_release() {
    let test = "world_less_frames_match_a_fresh_renderer_before_install_and_after_release";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let mut font = postretro_ui::text::build_font_system();
    let dir = tempfile::tempdir().unwrap();
    let fresh = world_less(&mut renderer, &mut font);
    assert_eq!(capture(&mut renderer, true), fresh, "fresh world frame");
    assert_eq!(capture(&mut renderer, false), fresh, "fresh empty frame");
    install_fixture(&mut renderer, dir.path());
    renderer.release_level_resources();
    assert_eq!(capture(&mut renderer, true), fresh, "released world frame");
    assert_eq!(capture(&mut renderer, false), fresh, "released empty frame");
    assert_eq!(
        world_less(&mut renderer, &mut font),
        fresh,
        "released UI frame"
    );
}

/// A Surface Depth change with no set and no staged materials stages
/// nothing: there is no material to rewrite.
#[test]
fn surface_depth_change_with_no_level_set_stages_no_material_write() {
    use postretro_render_cpu::surface_depth::SurfaceDepthQuality;
    let test = "surface_depth_change_with_no_level_set_stages_no_material_write";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let quality = |renderer: &Renderer| match renderer.surface_depth_quality() {
        SurfaceDepthQuality::Off => SurfaceDepthQuality::On,
        _ => SurfaceDepthQuality::Off,
    };
    let before = renderer.queue.counts();
    let next = quality(&renderer);
    renderer.set_surface_depth_quality(next);
    assert_eq!(renderer.queue.counts().writes, before.writes);
}

// --- Keyed resident sets (task 5b) ---

/// Stage one baked texture of `fill`, then build the one-triangle level with
/// an SH volume and `lights` into a resident, unpublished set.
fn build_fixture_with(
    renderer: &mut Renderer,
    root: &Path,
    fill: u8,
    lights: &[postretro_level_loader::MapLight],
) -> LevelSetId {
    let key = [fill; 32];
    write_prm(
        root,
        key,
        [
            Some(chain(PrmFormat::Rgba8UnormSrgb, 4, 4, fill)),
            None,
            None,
            None,
        ],
    );
    let names = vec![format!("baked-{fill:02x}")];
    let keys = TextureCacheKeysSection { keys: vec![key] };
    let materials = vec![Material::Default];
    renderer.install_textures(&names, &keys, root, &materials);
    let sh = sh_section([-1.0; 3], [1.0; 3], [2, 2, 2], |_| Vec3::splat(0.25));
    let direct = direct_section(&sh, Vec3::splat(0.5));
    with_one_triangle(&materials, Some(&sh), Some(&direct), lights, |geometry| {
        renderer
            .build_level_set(geometry, Default::default())
            .expect("level set builds")
    })
}

fn build_fixture(renderer: &mut Renderer, root: &Path, fill: u8) -> LevelSetId {
    build_fixture_with(renderer, root, fill, &[])
}

/// A dynamic spot light above the triangle, aimed down at it: a dynamic
/// depth-cache source, drawn through the spot shadow pool.
fn spot_light() -> postretro_level_loader::MapLight {
    use postretro_level_loader::{FalloffModel, LightType, MapLight, ShadowType};
    MapLight {
        origin: [0.0, 0.0, 0.6],
        light_type: LightType::Spot,
        intensity: 10.0,
        color: [1.0, 0.5, 0.25],
        falloff_model: FalloffModel::Linear,
        falloff_range: 10.0,
        cone_angle_inner: 0.6,
        cone_angle_outer: 1.2,
        cone_direction: [0.0, 0.0, -1.0],
        is_dynamic: true,
        casts_entity_shadows: false,
        animated_slot: None,
        tags: vec![],
        cell_index: 0,
        shadow_type: ShadowType::StaticLightMap,
    }
}

fn publish(renderer: &mut Renderer, id: LevelSetId) -> Option<LevelSetId> {
    renderer.publish_level_set(id, PreviousLevelSet::KeepResident)
}

fn occupies_a_spot_slot(renderer: &Renderer) -> bool {
    renderer
        .full()
        .spot_shadow_pool
        .slot_cone_matrices
        .iter()
        .any(Option::is_some)
}

#[test]
fn release_drops_one_load_set_only() {
    let test = "release_drops_one_load_set_only";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let first = build_fixture(&mut renderer, dir.path(), 0x5A);
    assert_eq!(publish(&mut renderer, first), None);
    let first_draw = capture(&mut renderer, true);

    // A second set resident beside the active one; releasing it creates
    // nothing and leaves the active draw as it was.
    let second = build_fixture(&mut renderer, dir.path(), 0xA5);
    assert_eq!(renderer.resident_level_sets(), vec![second]);
    let before = gpu_creation_counts();
    assert!(renderer.release_level_set(second));
    let released = gpu_creation_counts().since(before);
    assert_eq!(released.total(), 0, "release creates nothing: {released:?}");
    assert!(renderer.resident_level_sets().is_empty());
    assert_eq!(renderer.active_level_set(), Some(first));
    assert_eq!(hash(&capture(&mut renderer, true)), hash(&first_draw));

    // Publish another set, keeping the first resident, then release the
    // first: the new active draw is unchanged by it.
    let third = build_fixture(&mut renderer, dir.path(), 0xA5);
    assert_eq!(publish(&mut renderer, third), Some(first));
    let third_draw = capture(&mut renderer, true);
    assert_ne!(hash(&third_draw), hash(&first_draw), "the sets draw apart");
    let before = gpu_creation_counts();
    assert!(renderer.release_level_set(first));
    let released = gpu_creation_counts().since(before);
    assert_eq!(released.total(), 0, "release creates nothing: {released:?}");
    assert_eq!(renderer.active_level_set(), Some(third));
    assert!(renderer.resident_level_sets().is_empty());
    assert_eq!(hash(&capture(&mut renderer, true)), hash(&third_draw));
    assert!(!renderer.release_level_set(first), "a set releases once");
}

#[test]
fn same_map_loads_hold_separate_sets() {
    let test = "same_map_loads_hold_separate_sets";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    for release_first in [false, true] {
        let a = build_fixture(&mut renderer, dir.path(), 0x5A);
        let b = build_fixture(&mut renderer, dir.path(), 0x5A);
        assert_ne!(a, b, "every build mints its own key");
        {
            let (sa, sb) = (
                renderer.level_set(a).unwrap(),
                renderer.level_set(b).unwrap(),
            );
            assert!(sa.vertex_buffer != sb.vertex_buffer);
            assert!(sa.index_buffer != sb.index_buffer);
            assert!(sa.lights_buffer != sb.lights_buffer);
            assert!(sa.lighting_bind_group != sb.lighting_bind_group);
            assert!(sa.gpu_textures[0].bind_group != sb.gpu_textures[0].bind_group);
            assert!(sa.extent.fog.bind_group != sb.extent.fog.bind_group);
            assert!(sa.extent.spot_shadow != sb.extent.spot_shadow);
        }
        // Publish one and release the other: the published draw holds.
        let (keep, drop) = if release_first { (b, a) } else { (a, b) };
        publish(&mut renderer, keep);
        let drawn = capture(&mut renderer, true);
        let before = gpu_creation_counts();
        assert!(renderer.release_level_set(drop));
        assert_eq!(gpu_creation_counts().since(before).total(), 0);
        assert_eq!(hash(&capture(&mut renderer, true)), hash(&drawn));
        renderer.release_level_resources();
        assert!(!renderer.has_level_resources());
    }
}

#[test]
fn spot_shadow_pool_shared_by_tier() {
    use std::sync::Arc;
    let test = "spot_shadow_pool_shared_by_tier";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let lights = [spot_light()];
    let tier = |renderer: &Renderer, id| renderer.level_set(id).unwrap().shadow.tier.clone();

    // Same tier: one pool, which outlives the first release.
    let a = build_fixture_with(&mut renderer, dir.path(), 0x5A, &lights);
    let b = build_fixture_with(&mut renderer, dir.path(), 0x5A, &lights);
    let pool = tier(&renderer, a);
    assert!(Arc::ptr_eq(&pool, &tier(&renderer, b)), "same tier shares");
    let weak = Arc::downgrade(&pool);
    drop(pool);
    assert!(renderer.release_level_set(a));
    let survivor = weak.upgrade().expect("the pool outlives the first release");
    assert!(Arc::ptr_eq(&survivor, &tier(&renderer, b)));
    let c = build_fixture_with(&mut renderer, dir.path(), 0x5A, &lights);
    assert!(
        Arc::ptr_eq(&survivor, &tier(&renderer, c)),
        "a later build reuses it"
    );
    drop(survivor);
    assert!(renderer.release_level_set(c));

    // Different tier: the resident level keeps its own pool and its spot-lit
    // draw is unchanged by the second build.
    publish(&mut renderer, b);
    let resident_draw = capture(&mut renderer, true);
    assert!(occupies_a_spot_slot(&renderer), "the draw is spot-lit");
    let resolution = renderer.level_set(b).unwrap().shadow.tier.resolution();
    renderer.set_spot_shadow_map_resolution(resolution / 2);
    let d = build_fixture_with(&mut renderer, dir.path(), 0x5A, &lights);
    let (b_tier, d_tier) = (tier(&renderer, b), tier(&renderer, d));
    assert!(
        !Arc::ptr_eq(&b_tier, &d_tier),
        "a new tier builds its own pool"
    );
    assert_eq!(d_tier.resolution(), resolution / 2);
    assert!(Arc::ptr_eq(
        &renderer.full().spot_shadow_pool.textures,
        &b_tier.spot
    ));
    assert_eq!(hash(&capture(&mut renderer, true)), hash(&resident_draw));
    // Publishing the other tier's set samples its own pool.
    publish(&mut renderer, d);
    capture(&mut renderer, true);
    assert!(Arc::ptr_eq(
        &renderer.full().spot_shadow_pool.textures,
        &d_tier.spot
    ));
    assert!(occupies_a_spot_slot(&renderer));
}

/// Building a second set beside the active one leaves the active draw
/// pixel-identical and its depth caches warm. The fixture's dynamic spot is a
/// dynamic world-depth cache source (no fixture here carries a promoted
/// source: that needs a baked selected-static or section-45 light).
#[test]
fn building_beside_the_active_set_changes_nothing_it_draws() {
    let test = "building_beside_the_active_set_changes_nothing_it_draws";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let lights = [spot_light()];
    for (label, beside) in [("plain", &[][..]), ("spot-lit", &lights[..])] {
        let active = build_fixture_with(&mut renderer, dir.path(), 0x5A, &lights);
        publish(&mut renderer, active);
        // Warm the dynamic cache: the first frame renders world depth, the
        // second reuses it.
        capture(&mut renderer, true);
        let warm = capture(&mut renderer, true);
        assert!(occupies_a_spot_slot(&renderer), "{label}: spot-lit");
        let warm_frame = renderer.full().dynamic_depth_cache_diagnostics.frame;
        assert!(
            warm_frame.world_pass_skips > 0,
            "{label}: the second frame reuses cached world depth: {warm_frame:?}"
        );

        let beside_id = build_fixture_with(&mut renderer, dir.path(), 0xA5, beside);
        assert_eq!(renderer.active_level_set(), Some(active), "{label}");
        assert_eq!(
            hash(&capture(&mut renderer, true)),
            hash(&warm),
            "{label}: the active draw is unchanged by the build"
        );
        assert_eq!(
            renderer.full().dynamic_depth_cache_diagnostics.frame,
            warm_frame,
            "{label}: the build reset no cache"
        );
        assert!(renderer.release_level_set(beside_id));
        renderer.release_level_resources();
    }
}

#[test]
fn publish_rebuilds_extent_bound_groups_once_on_mismatch() {
    use postretro_render_cpu::render_extent::Extent;
    let test = "publish_rebuilds_extent_bound_groups_once_on_mismatch";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let e1 = renderer.scene_extent();
    let active = build_fixture(&mut renderer, dir.path(), 0x5A);
    publish(&mut renderer, active);
    let resident = build_fixture(&mut renderer, dir.path(), 0xA5);

    // Resize to E2: only the active set's groups follow.
    renderer.record_surface_size(24, 20);
    renderer.commit_extents();
    let e2 = renderer.scene_extent();
    assert_ne!(e1, e2);
    assert_eq!(renderer.extent_rebuilds(), 0, "a resize is not a publish");
    assert_eq!(renderer.level().extent.extent, e2);
    assert_eq!(renderer.level_set(resident).unwrap().extent.extent, e1);

    let before = renderer.extent_rebuilds();
    publish(&mut renderer, resident);
    assert_eq!(
        renderer.extent_rebuilds(),
        before + 1,
        "one counted rebuild"
    );
    let level = renderer.level();
    assert_eq!(level.extent.extent, e2);
    assert_eq!(
        level.extent.fog.scatter_dims(),
        super::fog_pass::scatter_dims_for(e2.width, e2.height, level.extent.fog.pixel_scale())
    );
    // The first frame binds E2 views: it records cleanly and draws as a set
    // built at E2 does.
    let scope = renderer
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    let rebuilt = capture(&mut renderer, true);
    assert!(pollster::block_on(scope.pop()).is_none());
    let fresh = build_fixture(&mut renderer, dir.path(), 0xA5);
    let before = renderer.extent_rebuilds();
    publish(&mut renderer, fresh);
    assert_eq!(
        renderer.extent_rebuilds(),
        before,
        "a matching extent counts 0"
    );
    assert_eq!(renderer.level().extent.extent, e2);
    assert_eq!(hash(&capture(&mut renderer, true)), hash(&rebuilt));
    assert_eq!(Extent::new(24, 20), e2);
}

#[test]
fn sampler_maps_merge_at_publish() {
    let test = "sampler_maps_merge_at_publish";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mip_counts = |renderer: &Renderer| {
        let mut counts: Vec<u32> = renderer.full().samplers.aniso.keys().copied().collect();
        counts.sort_unstable();
        counts
    };
    let boot = mip_counts(&renderer);
    let id = build_fixture(&mut renderer, dir.path(), 0x5A);
    let mip_count = renderer.level_set(id).unwrap().loaded_textures[0].mip_count;
    assert!(
        !boot.contains(&mip_count),
        "the fixture needs a new mip count"
    );
    assert_eq!(
        mip_counts(&renderer),
        boot,
        "a build writes no renderer map"
    );
    assert!(
        renderer
            .level_set(id)
            .unwrap()
            .samplers
            .aniso
            .contains_key(&mip_count),
        "the build holds its new sampler"
    );
    publish(&mut renderer, id);
    assert!(
        mip_counts(&renderer).contains(&mip_count),
        "publish merges it"
    );
    assert!(renderer.level().samplers.aniso.is_empty());
    // A second build at that mip count finds it in the renderer's map.
    let again = build_fixture(&mut renderer, dir.path(), 0xA5);
    assert!(renderer.level_set(again).unwrap().samplers.aniso.is_empty());
}

/// The fluid reflection target and its group 5 belong to their set: a
/// publish hands the replaced set's back to it and lends the published set's
/// to the pass, building nothing, and the set draws as before when it
/// returns.
#[test]
fn fluid_reflection_targets_follow_their_set_across_publishes() {
    use crate::render::fluid_reflection_frame_test_fixtures::{
        base_view, capture_checked, install_base_level, ssr_renderer,
    };
    use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
    let Some(mut renderer) = ssr_renderer() else {
        return;
    };
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    install_base_level(&mut renderer);
    let fluid_set = renderer.active_level_set().expect("published");
    let target = renderer.full().fluid_surface.reflection_target_extent();
    assert!(target.is_some(), "the base level holds a reflection target");
    let fluid_draw = capture_checked(&mut renderer, base_view());
    let builds = renderer.full().fluid_surface.reflection_target_builds();

    let other = with_one_triangle(&[Material::Default], None, None, &[], |geometry| {
        renderer
            .build_level_set(geometry, Default::default())
            .expect("level set builds")
    });
    let before = gpu_creation_counts();
    assert_eq!(publish(&mut renderer, other), Some(fluid_set));
    assert_eq!(gpu_creation_counts().since(before).bind_groups, 0);
    assert_eq!(
        renderer.full().fluid_surface.reflection_target_extent(),
        None
    );
    assert!(
        renderer
            .level_set(fluid_set)
            .unwrap()
            .fluid_targets
            .has_reflection_target(),
        "the resident set keeps its target"
    );

    let before = gpu_creation_counts();
    assert_eq!(publish(&mut renderer, fluid_set), Some(other));
    let created = gpu_creation_counts().since(before);
    assert_eq!(
        created.total(),
        0,
        "the hand-back creates nothing: {created:?}"
    );
    assert_eq!(
        renderer.full().fluid_surface.reflection_target_extent(),
        target
    );
    assert_eq!(
        renderer.full().fluid_surface.reflection_target_builds(),
        builds
    );
    assert_eq!(
        hash(&capture_checked(&mut renderer, base_view())),
        hash(&fluid_draw),
        "the set draws its reflections as before"
    );
}
