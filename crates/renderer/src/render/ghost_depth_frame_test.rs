//! Adapter-backed proof of the ghost depth pre-pass through the production
//! capture recorder: a ghost's nearer surface hides its farther one under both
//! `render.ghosts` modes, while a farther surface nothing covers still draws;
//! and a fluid surface behind a ghost still draws over it, so the ghost passes
//! leave scene depth to the opaque scene.
//! See: context/lib/rendering_pipeline.md §7.9

use glam::{Mat4, Vec3};
use postretro_level_format::geometry::Vertex;
use postretro_render_cpu::fluid_surface::FluidSurfaceLook;
use postretro_render_cpu::ghost_instance::{GhostInstanceLook, KinematicGhostInstance};
use postretro_render_cpu::ghost_transparency::GhostTransparency;

use super::Renderer;
use super::fluid_surface_frame_test::{SIDE, capture_from, fluid_quad, look_draw, renderer};

/// One mover holding a near quad and, overlapping it on screen, a far one:
/// the self-overlap a closed mouth over teeth makes.
const NEAR_AND_FAR: u32 = 1;
const NEAR_ONLY: u32 = 2;
const FAR_ONLY: u32 = 3;

/// Clip depths under the identity view-projection; the fluid quad sits at 0.5.
const NEAR_DEPTH: f32 = 0.3;
const FAR_DEPTH: f32 = 0.6;

/// Texel centres of the 64-texel placeholder checkerboard (8-texel squares,
/// magenta at the origin): one magenta square, one black.
const MAGENTA_UV: [f32; 2] = [4.5 / 64.0, 4.5 / 64.0];
const BLACK_UV: [f32; 2] = [12.5 / 64.0, 4.5 / 64.0];

/// Columns where the near quad (clip x −0.75..0.25) overlaps the far quad
/// (−0.25..0.75), and where only the far quad lies; rows inside both quads'
/// clip y −0.75..0.75. Pixel centres sit at −1 + (i + 0.5) / 8.
const OVERLAP_COLUMNS: std::ops::Range<u32> = 6..10;
const FAR_ONLY_COLUMNS: std::ops::Range<u32> = 10..14;
const ROWS: std::ops::Range<u32> = 3..13;

const CAMERA: Vec3 = Vec3::new(0.0, 0.0, -1.0);

const LOOK: GhostInstanceLook = GhostInstanceLook {
    opacity: 0.5,
    saturation: 1.0,
    tint: [1.0, 1.0, 1.0],
};

/// A quad facing the identity camera (counter-clockwise in clip space) over
/// clip x `x0..x1`, y −0.75..0.75, at clip depth `z`, with one flat UV.
fn quad(x0: f32, x1: f32, z: f32, uv: [f32; 2]) -> [Vertex; 4] {
    let vertex = |x: f32, y: f32| {
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
        vertex(x0, -0.75),
        vertex(x1, -0.75),
        vertex(x1, 0.75),
        vertex(x0, 0.75),
    ]
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

/// Install the three movers. The doubled one draws its near quad first, so a
/// pass that let the far quad through would show it over the near one under
/// dither and under it twice-blended under blend.
fn install_movers(renderer: &mut Renderer) {
    let near = quad(-0.75, 0.25, NEAR_DEPTH, MAGENTA_UV);
    let far = quad(-0.25, 0.75, FAR_DEPTH, BLACK_UV);
    let geometry = postretro_level_loader::KinematicGeometry {
        movers: vec![
            mover(NEAR_AND_FAR, &[near.clone(), far.clone()]),
            mover(NEAR_ONLY, &[near]),
            mover(FAR_ONLY, &[far]),
        ],
        waypoints: Vec::new(),
    };
    let device = renderer.device.clone();
    renderer
        .full_mut()
        .kinematic_brush
        .install_geometry(&device, Some(&geometry), 1);
}

/// One captured frame with `ghost`'s mover drawn as a ghost at the origin, or
/// no ghost.
fn ghost_frame(renderer: &mut Renderer, ghost: Option<u32>) -> Vec<u8> {
    let instances: Vec<KinematicGhostInstance> = ghost
        .into_iter()
        .map(|mover_id| KinematicGhostInstance {
            mover_id,
            transform: Mat4::IDENTITY,
            look: LOOK,
        })
        .collect();
    renderer.set_kinematic_ghost_draws(&instances);
    assert_eq!(renderer.full().kinematic_ghost.has_draws(), ghost.is_some());
    capture_from(renderer, CAMERA)
}

fn pixel(frame: &[u8], column: u32, row: u32) -> [u8; 4] {
    frame.as_chunks::<4>().0[(row * SIDE + column) as usize]
}

/// Within one 8-bit step on every channel.
fn same(a: [u8; 4], b: [u8; 4]) -> bool {
    (0..3).all(|channel| a[channel].abs_diff(b[channel]) <= 1)
}

// The owner playtest's teeth-through-lips defect: every pixel of a ghost shows
// only its nearest surface. Where the near quad covers the far one, the
// doubled mover's ghost matches the near quad's ghost alone, in both modes;
// where nothing nearer covers the far quad, it matches the far quad's ghost.
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

        // The two surfaces read differently and the ghost draws at all, so
        // the equalities below can fail.
        let overlap: Vec<(u32, u32)> = ROWS
            .flat_map(|row| OVERLAP_COLUMNS.map(move |column| (column, row)))
            .collect();
        assert!(
            overlap
                .iter()
                .any(|&(c, r)| !same(pixel(&near, c, r), pixel(&far, c, r))),
            "{mode:?}: the near and far surfaces are distinguishable"
        );
        assert!(
            overlap
                .iter()
                .any(|&(c, r)| !same(pixel(&near, c, r), pixel(&none, c, r))),
            "{mode:?}: the near surface's ghost draws"
        );

        for &(column, row) in &overlap {
            assert!(
                same(pixel(&both, column, row), pixel(&near, column, row)),
                "{mode:?} ({column}, {row}): the overlap shows the near surface alone, \
                 got {:?} vs near {:?}",
                pixel(&both, column, row),
                pixel(&near, column, row),
            );
        }
        for row in ROWS {
            for column in FAR_ONLY_COLUMNS {
                assert!(
                    same(pixel(&both, column, row), pixel(&far, column, row)),
                    "{mode:?} ({column}, {row}): the uncovered far surface still draws"
                );
            }
        }
    }
    eprintln!("[GhostDepthProof] self-overlap GPU case ran in both modes");
}

// The ghost passes never write scene depth: an opaque fluid surface behind a
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
