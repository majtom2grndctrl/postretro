//! Adapter-backed proofs of the glossy floor layer's source half: the
//! highlight-free colour forward, the mover pass and the skinned-mesh pass
//! write at the floor layer's tiers, the scene-colour chain the floor march
//! will read, and both targets' lifecycle.
//! See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors)

use glam::{Mat4, UVec2, Vec3};
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_TILE_DIMENSION, ReflectionProbeWorldBinding,
    reflection_probe_full_colour_mip_count, reflection_probe_zero_distances,
};
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::frame_uniforms::SpecularStyle;
use postretro_render_cpu::glossy_floor_blur::GlossyFloorTermBlur;

use super::fluid_reflection_frame_test_fixtures::{FRAME, View, centre, pixels};
use super::fluid_reflection_sums_frame_test_fixtures::frame_at;
use super::fluid_reflection_sums_frame_test_fixtures::take;
use super::ghost_depth_frame_test::install_skinned_model;
use super::glossy_floor::ColourOutputs;
use super::glossy_floor_frame_test_fixtures::{
    FloorFrame, FloorTargets, GLOSSY_BROAD_TINTED, GLOSSY_BROAD_UNTINTED, HIGHLIGHT, ROOM_X,
    ROOM_Z, SPECULAR_WALL, WALL_HEIGHT, box_mean, floor_frame, install_faces, install_room,
    read_floor_layer, read_floor_targets, read_scene_colour, room_faces, room_view, store_slack,
    with_forward_highlight, with_mover_highlight,
};
use super::glossy_overlay_frame_test_fixtures::{
    FloorMaterial, floor_probe, overlays, renderer, set_bucket_material, with_faces, with_variant,
};
use super::glossy_overlay_gate_frame_fixtures::{
    at, install_mover, mover_of, read_guard_marks, set_mover, upright_mover,
};
use super::glossy_overlay_guard_frame_test::{CHARACTER, character_at, character_quads};
use super::glossy_overlay_room_frame_test_fixtures::ray_through;
use super::glossy_overlay_weights_frame_test::{point_light, set_light};
use super::gpu_create::{GpuCreationCounts, gpu_creation_counts};
use super::{
    GLOSSY_FLOOR_LAYER, GLOSSY_FLOOR_LAYER_DEPTH, GLOSSY_HIGHLIGHT_FREE, GLOSSY_SCENE_COLOUR_CHAIN,
    GLOSSY_STATIC_DIFFUSE, Renderer,
};
use FluidReflectionQuality::{High, Low, Medium, Off, Ultra, VeryHigh};

/// Where a pixel's centre ray first meets the room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Surface {
    Floor,
    GlossyWall,
    SpecularWall,
}

/// The room surface `pixel` shows, at least 5 cm inside it, or `None`.
fn surface_at(view: View, pixel: UVec2) -> Option<Surface> {
    let (origin, direction) = ray_through(view, centre(pixel));
    let margin = 0.05;
    let wall_t = (ROOM_Z[1] - origin.z) / direction.z;
    let wall = origin + direction * wall_t;
    if wall_t > 0.0 && wall.y > margin && wall.y < WALL_HEIGHT - margin {
        if wall.x > margin && wall.x < ROOM_X[1] - margin {
            return Some(Surface::GlossyWall);
        }
        if wall.x < -margin && wall.x > ROOM_X[0] + margin {
            return Some(Surface::SpecularWall);
        }
    }
    let floor_t = -origin.y / direction.y;
    let floor = origin + direction * floor_t;
    (floor_t > 0.0
        && floor.x > ROOM_X[0] + margin
        && floor.x < ROOM_X[1] - margin
        && floor.z > ROOM_Z[0] + margin
        && floor.z < ROOM_Z[1] - margin)
        .then_some(Surface::Floor)
}

/// A frame with the overlay adding nothing, so scene colour holds the
/// opaque passes alone, as the chain reads them.
fn opaque_frame(renderer: &mut Renderer, view: View) -> FloorFrame {
    with_variant(renderer, "fs_test_zero", |renderer| {
        floor_frame(renderer, view)
    })
}

/// [`opaque_frame`] with forward's and the mover's test highlights.
fn highlighted_frame(renderer: &mut Renderer, view: View) -> FloorFrame {
    with_forward_highlight(renderer, |renderer| {
        with_mover_highlight(renderer, |renderer| opaque_frame(renderer, view))
    })
}

/// The mover standing in front of the `_s`-only wall.
fn mover_in_front() -> Mat4 {
    Mat4::from_translation(Vec3::new(-1.5, 0.0, 6.0))
}

/// A mover whose upright quad faces the eye (−Z), so a light between them
/// draws its highlight on the side the camera sees.
fn facing_mover() -> postretro_level_loader::KinematicGeometry {
    let corners = [
        [-0.7, 0.0, 0.0],
        [0.7, 0.0, 0.0],
        [0.7, 1.8, 0.0],
        [-0.7, 1.8, 0.0],
    ];
    mover_of(&corners, [0.0, 0.0, -1.0], [-1.0, 0.0, 0.0])
}

/// A runtime point light between the eye and the mover, reaching the walls.
fn front_light() -> ([u8; 64], [u8; 16]) {
    point_light(Vec3::new(-1.5, 1.5, 4.0), Vec3::splat(4.0), 12.0, 0)
}

/// Assert every texel of `floor`'s chain: level 0 is the box mean of the
/// highlight-free colour over the floor tier's block, each further level
/// the mean of the level above, within the format's slack.
fn assert_chain_box_filters(renderer: &Renderer, floor: &FloorTargets, what: &str) {
    let (scene, source) = &floor.highlight_free;
    let (extent, _) = &floor.chain[0];
    let block = scene.x / extent.x;
    assert_eq!(
        (scene.x % extent.x, scene.y % extent.y, scene.y / extent.y),
        (0, 0, block),
        "{what}: the proof extent divides evenly"
    );
    let mut levels = vec![(*scene, source)];
    levels.extend(floor.chain.iter().map(|(extent, texels)| (*extent, texels)));
    for (level, pair) in levels.windows(2).enumerate() {
        let (above_extent, above) = pair[0];
        let (extent, texels) = pair[1];
        // Four taps a quarter texel either side make an exact box at two or
        // four source texels a side.
        let block = above_extent / extent;
        let even = above_extent == extent * block && block.x == block.y && matches!(block.x, 2 | 4);
        for (index, texel) in texels.iter().enumerate() {
            let cell = UVec2::new(index as u32 % extent.x, index as u32 / extent.x);
            if even {
                let expected = box_mean(above, above_extent, cell, block.x);
                assert!(
                    (*texel - expected).abs().max_element()
                        <= store_slack(renderer, expected.max_element()),
                    "{what}: level {level} at {cell} is {texel}, the box mean {expected}"
                );
            } else {
                // An odd axis halves with bilinear taps across the texels
                // it straddles: the texel stays within their range.
                let (low, high) = footprint_range(above, above_extent, cell, extent);
                let slack = store_slack(renderer, high.max_element());
                assert!(
                    texel.cmpge(low - slack).all() && texel.cmple(high + slack).all(),
                    "{what}: level {level} at {cell} is {texel}, outside its footprint's \
                     [{low}, {high}]"
                );
            }
        }
    }
}

/// The per-channel range of the texels of `above` that destination texel
/// `cell` of an `extent` grid covers, by its edges in `above`'s texels.
fn footprint_range(
    above: &[Vec3],
    above_extent: UVec2,
    cell: UVec2,
    extent: UVec2,
) -> (Vec3, Vec3) {
    let scale = above_extent.as_vec2() / extent.as_vec2();
    let start = (cell.as_vec2() * scale).floor().as_uvec2();
    let end = ((cell + 1).as_vec2() * scale)
        .ceil()
        .as_uvec2()
        .min(above_extent);
    let mut range = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for y in start.y..end.y {
        for x in start.x..end.x {
            let texel = above[(y * above_extent.x + x) as usize];
            range = (range.0.min(texel), range.1.max(texel));
        }
    }
    range
}

// The halves the floor march does not need are dropped.
// At a floor tier, a wall with `_s` and no `_p` (untinted), a glossy wall
// and the glossy floor (tinted, then untinted) and a mover face each draw a
// bright highlight from a test-only static highlight (forward's world
// static specular a constant, the mover's selected-static gate opened for a
// runtime light). The highlight-free colour holds each pass's colour with
// that highlight left out: the colour of the same frame without the test
// highlights, within the stores' rounding. The chain's level 0 is its box
// mean, so the scene colour minus highlights, and each level the mean of
// the one above. Both specular styles.
#[test]
fn the_highlight_free_colour_leaves_each_passs_highlight_out_and_the_chain_filters_it() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_room(&mut renderer, GLOSSY_BROAD_TINTED);
    install_mover(&mut renderer, &facing_mover());
    set_mover(&mut renderer, Some(mover_in_front()));
    set_light(&mut renderer, Some(front_light()));
    let view = room_view();
    for style in SpecularStyle::ALL {
        renderer.set_specular_style(style);
        for glossy in [GLOSSY_BROAD_TINTED, GLOSSY_BROAD_UNTINTED] {
            set_bucket_material(&mut renderer, 0, glossy, true);
            let what = format!("{style:?}, glossy slot {:?}", glossy.slot);
            let plain = opaque_frame(&mut renderer, view);
            let marks = read_guard_marks(&renderer).expect("D_static");
            let lit = highlighted_frame(&mut renderer, view);
            let mut highlighted = [0usize; 4];
            for pixel in pixels() {
                let expected = plain.colour(pixel);
                let separated = lit.highlight_free(pixel);
                assert!(
                    (separated - expected).abs().max_element()
                        <= store_slack(&renderer, expected.max_element()),
                    "{what}: {pixel} keeps {separated}, the frame without highlights {expected}"
                );
                // Where the test highlight is bright, count its surface.
                if (lit.colour(pixel) - expected).max_element() < HIGHLIGHT * 0.1 {
                    continue;
                }
                let slot = if at(&marks, pixel) == 1.0 {
                    0
                } else {
                    match surface_at(view, pixel) {
                        Some(Surface::Floor) => 1,
                        Some(Surface::GlossyWall) => 2,
                        Some(Surface::SpecularWall) => 3,
                        None => continue,
                    }
                };
                highlighted[slot] += 1;
            }
            assert!(
                highlighted.iter().all(|&count| count >= 8),
                "{what}: bright highlights on the mover, floor, glossy wall and `_s` wall: \
                 {highlighted:?}"
            );
            assert_chain_box_filters(&renderer, lit.floor.as_ref().expect("targets"), &what);
        }
    }
    renderer.release_level_resources();
}

// Own outputs, A/B within the build: forward's
// and the mover pass's colour at a floor tier (their highlight-free forms)
// is bit for bit the colour at Low (the two-output forms) and, for the
// production sources, at Off (forward's one-output form): adding the
// highlight-free output moves no colour bit, test highlights or none, in
// both specular styles. The overlay adds nothing, so scene colour is the
// opaque passes alone.
#[test]
fn forward_and_mover_colour_is_the_same_bits_at_a_floor_tier_as_below_it() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_room(&mut renderer, GLOSSY_BROAD_TINTED);
    install_mover(&mut renderer, &facing_mover());
    set_mover(&mut renderer, Some(mover_in_front()));
    set_light(&mut renderer, Some(front_light()));
    let view = room_view();
    for style in SpecularStyle::ALL {
        renderer.set_specular_style(style);
        let colour = |renderer: &mut Renderer, tier, highlighted: bool| {
            renderer.set_reflection_quality(tier);
            let frame = if highlighted {
                highlighted_frame(renderer, view)
            } else {
                opaque_frame(renderer, view)
            };
            let outputs = renderer.full().kinematic_brush.take_bound_outputs();
            (frame, outputs)
        };
        for highlighted in [false, true] {
            let (floor, floor_outputs) = colour(&mut renderer, High, highlighted);
            let (low, low_outputs) = colour(&mut renderer, Low, highlighted);
            assert!(floor.floor.is_some() && low.floor.is_none());
            assert_eq!(floor_outputs, [ColourOutputs::HighlightFree]);
            assert_eq!(low_outputs, [ColourOutputs::Colour]);
            assert_eq!(
                floor.glossy.scene.colour, low.glossy.scene.colour,
                "{style:?}, highlights {highlighted}: High's colour is Low's"
            );
            if !highlighted {
                let (off, off_outputs) = colour(&mut renderer, Off, false);
                assert_eq!(off_outputs, [ColourOutputs::Colour]);
                assert_eq!(
                    floor.glossy.scene.colour, off.glossy.scene.colour,
                    "{style:?}: High's colour is Off's one-output form's"
                );
            }
        }
        renderer.set_reflection_quality(High);
    }
    renderer.release_level_resources();
}

// With the walls' test highlights on screen, a
// mover and a character with no highlight of their own stand in front of
// them: their pixels' highlight-free colour is their own pass's colour,
// with nothing of the wall's highlight taken from them. On the next frame,
// with both moved away (still drawn) or neither drawn at all, the
// highlight-free colour and the chain are the frame without them, bit for
// bit: the walls without their highlight, no trace of the frame before.
#[test]
fn movers_and_characters_keep_their_own_colour_and_leave_no_trace() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_room(&mut renderer, FloorMaterial::WHITE_SHARP);
    install_mover(&mut renderer, &upright_mover());
    install_skinned_model(&mut renderer, CHARACTER, &character_quads());
    let view = room_view();
    let frame = |renderer: &mut Renderer| {
        with_forward_highlight(renderer, |renderer| opaque_frame(renderer, view))
    };

    set_mover(&mut renderer, None);
    renderer.set_mesh_draws(&[]);
    let without = frame(&mut renderer);

    set_mover(&mut renderer, Some(mover_in_front()));
    renderer.set_mesh_draws(&[character_at(Vec3::new(1.5, 0.0, 6.0))]);
    let with = frame(&mut renderer);
    let marks = read_guard_marks(&renderer).expect("D_static");
    let (mut mover, mut character) = (0, 0);
    for pixel in pixels() {
        let is_mover = at(&marks, pixel) == 1.0;
        let is_character = !is_mover && with.colour(pixel) != without.colour(pixel);
        if !is_mover && !is_character {
            continue;
        }
        let own = with.colour(pixel);
        assert!(
            (with.highlight_free(pixel) - own).abs().max_element()
                <= store_slack(&renderer, own.max_element()),
            "{pixel}: the highlight-free colour is the pass's own colour {own}"
        );
        // The wall behind would have lost its highlight there.
        let behind = without.colour(pixel) - without.highlight_free(pixel);
        if matches!(
            surface_at(view, pixel),
            Some(Surface::GlossyWall | Surface::SpecularWall)
        ) && behind.max_element() > HIGHLIGHT * 0.1
        {
            if is_mover {
                mover += 1;
            } else {
                character += 1;
            }
        }
    }
    assert!(mover >= 8, "the mover covers the highlighted wall: {mover}");
    assert!(
        character >= 8,
        "the character covers the highlighted wall: {character}"
    );

    for (what, mover, meshes) in [
        (
            "moved away",
            Some(Mat4::from_translation(Vec3::new(0.0, 0.0, -40.0))),
            vec![character_at(Vec3::new(0.0, 0.0, -40.0))],
        ),
        ("not drawn", None, Vec::new()),
    ] {
        set_mover(&mut renderer, mover);
        renderer.set_mesh_draws(&meshes);
        let next = frame(&mut renderer);
        assert_eq!(
            next.floor, without.floor,
            "{what}: the walls without their highlight and the chain, no trace"
        );
    }
    renderer.release_level_resources();
}

/// A width and height.
type Size = (u32, u32);

/// The floor targets' extents: the highlight-free colour's and the chain's
/// level 0, with the chain's level count; `None` when not allocated.
fn floor_extents(renderer: &Renderer) -> Option<(Size, Size, u32)> {
    let targets = renderer.level().glossy.as_ref()?.floor()?;
    // The view the floor march samples covers every level of the chain.
    let view = targets.chain_view().texture();
    assert_eq!(view.mip_level_count(), targets.chain_mip_count());
    assert_eq!(view.width(), targets.chain_extent().width);
    let scene = targets.scene_extent();
    let chain = targets.chain_extent();
    Some((
        (scene.width, scene.height),
        (chain.width, chain.height),
        targets.chain_mip_count(),
    ))
}

/// The ledger's floor-layer rows, `(highlight-free, chain)` bytes.
fn floor_ledger(renderer: &Renderer) -> (Option<u64>, Option<u64>) {
    let ledger = renderer
        .reflection_probe_residency_report()
        .expect("full renderer");
    (
        ledger.bytes(GLOSSY_HIGHLIGHT_FREE),
        ledger.bytes(GLOSSY_SCENE_COLOUR_CHAIN),
    )
}

/// The ledger's floor-layer target rows, `(target, march depth)` bytes.
fn floor_layer_ledger(renderer: &Renderer) -> (Option<u64>, Option<u64>) {
    let ledger = renderer
        .reflection_probe_residency_report()
        .expect("full renderer");
    (
        ledger.bytes(GLOSSY_FLOOR_LAYER),
        ledger.bytes(GLOSSY_FLOOR_LAYER_DEPTH),
    )
}

/// Assert a frame drawn now meets the floor layer's boundary rows: its
/// targets at `scene` and the tier's fraction of it (`chain`), down to 1×1,
/// the highlight-free colour holding this frame's passes (the opaque scene
/// colour itself, the mover's included, with no test highlight drawn), the
/// chain its box mean, the ledger counting both. Returns what the frame
/// created.
fn assert_boundary_frame(
    renderer: &mut Renderer,
    scene: (u32, u32),
    chain: (u32, u32),
    what: &str,
) -> GpuCreationCounts {
    let levels = u32::BITS - chain.0.max(chain.1).leading_zeros();
    assert_eq!(
        floor_extents(renderer),
        Some((scene, chain, levels)),
        "{what}: the targets follow the current extent, the chain to 1×1"
    );
    let chains = renderer.full().glossy_overlay.floor().chains();
    let marches = renderer.full().glossy_overlay.floor().marches();
    let created = with_variant(renderer, "fs_test_zero", |renderer| {
        let before = gpu_creation_counts();
        frame_at(renderer, room_view(), 0.0);
        gpu_creation_counts().since(before)
    });
    assert_eq!(
        renderer.full().glossy_overlay.floor().chains(),
        chains + 1,
        "{what}: one chain"
    );
    assert_eq!(
        renderer.full().glossy_overlay.floor().marches(),
        marches + 1,
        "{what}: one march"
    );
    let floor = read_floor_targets(renderer).expect("targets");
    let (extent, colour) = read_scene_colour(renderer);
    assert_eq!(extent, UVec2::new(scene.0, scene.1));
    assert_eq!(floor.highlight_free.0, extent);
    let mut moved = 0;
    for (index, (free, own)) in floor.highlight_free.1.iter().zip(&colour).enumerate() {
        assert!(
            (*free - *own).abs().max_element() <= store_slack(renderer, own.max_element()),
            "{what}: texel {index} holds this frame's colour {own}, not {free}"
        );
        moved += usize::from(*own != colour[0]);
    }
    assert!(moved > 0, "{what}: the frame draws something");
    assert_chain_box_filters(renderer, &floor, what);
    let (bytes_free, bytes_chain) = floor_ledger(renderer);
    assert!(
        bytes_free.is_some_and(|bytes| bytes >= u64::from(scene.0 * scene.1) * 4),
        "{what}: the ledger counts the highlight-free colour: {bytes_free:?}"
    );
    assert!(bytes_chain.is_some(), "{what}: the ledger counts the chain");
    // The floor layer's target and the march's depth at the chain's level-0
    // extent: eight and four bytes a texel.
    let texels = u64::from(chain.0 * chain.1);
    assert_eq!(
        floor_layer_ledger(renderer),
        (Some(texels * 8), Some(texels * 4)),
        "{what}: the ledger counts the floor layer and its depth"
    );
    let layer = read_floor_layer(renderer).expect("the floor layer");
    assert_eq!(layer.extent, UVec2::new(chain.0, chain.1));
    created
}

/// Assert the floor targets are gone at the current tier: no allocation,
/// no ledger row, no chain on a frame, the mover drawing its one output.
fn assert_released(renderer: &mut Renderer, what: &str) {
    assert_eq!(floor_extents(renderer), None, "{what}: no floor targets");
    assert_eq!(
        floor_ledger(renderer),
        (None, None),
        "{what}: no ledger rows"
    );
    assert_eq!(floor_layer_ledger(renderer), (None, None));
    let marches = renderer.full().glossy_overlay.floor().marches();
    let chains = renderer.full().glossy_overlay.floor().chains();
    renderer.full().kinematic_brush.take_bound_outputs();
    floor_frame(renderer, room_view());
    assert_eq!(
        renderer.full().glossy_overlay.floor().chains(),
        chains,
        "{what}: no chain"
    );
    assert_eq!(
        renderer.full().glossy_overlay.floor().marches(),
        marches,
        "{what}: no march"
    );
    assert_eq!(
        renderer.full().kinematic_brush.take_bound_outputs(),
        [ColourOutputs::Colour],
        "{what}: the mover writes its colour alone"
    );
}

/// Build the room as a level set off the main thread at the renderer's
/// tiers and scene extent, returning its id unpublished.
fn build_room_off_main(renderer: &mut Renderer) -> super::LevelSetId {
    let probes = floor_probe(true);
    let tiers = renderer.level_build_tiers();
    let (builder, mut pending) = renderer.begin_level_set_build(tiers);
    let section = probes.clone();
    builder.geometry_from(
        Box::new(move |visit| {
            with_faces(&room_faces(), Some(&section), None, |geometry| {
                visit(
                    geometry,
                    super::level_build::GeometryExtras {
                        volumes: &[],
                        reflection_probes: Some(&section),
                        fog_pixel_scale: 0,
                        fog_cell_masks: None,
                    },
                )
            })
        }),
        super::level_build::GeometryRest {
            gpu_lighting_payloads: Default::default(),
            sh_plan: None,
            lightmap_plan: None,
            fluid: None,
            planar_candidate: None,
        },
    );
    builder.finish();
    take(renderer, &mut pending)
}

/// Publish `id` and give it the room's materials and the mover.
fn publish_room(renderer: &mut Renderer, id: super::LevelSetId) {
    renderer.publish_level_set(id, super::level_resource_set::PreviousLevelSet::Retire);
    set_bucket_material(renderer, 0, FloorMaterial::WHITE_SHARP, true);
    set_bucket_material(renderer, 1, SPECULAR_WALL, false);
    install_mover(renderer, &upright_mover());
    set_mover(renderer, Some(mover_in_front()));
}

// Lifecycle: the highlight-free colour,
// the chain and the floor layer's target and depth follow the floor layer.
// On the first frame after install, a resize, a level set published at
// another extent and a live low → medium switch, all sit at the current
// extent (the chain and the floor layer at the tier's fraction, the chain
// down to 1×1), hold that frame's passes (the mover's colour) and appear in
// the memory ledger, and the frame records one chain and one march. At low
// and off, and after unload, none exists, no chain or march records and the
// mover writes its one output. A tier change builds no pipeline under either
// specular style: low → medium builds the targets and their groups alone,
// high → low the overlay's inputs over the placeholder alone.
#[test]
fn the_floor_targets_follow_the_floor_tier_and_extent_and_release_below_it() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let frame = (FRAME, FRAME);
    let half = (FRAME / 2, FRAME / 2);
    let quarter = (FRAME / 4, FRAME / 4);

    install_room(&mut renderer, FloorMaterial::WHITE_SHARP);
    install_mover(&mut renderer, &upright_mover());
    set_mover(&mut renderer, Some(mover_in_front()));
    let created = assert_boundary_frame(&mut renderer, frame, half, "first frame after install");
    assert_eq!(
        created.pipelines, 0,
        "the frame after install builds no pipeline"
    );

    // A resize, and back.
    renderer.record_surface_size(48, 40);
    renderer.commit_extents();
    assert_boundary_frame(
        &mut renderer,
        (48, 40),
        (24, 20),
        "first frame after a resize",
    );
    renderer.record_surface_size(FRAME, FRAME);
    renderer.commit_extents();
    assert_boundary_frame(
        &mut renderer,
        frame,
        half,
        "first frame back at the proof extent",
    );

    // A level set built at the proof extent, published at another.
    let id = build_room_off_main(&mut renderer);
    renderer.record_surface_size(48, 40);
    renderer.commit_extents();
    publish_room(&mut renderer, id);
    assert_boundary_frame(
        &mut renderer,
        (48, 40),
        (24, 20),
        "first frame of a level set published at another extent",
    );
    renderer.record_surface_size(FRAME, FRAME);
    renderer.commit_extents();

    for style in SpecularStyle::ALL {
        renderer.set_specular_style(style);
        let what = |change: &str| format!("{style:?}: {change}");

        let before = gpu_creation_counts();
        renderer.set_reflection_quality(Low);
        let built = gpu_creation_counts().since(before);
        assert_eq!(
            (built.total(), built.bind_groups),
            (1, 1),
            "{}: {built:?}",
            what("high → low builds the overlay's inputs over the placeholder alone")
        );
        assert_released(&mut renderer, &what("low"));
        assert!(renderer.level().glossy.as_ref().is_some_and(|g| g.ready()));

        let before = gpu_creation_counts();
        renderer.set_reflection_quality(Medium);
        let built = gpu_creation_counts().since(before);
        let levels = floor_extents(&renderer).expect("medium targets").2;
        // The highlight-free colour, the chain, the floor layer and its
        // depth; the march uniform; a group per chain level, the march's
        // and the overlay's inputs over the new floor layer.
        assert_eq!(
            (
                built.pipelines,
                built.buffers,
                built.textures,
                built.bind_groups
            ),
            (0, 1, 4, u64::from(levels) + 2),
            "{}: {built:?}",
            what("low → medium builds the targets and their groups alone")
        );
        let created =
            assert_boundary_frame(&mut renderer, frame, quarter, &what("first medium frame"));
        assert_eq!(created.pipelines, 0);

        let before = gpu_creation_counts();
        renderer.set_reflection_quality(Off);
        assert_eq!(gpu_creation_counts().since(before).total(), 0);
        assert_released(&mut renderer, &what("off"));

        let before = gpu_creation_counts();
        renderer.set_reflection_quality(High);
        assert_eq!(
            gpu_creation_counts().since(before).pipelines,
            0,
            "{}",
            what("off → high builds no pipeline")
        );
        assert_boundary_frame(&mut renderer, frame, half, &what("first high frame"));
    }

    renderer.release_level_resources();
    assert_eq!(
        floor_ledger(&renderer),
        (None, None),
        "unload: no ledger rows"
    );
    assert_eq!(floor_layer_ledger(&renderer), (None, None));
    let ledger = renderer.reflection_probe_residency_report().expect("full");
    assert_eq!(ledger.bytes(GLOSSY_STATIC_DIFFUSE), None);
}

/// The sets, active first, whose glossy state holds any target.
fn sets_holding_glossy_targets(renderer: &Renderer) -> Vec<super::LevelSetId> {
    let full = renderer.full();
    full.level
        .iter()
        .chain(&full.resident_levels)
        .filter(|set| {
            set.glossy
                .as_ref()
                .is_some_and(|glossy| glossy.residency_rows().next().is_some())
        })
        .map(|set| set.id)
        .collect()
}

/// The ledger's bytes for every glossy row: D_static, the highlight-free
/// colour, the chain, the floor layer and its depth.
fn glossy_ledger(renderer: &Renderer) -> [Option<u64>; 5] {
    let ledger = renderer
        .reflection_probe_residency_report()
        .expect("full renderer");
    [
        GLOSSY_STATIC_DIFFUSE,
        GLOSSY_HIGHLIGHT_FREE,
        GLOSSY_SCENE_COLOUR_CHAIN,
        GLOSSY_FLOOR_LAYER,
        GLOSSY_FLOOR_LAYER_DEPTH,
    ]
    .map(|row| ledger.bytes(row))
}

/// Publish `id`, keeping the set it replaces resident; returns that set's
/// key and what the publish created.
fn publish_keeping(
    renderer: &mut Renderer,
    id: super::LevelSetId,
) -> (Option<super::LevelSetId>, GpuCreationCounts) {
    let before = gpu_creation_counts();
    let kept = renderer.publish_level_set(
        id,
        super::level_resource_set::PreviousLevelSet::KeepResident,
    );
    (kept, gpu_creation_counts().since(before))
}

// Owner ruling on glossy targets and level sets. No set's build makes a
// glossy target: a set built beside a published glossy level holds no
// D_static and no floor target, and the ledger counts the published set's
// alone. Its publish at a tier that moved since its build builds each target
// once (D_static and the four floor targets: five textures), counted, at the
// kept tier, and the set it keeps resident gives its own up. A set published
// back at a moved extent builds its own at that extent. At no step do two
// sets hold targets, and the ledger always holds one set's rows.
#[test]
fn glossy_targets_exist_only_on_the_published_set_and_build_once_at_its_publish() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let quarter = (FRAME / 4, FRAME / 4);
    install_room(&mut renderer, FloorMaterial::WHITE_SHARP);
    let first = renderer.active_level_set().expect("installed");
    assert_eq!(sets_holding_glossy_targets(&renderer), [first]);
    let ledger = glossy_ledger(&renderer);
    assert!(
        ledger.iter().all(Option::is_some),
        "the published set's rows: {ledger:?}"
    );

    // Built off the main thread beside the published level: no target, no
    // ledger row of its own.
    let id = build_room_off_main(&mut renderer);
    let built = renderer
        .level_set(id)
        .and_then(|set| set.glossy.as_ref())
        .expect("the built set's glossy state");
    assert!(built.static_diffuse_view().is_none() && built.floor().is_none());
    assert_eq!(
        built.residency_rows().count(),
        0,
        "the built set: no target"
    );
    assert_eq!(sets_holding_glossy_targets(&renderer), [first]);
    assert_eq!(
        glossy_ledger(&renderer),
        ledger,
        "the ledger: one set's rows"
    );

    // The tier moves while the set is resident; its publish builds each
    // target once, at the kept tier.
    renderer.set_reflection_quality(Medium);
    assert_eq!(sets_holding_glossy_targets(&renderer), [first]);
    let builds = renderer.glossy_target_builds();
    let (kept, created) = publish_keeping(&mut renderer, id);
    assert_eq!(kept, Some(first));
    assert_eq!(
        (created.pipelines, created.textures),
        (0, 5),
        "the publish builds each glossy target once: {created:?}"
    );
    assert_eq!(renderer.glossy_target_builds(), builds + 1, "counted");
    assert_eq!(
        sets_holding_glossy_targets(&renderer),
        [id],
        "the kept set gave its targets up"
    );
    set_bucket_material(&mut renderer, 0, FloorMaterial::WHITE_SHARP, true);
    set_bucket_material(&mut renderer, 1, SPECULAR_WALL, false);
    assert_boundary_frame(
        &mut renderer,
        (FRAME, FRAME),
        quarter,
        "the published set at the kept tier",
    );

    // The extent moves; the set published back builds its own at it.
    renderer.record_surface_size(48, 40);
    renderer.commit_extents();
    let (kept, _) = publish_keeping(&mut renderer, first);
    assert_eq!(kept, Some(id));
    assert_eq!(renderer.glossy_target_builds(), builds + 2);
    assert_eq!(sets_holding_glossy_targets(&renderer), [first]);
    assert_boundary_frame(
        &mut renderer,
        (48, 40),
        (12, 10),
        "published back at a moved extent",
    );
    let published = glossy_ledger(&renderer);

    // A swap at an unchanged tier and extent: five textures again, and the
    // ledger holds one set's rows, the same bytes.
    let (kept, created) = publish_keeping(&mut renderer, id);
    assert_eq!(kept, Some(first));
    assert_eq!((created.pipelines, created.textures), (0, 5), "{created:?}");
    assert_eq!(sets_holding_glossy_targets(&renderer), [id]);
    assert_eq!(glossy_ledger(&renderer), published);

    // Unload: no set holds a target and the ledger has no glossy row.
    assert!(renderer.release_level_set(id));
    assert!(sets_holding_glossy_targets(&renderer).is_empty());
    assert_eq!(glossy_ledger(&renderer), [None; 5]);
    assert!(renderer.release_level_set(first));
}

// A tier change between floor tiers of one fraction (high, very high and
// ultra, all ½) keeps every target: it builds no texture or pipeline, only
// the march's uniform and group, and the first frame at each tier records
// one chain and one march at the same extents.
#[test]
fn floor_tiers_of_one_fraction_share_the_floor_targets() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let frame = (FRAME, FRAME);
    let half = (FRAME / 2, FRAME / 2);
    install_room(&mut renderer, FloorMaterial::WHITE_SHARP);
    renderer.set_reflection_quality(High);
    assert_boundary_frame(&mut renderer, frame, half, "high");
    for (tier, what) in [
        (VeryHigh, "high → very high"),
        (High, "very high → high"),
        (Ultra, "high → ultra"),
        (VeryHigh, "ultra → very high"),
    ] {
        let before = gpu_creation_counts();
        renderer.set_reflection_quality(tier);
        let built = gpu_creation_counts().since(before);
        assert_eq!(
            (
                built.pipelines,
                built.textures,
                built.buffers,
                built.bind_groups
            ),
            (0, 0, 1, 1),
            "{what}: the march's uniform and group alone: {built:?}"
        );
        assert_boundary_frame(&mut renderer, frame, half, what);
    }
}

// No up-facing face. A level whose glossy faces
// bound to a probe are all walls takes no floor layer: at a floor tier it
// allocates no highlight-free colour and no chain, records no chain, and
// its mover writes its one output, while the overlay still draws the
// wall's probe term.
#[test]
fn a_level_without_an_up_facing_glossy_face_takes_no_floor_layer() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let mut probes = floor_probe(false);
    probes
        .world_bindings
        .push(ReflectionProbeWorldBinding { face: 1, probe: 0 });
    install_faces(&mut renderer, &room_faces(), &probes);
    set_bucket_material(&mut renderer, 0, FloorMaterial::WHITE_SHARP, true);
    install_mover(&mut renderer, &upright_mover());
    set_mover(&mut renderer, Some(mover_in_front()));
    for tier in [Medium, High] {
        renderer.set_reflection_quality(tier);
        let glossy = renderer.level().glossy.as_ref().expect("glossy state");
        assert!(!glossy.has_floor_faces() && glossy.ready());
        let overlays_before = overlays(&renderer);
        assert_released(&mut renderer, &format!("{tier:?}, walls alone"));
        assert_eq!(
            overlays(&renderer),
            overlays_before + 1,
            "the overlay draws"
        );
    }
    renderer.release_level_resources();
}

// The floor blur matches the probe term the section's own header sets: the
// march's uniform carries α₀ from id 54's tile dimension and its colour mip
// count, for a 64² and a 256² capture with full chains and a one-mip 128²
// section (whose probe term reads mip 0 alone), at each floor tier. The
// march binds no probe array, so the header reaches it through the uniform
// alone.
#[test]
fn the_floor_march_blurs_by_the_sections_own_tile_and_mip_count() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    for (tile, mips) in [
        (64, reflection_probe_full_colour_mip_count(64)),
        (256, reflection_probe_full_colour_mip_count(256)),
        (REFLECTION_PROBE_TILE_DIMENSION, 1),
    ] {
        let mut probes = floor_probe(true);
        let texel = probes.tiles[..8].to_vec();
        let texels: u32 = (0..mips).map(|mip| (tile >> mip).pow(2)).sum();
        probes.tile_dimension = tile;
        probes.colour_mip_count = mips;
        probes.tiles = texel.repeat(texels as usize);
        probes.distances = reflection_probe_zero_distances(1, tile);
        install_faces(&mut renderer, &room_faces(), &probes);
        set_bucket_material(&mut renderer, 0, FloorMaterial::WHITE_SHARP, true);
        for tier in [Medium, High] {
            renderer.set_reflection_quality(tier);
            let glossy = renderer.level().glossy.as_ref().expect("glossy state");
            let march = glossy
                .floor()
                .expect("a floor tier builds the floor targets")
                .march();
            assert_eq!(
                march.term_blur,
                GlossyFloorTermBlur::of_section(tile, mips),
                "{tile}² × {mips} at {tier:?}"
            );
        }
        renderer.release_level_resources();
    }
}

// No overlay output. The chain records before the
// overlay: it is bit for bit the same whatever the overlay adds, while the
// scene colour it leaves differs.
#[test]
fn the_chain_holds_no_overlay_output() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_room(&mut renderer, FloorMaterial::WHITE_SHARP);
    install_mover(&mut renderer, &upright_mover());
    set_mover(&mut renderer, Some(mover_in_front()));
    let view = room_view();
    let production = floor_frame(&mut renderer, view);
    let zero = opaque_frame(&mut renderer, view);
    let constant = with_variant(&mut renderer, "fs_test_constant", |renderer| {
        floor_frame(renderer, view)
    });
    assert_ne!(production.glossy.scene.colour, zero.glossy.scene.colour);
    assert_ne!(constant.glossy.scene.colour, zero.glossy.scene.colour);
    for frame in [&production, &constant] {
        assert_eq!(frame.floor, zero.floor);
    }
    renderer.release_level_resources();
}
