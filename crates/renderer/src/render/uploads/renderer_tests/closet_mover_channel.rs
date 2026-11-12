//! Renderer half, on the compiled `mover-closet`, whole-loaded: the door's
//! t′ sequence the game half hands over (rising, a reversal just past
//! mid-travel, closing, held), each frame composing the indirect atlas and
//! writing the door's live depth tiles. At every t′ the composed atlas and
//! the tile texture equal, texel by texel, the piecewise mix of reference
//! frames rendered at every t′ where some probe's compose can bend: each of
//! the door's knots, where a stored or base-reading pose reads exactly, and
//! the midpoint of every pair of knots, where a depth interval between the
//! two termini reaches its base. Between two neighbouring references every
//! probe's compose is linear in t′: bounce lerps between the poses around
//! t′ (a skipped pose blended across); depth between the two termini runs
//! its raw moments D_lo → base → D_hi, and depth on an interval a mid pose
//! bounds lerps the mean and the variance, rebuilding `E[d²]` from the
//! mean. So the mix of the neighbouring references is that probe's
//! piecewise mix, whichever poses it stores: in the raw moments, or in the
//! mean and the variance. Every other texel holds still. The closet gates
//! no direct light (its id 57 has no direct block), so no direct atlas
//! moves.
//! See: context/lib/rendering_pipeline.md §4 (Mover state lighting)
use super::*;
use crate::render::MoverTravel;
use crate::render::composed_tiles::{f16_to_f32, read_halves};
use postretro_level_format::mover_probe_channel::Knot;

/// The door's t′ per frame: first hand-over at 0, rising through the
/// midpoint, held across two frames, reversed just past mid-travel, closing
/// through the midpoint, then held at 0.
const SEQUENCE: [f32; 10] = [0.0, 0.25, 0.5, 0.625, 0.625, 0.5, 0.375, 0.125, 0.0, 0.0];

struct Closet {
    renderer: Renderer,
    font: postretro_ui::text::FontSystem,
    door: u32,
    /// The door's knots through its ramp.
    knots: Vec<Knot>,
}

impl Closet {
    fn load(path: &str) -> Option<Self> {
        let mut renderer = renderer()?;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev");
        let mut world = postretro_level_loader::load_prl(path).unwrap();
        assert!(
            world.sh_stream_manifest().is_none(),
            "run with POSTRETRO_SH_STREAMING=off: this row reads the whole-load atlas"
        );
        let channel = world.mover_probe_channel().expect("the closet ships id 57");
        assert!(
            channel.direct_records.is_empty() && !channel.bounce_entries.is_empty(),
            "the closet's id 57 carries bounce and no direct record"
        );
        assert!(!channel.depth_records.is_empty(), "and a depth block");
        let door = world.kinematic_geometry.movers[0].mover_id;
        let knots = channel
            .mover_record(door)
            .expect("the door has a mover record")
            .effective_knots();
        let materials: Vec<_> = world
            .texture_names
            .iter()
            .map(|_| postretro_render_data::material::Material::Default)
            .collect();
        renderer.install_textures(
            &world.texture_names,
            &world.texture_cache_keys,
            &root.join("cache"),
            &materials,
        );
        renderer.normalize_world_uvs(&mut world);
        let lighting = world.take_gpu_lighting_payloads();
        renderer
            .install_level_geometry(
                &crate::render::level_world_to_geometry(&world, &materials),
                lighting,
            )
            .expect("level resources install");
        Some(Self {
            renderer,
            font: postretro_ui::text::build_font_system(),
            door,
            knots,
        })
    }

    /// One recorded frame with the door's state at `t`, then both readbacks.
    fn frame(&mut self, t: f32) -> (Vec<f32>, Vec<f32>) {
        self.renderer.set_mover_travel(&[MoverTravel {
            mover_id: self.door,
            t,
        }]);
        self.renderer
            .drain_sh_residency(ShDrainBatch::default())
            .expect("whole-load drain");
        self.renderer
            .update_per_frame_uniforms(Mat4::IDENTITY, Vec3::ZERO, 0.0);
        self.renderer
            .update_viewmodel_view_projection(1.0, Mat4::IDENTITY);
        let encoder = record_window(&mut self.renderer, &mut self.font);
        self.renderer.submit_windowed_frame(encoder);
        self.renderer
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let level = self.renderer.level();
        let atlas = read_values(
            &self.renderer,
            &level.sh_volume_resources.total_atlas_texture,
        );
        let tiles = read_values(
            &self.renderer,
            level.sh_volume_resources.depth_tiles.texture(),
        );
        (atlas, tiles)
    }
}

/// Every half of a 16-bit-float texture, all layers, de-padded, as f32.
fn read_values(renderer: &Renderer, texture: &wgpu::Texture) -> Vec<f32> {
    read_halves(renderer, texture)
        .into_iter()
        .map(f16_to_f32)
        .collect()
}

/// The t′ where some probe's compose can bend, ascending: every knot, and
/// the midpoint of every pair of knots, since any two may bound one probe's
/// interval once the poses between them are skipped for it.
fn breakpoints(knots: &[Knot]) -> Vec<f32> {
    let mut at: Vec<f32> = knots.iter().map(|knot| knot.t).collect();
    for (i, lo) in knots.iter().enumerate() {
        for hi in &knots[i + 1..] {
            at.push(0.5 * (lo.t + hi.t));
        }
    }
    at.sort_by(f32::total_cmp);
    at.dedup();
    at
}

/// One reference frame: its t′ and both readbacks.
struct Reference {
    t: f32,
    atlas: Vec<f32>,
    tiles: Vec<f32>,
}

/// How a readback's halves mix between references.
#[derive(Clone, Copy, PartialEq)]
enum Halves {
    /// Every half lerps: the indirect atlas.
    Linear,
    /// `(E[d], E[d²])` pairs: the mean lerps, and `E[d²]` lerps either raw
    /// (between the termini) or as the variance, rebuilt from the mean.
    Moments,
}

/// Texels that move with t′, each checked against the mix of the two
/// references around `t` (`refs` ascending by t′, `pick` the readback),
/// within one f16 step at the larger magnitude: the store's rounding, plus
/// at most half a step a reference carries where it is itself a mix. A
/// depth texel's `E[d²]` passes as the raw mix or as the variance mix
/// (`max(0, E[d²] − E[d]²)` at each reference, lerped, plus the stored
/// mean squared) within two steps, since each reference's variance carries
/// its own `E[d²]` rounding. A texel every reference holds alike must hold
/// still. Returns the moving halves and, of those, the `E[d²]` halves only
/// the variance mix fits.
fn check_mix(
    found: &[f32],
    refs: &[Reference],
    pick: impl Fn(&Reference) -> &[f32],
    halves: Halves,
    t: f32,
    what: &str,
) -> (usize, usize) {
    let hi = refs
        .iter()
        .position(|reference| reference.t >= t)
        .expect("references span t′ 0 to 1");
    let lo = if refs[hi].t == t { hi } else { hi - 1 };
    let s = if lo == hi {
        0.0
    } else {
        (t - refs[lo].t) / (refs[hi].t - refs[lo].t)
    };
    let (below, above) = (pick(&refs[lo]), pick(&refs[hi]));
    assert_eq!(found.len(), below.len(), "{what}: same layout");
    let mut moving = 0;
    let mut by_variance = 0;
    for (index, &got) in found.iter().enumerate() {
        let first = pick(&refs[0])[index];
        if refs.iter().all(|reference| pick(reference)[index] == first) {
            assert_eq!(
                got.to_bits(),
                first.to_bits(),
                "{what} t′ {t}: half {index} no reference moves changed"
            );
            continue;
        }
        moving += 1;
        let (a, b) = (below[index], above[index]);
        let expected = a + (b - a) * s;
        let step = a.abs().max(b.abs()).max(6.1e-5) * 2f32.powi(-10);
        if (got - expected).abs() <= 1.01 * step {
            continue;
        }
        let rebuilt = (halves == Halves::Moments && index % 2 == 1).then(|| {
            let variance = |mean_sq: f32, mean: f32| (mean_sq - mean * mean).max(0.0);
            let (below_var, above_var) =
                (variance(a, below[index - 1]), variance(b, above[index - 1]));
            let mean = found[index - 1];
            (below_var + (above_var - below_var) * s + mean * mean).min(65504.0)
        });
        assert!(
            rebuilt.is_some_and(|rebuilt| (got - rebuilt).abs() <= 2.01 * step),
            "{what} t′ {t}: half {index} holds {got}, the mix of {a} (t′ {}) and {b} (t′ {}) is \
             {expected}, the variance mix {rebuilt:?}",
            refs[lo].t,
            refs[hi].t,
        );
        by_variance += 1;
    }
    (moving, by_variance)
}

#[test]
#[ignore = "requires MOVER_CLOSET_PRL (content/dev/maps/mover-closet.map compiled) and \
            POSTRETRO_SH_STREAMING=off"]
fn c9_the_closet_composes_and_writes_the_piecewise_mix_at_every_handed_t() {
    let path = std::env::var("MOVER_CLOSET_PRL").expect("set MOVER_CLOSET_PRL");
    let Some(mut closet) = Closet::load(&path) else {
        return;
    };
    let at = breakpoints(&closet.knots);
    let refs: Vec<Reference> = at
        .iter()
        .map(|&t| {
            let (atlas, tiles) = closet.frame(t);
            Reference { t, atlas, tiles }
        })
        .collect();
    let (mut atlas_moving, mut tiles_moving, mut by_variance) = (0, 0, 0);
    for t in SEQUENCE {
        let (atlas, tiles) = closet.frame(t);
        (atlas_moving, _) = check_mix(
            &atlas,
            &refs,
            |r| &r.atlas,
            Halves::Linear,
            t,
            "indirect atlas",
        );
        let (moving, variance) = check_mix(
            &tiles,
            &refs,
            |r| &r.tiles,
            Halves::Moments,
            t,
            "depth tiles",
        );
        tiles_moving = moving;
        by_variance += variance;
    }
    eprintln!(
        "C9 closet: {} knot(s) {:?}, {} reference frame(s); {atlas_moving} indirect-atlas halves \
         and {tiles_moving} depth-tile halves move with t′, every one at its piecewise mix over \
         {} frames ({by_variance} E[d²] half-frames fit only the variance mix)",
        closet.knots.len(),
        closet.knots.iter().map(|knot| knot.t).collect::<Vec<_>>(),
        refs.len(),
        SEQUENCE.len()
    );
    assert!(
        atlas_moving > 0,
        "the door's bounce moves the indirect atlas"
    );
    assert!(tiles_moving > 0, "the door's live tiles move");
}
