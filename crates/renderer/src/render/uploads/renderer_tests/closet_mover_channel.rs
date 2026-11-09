//! Renderer half, on the compiled `mover-closet`, whole-loaded: the door's
//! t′ sequence the game half hands over (rising, a reversal just past
//! mid-travel, closing, held), each frame composing the indirect atlas and
//! writing the door's live depth tiles. At every t′ the composed atlas
//! equals, texel by texel, the linear mix of the frames at t′ = 0 and t′ = 1:
//! the affected probes compose lerp(S^b₀, S^b₁, t′). The tile texture blends
//! through the frame at t′ = 0.5, where each live tile holds its probe's base
//! (the D-rows prove that bit for bit): lerp(D₀, base, 2t′) up to 0.5, then
//! lerp(base, D₁, 2t′ − 1). Every other texel holds still. The closet gates
//! no direct light (its id 57 has no direct block), so no direct atlas moves.
//! See: context/lib/rendering_pipeline.md §4 (Mover state lighting)
use super::*;
use crate::render::MoverTravel;
use crate::render::composed_tiles::{f16_to_f32, read_halves};

/// The door's t′ per frame: first hand-over at 0, rising through the
/// midpoint, held across two frames, reversed just past mid-travel, closing
/// through the midpoint, then held at 0.
const SEQUENCE: [f32; 10] = [0.0, 0.25, 0.5, 0.625, 0.625, 0.5, 0.375, 0.125, 0.0, 0.0];

struct Closet {
    renderer: Renderer,
    font: postretro_ui::text::FontSystem,
    door: u32,
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

/// Texels that move with t′, each checked against its mix at `t` within the
/// texel's f16 spacing: the linear mix of `at0` and `at1`, or with `mid`,
/// the blend through it (`at0` → `mid` over the first half, `mid` → `at1`
/// over the second). A texel every reference holds alike must hold still.
fn check_mix(
    found: &[f32],
    at0: &[f32],
    mid: Option<&[f32]>,
    at1: &[f32],
    t: f32,
    what: &str,
) -> usize {
    assert_eq!(found.len(), at0.len(), "{what}: same layout");
    let mut moving = 0;
    for (index, ((&got, &a0), &b1)) in found.iter().zip(at0).zip(at1).enumerate() {
        let (a, b, s) = match mid.map(|mid| mid[index]) {
            Some(m) if t <= 0.5 => (a0, m, 2.0 * t),
            Some(m) => (m, b1, 2.0 * t - 1.0),
            None => (a0, b1, t),
        };
        let still = a0 == b1 && mid.is_none_or(|mid| mid[index] == a0);
        if !still {
            moving += 1;
        } else {
            assert_eq!(
                got.to_bits(),
                a0.to_bits(),
                "{what} t′ {t}: half {index} no reference moves changed"
            );
            continue;
        }
        let expected = a + (b - a) * s;
        // One f16 step at the larger magnitude, for the store's rounding,
        // plus the mix's own f32 rounding.
        let step = a.abs().max(b.abs()).max(6.1e-5) * 2f32.powi(-10);
        assert!(
            (got - expected).abs() <= 1.01 * step,
            "{what} t′ {t}: half {index} holds {got}, the mix of {a} and {b} is {expected}"
        );
    }
    moving
}

#[test]
#[ignore = "requires MOVER_CLOSET_PRL (content/dev/maps/mover-closet.map compiled) and \
            POSTRETRO_SH_STREAMING=off"]
fn c9_the_closet_composes_and_writes_the_mix_at_every_handed_t() {
    let path = std::env::var("MOVER_CLOSET_PRL").expect("set MOVER_CLOSET_PRL");
    let Some(mut closet) = Closet::load(&path) else {
        return;
    };
    let (atlas_open, tiles_open) = closet.frame(1.0);
    let (_, tiles_base) = closet.frame(0.5);
    let (atlas_closed, tiles_closed) = closet.frame(0.0);
    let mut moving = (0, 0);
    for t in SEQUENCE {
        let (atlas, tiles) = closet.frame(t);
        moving = (
            check_mix(
                &atlas,
                &atlas_closed,
                None,
                &atlas_open,
                t,
                "indirect atlas",
            ),
            check_mix(
                &tiles,
                &tiles_closed,
                Some(&tiles_base),
                &tiles_open,
                t,
                "depth tiles",
            ),
        );
    }
    eprintln!(
        "C9 closet: {} indirect-atlas halves and {} depth-tile halves move with t′, \
         every one at the mix over {} frames",
        moving.0,
        moving.1,
        SEQUENCE.len()
    );
    assert!(moving.0 > 0, "the door's bounce moves the indirect atlas");
    assert!(moving.1 > 0, "the door's live tiles move");
}
