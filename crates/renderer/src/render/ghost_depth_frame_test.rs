//! Adapter-backed proof of the ghost pass through the production capture
//! recorder: under both `render.ghosts` modes a ghost shows only its own
//! nearest surface, skinned and mover alike, and loses none of its pixels to
//! the `Equal` test; a nearer ghost blends over a farther one, a ghost
//! platform included; and a fluid surface behind a ghost still draws over it,
//! so the ghost pass leaves scene depth to the opaque scene.
//! See: context/lib/rendering_pipeline.md §7.9

use glam::{Mat4, Vec3};
use postretro_level_format::geometry::Vertex;
use postretro_model::gltf_loader::{LoadedModel, Submesh};
use postretro_model::mesh::{SkinnedMesh, SkinnedVertex};
use postretro_model::sample_params::MeshSampleParams;
use postretro_model::skeleton::{Joint, RestLocal, Skeleton};
use postretro_render_cpu::fluid_surface::FluidSurfaceLook;
use postretro_render_cpu::ghost_instance::{
    GhostInstanceLook, KinematicGhostInstance, screen_door_coverage, screen_door_threshold,
};
use postretro_render_cpu::ghost_transparency::GhostTransparency;
use postretro_render_cpu::mesh_instances::{
    MeshInstanceInput, MeshPaletteCacheKey, MeshPresentation, plan_mesh_frame_plans,
};

use super::Renderer;
use super::fluid_surface_frame_test::{
    SIDE, capture_encode, capture_from, fluid_quad, look_draw, renderer,
};

/// One mover holding a near quad and, overlapping it on screen, a far one:
/// the self-overlap a closed mouth over teeth makes. Then each quad alone.
const NEAR_AND_FAR: u32 = 1;
const NEAR_ONLY: u32 = 2;
const FAR_ONLY: u32 = 3;
/// Two distinct ghosts' movers, each an outer quad in front of an inner one.
const NEAR_GHOST: u32 = 4;
const FAR_GHOST: u32 = 5;
/// A long platform receding to the right, for a pawn to stand on.
const PLATFORM: u32 = 6;

/// Skinned twins of the self-overlap movers and of both distinct ghosts, and
/// the pawn standing on the platform.
const SKINNED_NEAR_AND_FAR: &str = "ghost-depth/near-and-far";
const SKINNED_NEAR_ONLY: &str = "ghost-depth/near-only";
const SKINNED_FAR_ONLY: &str = "ghost-depth/far-only";
const SKINNED_NEAR_GHOST: &str = "ghost-depth/near-ghost";
const SKINNED_FAR_GHOST: &str = "ghost-depth/far-ghost";
const SKINNED_PAWN: &str = "ghost-depth/pawn";

/// Clip depths under the identity view-projection; the fluid quad sits at 0.5.
const NEAR_DEPTH: f32 = 0.3;
const FAR_DEPTH: f32 = 0.6;
/// The distinct ghosts' surfaces: each ghost's inner surface lies behind its
/// outer one, and the whole near ghost in front of the far one.
const NEAR_GHOST_OUTER: f32 = 0.2;
const NEAR_GHOST_INNER: f32 = 0.25;
const FAR_GHOST_OUTER: f32 = 0.6;
const FAR_GHOST_INNER: f32 = 0.65;
/// The platform's depth at its near (left) and far (right) edges, so its
/// bounds centre sits at 0.5 and its farthest corner at 0.875; the pawn's
/// depth, nearer than the platform across the pawn's columns but behind the
/// platform's centre.
const PLATFORM_NEAR_EDGE: f32 = 0.125;
const PLATFORM_FAR_EDGE: f32 = 0.875;
const PAWN_DEPTH: f32 = 0.6;

/// Texel centres of the 64-texel placeholder checkerboard (8-texel squares,
/// magenta at the origin): one magenta square, one black.
const MAGENTA_UV: [f32; 2] = [4.5 / 64.0, 4.5 / 64.0];
const BLACK_UV: [f32; 2] = [12.5 / 64.0, 4.5 / 64.0];
/// Those texels in linear light, and the white clear behind every ghost.
const MAGENTA: Vec3 = Vec3::new(1.0, 0.0, 1.0);
const BLACK: Vec3 = Vec3::ZERO;
const WHITE: Vec3 = Vec3::ONE;

/// Columns where the near quad (clip x −0.75..0.25) overlaps the far quad
/// (−0.25..0.75), where only the far quad lies, and where each covers; rows
/// inside both quads' clip y −0.75..0.75. Pixel centres sit at
/// −1 + (i + 0.5) / 8.
const OVERLAP_COLUMNS: std::ops::Range<u32> = 6..10;
const FAR_ONLY_COLUMNS: std::ops::Range<u32> = 10..14;
const NEAR_COLUMNS: std::ops::Range<u32> = 2..10;
const FAR_COLUMNS: std::ops::Range<u32> = 6..14;
const ROWS: std::ops::Range<u32> = 3..13;
/// Columns the platform (clip x −0.75..0.75) covers, and the pawn on its far
/// half (0.25..0.75).
const PLATFORM_COLUMNS: std::ops::Range<u32> = 2..14;
const PAWN_COLUMNS: std::ops::Range<u32> = 10..14;

/// How far a region's kept-pixel count may sit from the screen door's share of
/// its area: the region's edges cut 4×4 tiles, whose partial rows and columns
/// need not keep exactly that share.
const TILE_EDGE_SLACK: f32 = 2.0;

const CAMERA: Vec3 = Vec3::new(0.0, 0.0, -1.0);

const LOOK: GhostInstanceLook = GhostInstanceLook {
    opacity: 0.5,
    saturation: 1.0,
    tint: [1.0, 1.0, 1.0],
};
const NEAR_LOOK: GhostInstanceLook = GhostInstanceLook {
    opacity: 0.4,
    ..LOOK
};
const FAR_LOOK: GhostInstanceLook = GhostInstanceLook {
    opacity: 0.6,
    ..LOOK
};

/// A quad facing the identity camera (counter-clockwise in clip space) over
/// clip x `x0..x1`, y −0.75..0.75, at clip depth `z0` along its `x0` edge and
/// `z1` along its `x1` edge, with one flat UV.
fn sloped_quad(x0: f32, x1: f32, z0: f32, z1: f32, uv: [f32; 2]) -> [Vertex; 4] {
    let vertex = |x: f32, y: f32, z: f32| {
        Vertex::new(
            [x, y, z],
            uv,
            [0.0, 0.0, -1.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        )
    };
    [
        vertex(x0, -0.75, z0),
        vertex(x1, -0.75, z1),
        vertex(x1, 0.75, z1),
        vertex(x0, 0.75, z0),
    ]
}

/// A [`sloped_quad`] at one clip depth `z`.
fn quad(x0: f32, x1: f32, z: f32, uv: [f32; 2]) -> [Vertex; 4] {
    sloped_quad(x0, x1, z, z, uv)
}

fn near_quad() -> [Vertex; 4] {
    quad(-0.75, 0.25, NEAR_DEPTH, MAGENTA_UV)
}

fn far_quad() -> [Vertex; 4] {
    quad(-0.25, 0.75, FAR_DEPTH, BLACK_UV)
}

/// The near distinct ghost, inner surface first in draw order: a magenta
/// outer quad over a black inner one.
fn near_ghost_quads() -> [[Vertex; 4]; 2] {
    [
        quad(-0.75, 0.25, NEAR_GHOST_INNER, BLACK_UV),
        quad(-0.75, 0.25, NEAR_GHOST_OUTER, MAGENTA_UV),
    ]
}

/// The far distinct ghost: a black outer quad over a magenta inner one.
fn far_ghost_quads() -> [[Vertex; 4]; 2] {
    [
        quad(-0.25, 0.75, FAR_GHOST_INNER, MAGENTA_UV),
        quad(-0.25, 0.75, FAR_GHOST_OUTER, BLACK_UV),
    ]
}

/// The platform: magenta, receding from the camera left to right.
fn platform_quad() -> [Vertex; 4] {
    sloped_quad(
        -0.75,
        0.75,
        PLATFORM_NEAR_EDGE,
        PLATFORM_FAR_EDGE,
        MAGENTA_UV,
    )
}

/// The pawn: black, upright in front of the platform's far half.
fn pawn_quad() -> [Vertex; 4] {
    quad(0.25, 0.75, PAWN_DEPTH, BLACK_UV)
}

/// A mover whose geometry is `quads` in draw order, at the origin.
fn mover(mover_id: u32, quads: &[[Vertex; 4]]) -> postretro_level_loader::LoadedKinematicMover {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for corners in quads {
        let base = vertices.len() as u32;
        vertices.extend_from_slice(corners);
        indices.extend([0, 1, 2, 0, 2, 3].map(|corner| base + corner));
    }
    postretro_level_loader::LoadedKinematicMover {
        mover_id,
        name: format!("ghost-depth-{mover_id}"),
        tags: Vec::new(),
        origin: Vec3::ZERO,
        path: String::new(),
        speed_mps: 1.0,
        wait_ms: 0.0,
        move_mode: 1,
        start_on_spawn: false,
        vertices,
        indices,
        face_meta: Vec::new(),
        spin_axis: Vec3::ZERO,
        spin_speed_deg_s: 0.0,
        spin_accel_deg_s2: 0.0,
        carry_yaw: false,
        block_policy: "displace".into(),
        crush_damage: 0.0,
        crush_interval_ms: 0.0,
        auto_close_ms: None,
        open_event: None,
        close_event: None,
        blocked_event: None,
        crush_event: None,
        open_sound: None,
        close_sound: None,
        blocked_sound: None,
        crush_sound: None,
        sealed_portal_ids: Vec::new(),
        carried_lights: Vec::new(),
    }
}

/// Install every mover. The doubled one draws its near quad first, so a
/// pass that let the far quad through would show it over the near one under
/// dither and under it twice-blended under blend.
fn install_movers(renderer: &mut Renderer) {
    let geometry = postretro_level_loader::KinematicGeometry {
        movers: vec![
            mover(NEAR_AND_FAR, &[near_quad(), far_quad()]),
            mover(NEAR_ONLY, &[near_quad()]),
            mover(FAR_ONLY, &[far_quad()]),
            mover(NEAR_GHOST, &near_ghost_quads()),
            mover(FAR_GHOST, &far_ghost_quads()),
            mover(PLATFORM, &[platform_quad()]),
        ],
        waypoints: Vec::new(),
    };
    let device = renderer.device.clone();
    renderer
        .full_mut()
        .kinematic_brush
        .install_geometry(&device, Some(&geometry), 1);
}

fn unorm16(value: f32) -> u16 {
    (value * 65535.0).round() as u16
}

/// Upload a rigid skinned model under `handle` whose geometry is `quads` in
/// draw order, one submesh on the placeholder material.
fn install_skinned_model(renderer: &mut Renderer, handle: &str, quads: &[[Vertex; 4]]) {
    let mut mesh = SkinnedMesh::default();
    for corners in quads {
        let base = mesh.vertices.len() as u32;
        mesh.vertices.extend(corners.iter().map(|corner| {
            SkinnedVertex::rigid(corner.position, corner.uv.map(unorm16), [0, 0], [0, 0])
        }));
        mesh.indices
            .extend([0, 1, 2, 0, 2, 3].map(|corner| base + corner));
    }
    mesh.compute_bounds();
    let index_count = mesh.indices.len() as u32;
    let model = LoadedModel {
        mesh,
        skeleton: Skeleton {
            joints: vec![Joint {
                parent: None,
                inverse_bind: Mat4::IDENTITY.to_cols_array_2d(),
                rest_local: RestLocal::default(),
            }],
        },
        // The all-zero key resolves to the placeholder checkerboard.
        submeshes: vec![Submesh {
            material_key: "0".repeat(64),
            indices: 0..index_count,
        }],
        joint_zones: vec![None],
        ..LoadedModel::default()
    };
    let path = std::path::Path::new(handle);
    assert!(
        renderer
            .upload_parsed_skinned_model(handle, path, &Ok(model), path)
            .is_some(),
        "{handle} uploads"
    );
}

fn install_skinned_models(renderer: &mut Renderer) {
    install_skinned_model(renderer, SKINNED_NEAR_AND_FAR, &[near_quad(), far_quad()]);
    install_skinned_model(renderer, SKINNED_NEAR_ONLY, &[near_quad()]);
    install_skinned_model(renderer, SKINNED_FAR_ONLY, &[far_quad()]);
    install_skinned_model(renderer, SKINNED_NEAR_GHOST, &near_ghost_quads());
    install_skinned_model(renderer, SKINNED_FAR_GHOST, &far_ghost_quads());
    install_skinned_model(renderer, SKINNED_PAWN, &[pawn_quad()]);
}

fn mover_ghost(mover_id: u32, look: GhostInstanceLook) -> KinematicGhostInstance {
    KinematicGhostInstance {
        mover_id,
        transform: Mat4::IDENTITY,
        look,
    }
}

/// A ghost of the skinned model `handle` at the origin, holder `holder`
/// through link 0, posed rigidly.
fn skinned_ghost(handle: &str, holder: u32, look: GhostInstanceLook) -> MeshInstanceInput {
    MeshInstanceInput {
        model: postretro_model::ModelHandle::from(handle),
        transform: Mat4::IDENTITY,
        shadow_bias_scale: 1.0,
        phase_seed: holder,
        palette_cache_key: MeshPaletteCacheKey::Ghost {
            link: 0,
            holder,
            attachment_index: None,
        },
        sample: MeshSampleParams::rigid(),
        pose_inputs: None,
        capture: None,
        resample: true,
        forward_visible: true,
        dynamic_shadow_visible: false,
        presentation: MeshPresentation::Ghost(look),
    }
}

/// A real skinned mesh far off screen. It plans ahead of the ghost plan, so
/// every skinned ghost instance sits at a nonzero instance offset, and it
/// draws no pixel.
fn off_screen_world_mesh() -> MeshInstanceInput {
    MeshInstanceInput {
        model: postretro_model::ModelHandle::from(SKINNED_NEAR_ONLY),
        transform: Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0)),
        shadow_bias_scale: 1.0,
        phase_seed: 0,
        palette_cache_key: MeshPaletteCacheKey::Entity(99),
        sample: MeshSampleParams::rigid(),
        pose_inputs: None,
        capture: None,
        resample: true,
        forward_visible: true,
        dynamic_shadow_visible: false,
        presentation: MeshPresentation::World,
    }
}

/// One captured frame with exactly these skinned and mover ghosts.
fn frame_with(
    renderer: &mut Renderer,
    skinned: &[MeshInstanceInput],
    movers: &[KinematicGhostInstance],
) -> Vec<u8> {
    renderer.set_ghost_mesh_draws(skinned);
    renderer.set_kinematic_ghost_draws(movers);
    assert_eq!(
        renderer.full().kinematic_ghost.has_draws(),
        !movers.is_empty()
    );
    capture_from(renderer, CAMERA)
}

/// One captured frame with `ghost`'s mover drawn as a ghost at the origin, or
/// no ghost.
fn ghost_frame(renderer: &mut Renderer, ghost: Option<u32>) -> Vec<u8> {
    let movers: Vec<KinematicGhostInstance> = ghost
        .into_iter()
        .map(|mover_id| mover_ghost(mover_id, LOOK))
        .collect();
    frame_with(renderer, &[], &movers)
}

/// One captured frame with the skinned model `ghost` drawn as a ghost at the
/// origin, or no ghost.
fn skinned_ghost_frame(renderer: &mut Renderer, ghost: Option<&str>) -> Vec<u8> {
    let skinned: Vec<MeshInstanceInput> = ghost
        .into_iter()
        .map(|handle| skinned_ghost(handle, 1, LOOK))
        .collect();
    frame_with(renderer, &skinned, &[])
}

fn pixel(frame: &[u8], column: u32, row: u32) -> [u8; 4] {
    frame.as_chunks::<4>().0[(row * SIDE + column) as usize]
}

/// Within one 8-bit step on every channel.
fn same(a: [u8; 4], b: [u8; 4]) -> bool {
    (0..3).all(|channel| a[channel].abs_diff(b[channel]) <= 1)
}

/// The opacity a ghost shader decodes from `look`.
fn drawn_opacity(look: GhostInstanceLook) -> f32 {
    GhostInstanceLook::unpack(look.pack()).opacity
}

/// Whether the screen door keeps pixel (`column`, `row`) at `look`.
fn screen_door_keeps(look: GhostInstanceLook, column: u32, row: u32) -> bool {
    drawn_opacity(look) > screen_door_threshold(column, row)
}

/// The linear colour one ghost surface of `colour` at `look` leaves over
/// `under` at pixel (`column`, `row`) under `mode`.
fn over(
    mode: GhostTransparency,
    look: GhostInstanceLook,
    colour: Vec3,
    under: Vec3,
    column: u32,
    row: u32,
) -> Vec3 {
    match mode {
        GhostTransparency::Blend => {
            let opacity = drawn_opacity(look);
            colour * opacity + under * (1.0 - opacity)
        }
        GhostTransparency::Dither => {
            if screen_door_keeps(look, column, row) {
                colour
            } else {
                under
            }
        }
    }
}

/// The self-overlap frames of one ghost path under `mode`: no ghost, the
/// doubled ghost, and each of its surfaces alone. Where the near surface
/// covers the far one the doubled ghost matches the near surface alone; where
/// nothing nearer covers the far one it matches the far surface alone. Every
/// overlap pixel of the near surface draws: in blend each differs from the
/// ghost-free frame, and in dither exactly the screen door's pixels do.
fn assert_each_pixel_shows_only_the_nearest_surface(
    path: &str,
    mode: GhostTransparency,
    none: &[u8],
    both: &[u8],
    near: &[u8],
    far: &[u8],
) {
    let overlap: Vec<(u32, u32)> = ROWS
        .flat_map(|row| OVERLAP_COLUMNS.map(move |column| (column, row)))
        .collect();
    // The two surfaces read differently, so the equalities below can fail.
    assert!(
        overlap
            .iter()
            .any(|&(c, r)| !same(pixel(near, c, r), pixel(far, c, r))),
        "{path} {mode:?}: the near and far surfaces are distinguishable"
    );

    // Regression: an `Equal` test that is not exact loses kept pixels, the
    // same speckle in every frame, which the equalities below cannot see.
    match mode {
        GhostTransparency::Blend => {
            for &(column, row) in &overlap {
                assert!(
                    !same(pixel(near, column, row), pixel(none, column, row)),
                    "{path} {mode:?} ({column}, {row}): the near surface draws, got {:?}",
                    pixel(near, column, row),
                );
            }
        }
        GhostTransparency::Dither => {
            let mut kept = 0usize;
            for &(column, row) in &overlap {
                let drawn = !same(pixel(near, column, row), pixel(none, column, row));
                assert_eq!(
                    drawn,
                    screen_door_keeps(LOOK, column, row),
                    "{path} {mode:?} ({column}, {row}): the near surface draws exactly \
                     where the screen door keeps the pixel"
                );
                kept += usize::from(drawn);
            }
            let share = screen_door_coverage(LOOK.opacity) * overlap.len() as f32;
            assert!(
                (kept as f32 - share).abs() <= TILE_EDGE_SLACK,
                "{path} {mode:?}: {kept} of {} overlap pixels kept, the screen door's \
                 share is {share}",
                overlap.len()
            );
        }
    }

    for &(column, row) in &overlap {
        assert!(
            same(pixel(both, column, row), pixel(near, column, row)),
            "{path} {mode:?} ({column}, {row}): the overlap shows the near surface alone, \
             got {:?} vs near {:?}",
            pixel(both, column, row),
            pixel(near, column, row),
        );
    }
    for row in ROWS {
        for column in FAR_ONLY_COLUMNS {
            assert!(
                same(pixel(both, column, row), pixel(far, column, row)),
                "{path} {mode:?} ({column}, {row}): the uncovered far surface still draws"
            );
        }
    }
}

// The owner playtest's teeth-through-lips defect, on a mover ghost: every
// pixel of a ghost shows only its nearest surface, in both modes.
#[test]
fn ghost_nearer_surface_hides_its_farther_one_in_both_modes() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_movers(&mut renderer);
    for mode in GhostTransparency::ALL {
        renderer.set_ghost_transparency(mode);
        let none = ghost_frame(&mut renderer, None);
        let both = ghost_frame(&mut renderer, Some(NEAR_AND_FAR));
        let near = ghost_frame(&mut renderer, Some(NEAR_ONLY));
        let far = ghost_frame(&mut renderer, Some(FAR_ONLY));
        assert_each_pixel_shows_only_the_nearest_surface("mover", mode, &none, &both, &near, &far);
    }
    eprintln!("[GhostDepthProof] mover self-overlap GPU case ran in both modes");
}

// The same defect on the skinned ghost path, the playtest's own (a rigged
// head's teeth through its lips): one rigid skinned model holding both
// surfaces draws only the nearer, in both modes.
#[test]
fn skinned_ghost_nearer_surface_hides_its_farther_one_in_both_modes() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_skinned_models(&mut renderer);
    for mode in GhostTransparency::ALL {
        renderer.set_ghost_transparency(mode);
        let none = skinned_ghost_frame(&mut renderer, None);
        let both = skinned_ghost_frame(&mut renderer, Some(SKINNED_NEAR_AND_FAR));
        let near = skinned_ghost_frame(&mut renderer, Some(SKINNED_NEAR_ONLY));
        let far = skinned_ghost_frame(&mut renderer, Some(SKINNED_FAR_ONLY));
        assert_each_pixel_shows_only_the_nearest_surface(
            "skinned", mode, &none, &both, &near, &far,
        );
    }
    eprintln!("[GhostDepthProof] skinned self-overlap GPU case ran in both modes");
}

/// The linear colour the two distinct ghosts leave at pixel (`column`, `row`)
/// over the white clear: each ghost's outer surface alone, the near ghost's
/// over the far ghost's where they overlap.
fn two_ghosts_expected(mode: GhostTransparency, column: u32, row: u32) -> Vec3 {
    let behind = if FAR_COLUMNS.contains(&column) {
        over(mode, FAR_LOOK, BLACK, WHITE, column, row)
    } else {
        WHITE
    };
    if NEAR_COLUMNS.contains(&column) {
        over(mode, NEAR_LOOK, MAGENTA, behind, column, row)
    } else {
        behind
    }
}

// Two distinct ghosts overlapping on screen, the nearer listed first: the
// farther draws first and shows through the nearer at the expected blend
// (or, dithered, where the nearer drops a pixel), and each still hides its
// own inner surface. Run as two movers and with either ghost skinned, so
// mover and skinned ghosts sort together and each kind's pipelines follow
// the other's in one pass. A real mesh ahead of the ghost plan puts every
// skinned ghost at a nonzero instance offset.
#[test]
fn a_nearer_ghost_blends_over_a_farther_one_and_each_hides_its_own_inner_surface() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_movers(&mut renderer);
    install_skinned_models(&mut renderer);
    let world = off_screen_world_mesh();
    let planned = plan_mesh_frame_plans(
        std::slice::from_ref(&world),
        &[skinned_ghost(SKINNED_FAR_GHOST, 1, FAR_LOOK)],
        &renderer.full().mesh_pass,
    );
    assert!(
        planned
            .ghost
            .groups
            .first()
            .is_some_and(|group| group.instance_offset > 0),
        "the world mesh plans ahead of the ghosts"
    );
    renderer.set_mesh_draws(std::slice::from_ref(&world));
    let far = mover_ghost(FAR_GHOST, FAR_LOOK);
    for mode in GhostTransparency::ALL {
        renderer.set_ghost_transparency(mode);
        for (path, frame) in [
            (
                "mover over mover",
                frame_with(
                    &mut renderer,
                    &[],
                    &[mover_ghost(NEAR_GHOST, NEAR_LOOK), far],
                ),
            ),
            (
                "skinned over mover",
                frame_with(
                    &mut renderer,
                    &[skinned_ghost(SKINNED_NEAR_GHOST, 1, NEAR_LOOK)],
                    &[far],
                ),
            ),
            (
                "mover over skinned",
                frame_with(
                    &mut renderer,
                    &[skinned_ghost(SKINNED_FAR_GHOST, 1, FAR_LOOK)],
                    &[mover_ghost(NEAR_GHOST, NEAR_LOOK)],
                ),
            ),
        ] {
            for row in ROWS {
                for column in 0..SIDE {
                    let want = capture_encode(two_ghosts_expected(mode, column, row));
                    let got = pixel(&frame, column, row);
                    assert!(
                        (0..3).all(|channel| got[channel].abs_diff(want[channel]) <= 1),
                        "{path} {mode:?} ({column}, {row}): got {got:?}, want {want:?}"
                    );
                }
            }
        }
    }
    eprintln!("[GhostDepthProof] two-ghost GPU case ran in both modes, mover and skinned");
}

/// The linear colour the platform and the pawn on it leave at pixel
/// (`column`, `row`) over the white clear: the pawn over the platform where
/// they overlap.
fn platform_and_pawn_expected(mode: GhostTransparency, column: u32, row: u32) -> Vec3 {
    let behind = if PLATFORM_COLUMNS.contains(&column) {
        over(mode, FAR_LOOK, MAGENTA, WHITE, column, row)
    } else {
        WHITE
    };
    if PAWN_COLUMNS.contains(&column) {
        over(mode, NEAR_LOOK, BLACK, behind, column, row)
    } else {
        behind
    }
}

// A ghost pawn standing in front of the far half of a long ghost platform,
// its bounds centre behind the platform's. The platform keys on its farthest
// bounds corner, so it draws first and shows through the pawn's silhouette
// at the expected blend. Keyed by its centre it would draw after the pawn
// and lose the pawn's whole silhouette, a pawn-shaped hole. Blend only:
// dither keeps the nearest pixel in either order.
#[test]
fn a_ghost_platform_shows_through_the_ghost_pawn_on_its_far_half() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_movers(&mut renderer);
    install_skinned_models(&mut renderer);
    // The pawn keys on its bounds centre, behind the platform's (0.5).
    let pawn_bounds = renderer.skinned_model_local_bounds(SKINNED_PAWN);
    assert!(((pawn_bounds.min.z + pawn_bounds.max.z) * 0.5 - PAWN_DEPTH).abs() < 1e-6);
    let mode = GhostTransparency::Blend;
    renderer.set_ghost_transparency(mode);
    let frame = frame_with(
        &mut renderer,
        &[skinned_ghost(SKINNED_PAWN, 1, NEAR_LOOK)],
        &[mover_ghost(PLATFORM, FAR_LOOK)],
    );
    for row in ROWS {
        for column in 0..SIDE {
            let want = capture_encode(platform_and_pawn_expected(mode, column, row));
            let got = pixel(&frame, column, row);
            assert!(
                (0..3).all(|channel| got[channel].abs_diff(want[channel]) <= 1),
                "({column}, {row}): got {got:?}, want {want:?}"
            );
        }
    }
    eprintln!("[GhostDepthProof] ghost platform GPU case ran in blend");
}

// The ghost pass never writes scene depth: an opaque fluid surface behind a
// ghost, drawn after it, covers it exactly as if no ghost were there. A ghost
// that wrote scene depth would fail the fluid's depth test across its
// footprint and show through.
#[test]
fn a_fluid_surface_behind_a_ghost_still_draws_over_it() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_movers(&mut renderer);
    let bare = ghost_frame(&mut renderer, None);
    let ghost_only = ghost_frame(&mut renderer, Some(NEAR_ONLY));
    assert_ne!(bare, ghost_only, "without the fluid the ghost shows");

    // Full screen, clip depth 0.5: behind the ghost's 0.3, opaque.
    renderer.install_fluid_surfaces(&[fluid_quad(-1.0, 1.0)], None);
    renderer.set_fluid_surface_draws(&[look_draw(0, FluidSurfaceLook::surface_only(1.0))]);
    for mode in GhostTransparency::ALL {
        renderer.set_ghost_transparency(mode);
        let fluid_only = ghost_frame(&mut renderer, None);
        assert_ne!(fluid_only, bare, "{mode:?}: the fluid draws");
        let fluid_over_ghost = ghost_frame(&mut renderer, Some(NEAR_ONLY));
        assert_eq!(
            fluid_over_ghost, fluid_only,
            "{mode:?}: the fluid behind the ghost draws over its whole footprint"
        );
    }
    eprintln!("[GhostDepthProof] fluid-behind-ghost GPU case ran in both modes");
}
