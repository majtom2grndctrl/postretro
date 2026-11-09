//! C9, renderer half, on the compiled `mover-closet`, whole-loaded: the door's
//! t′ sequence the game half hands over (rising, a reversal at mid-travel,
//! closing, held), each frame composing the indirect atlas and writing the
//! door's live depth tiles. At every t′ the composed atlas and the tile
//! texture equal, texel by texel, the linear mix of the frames at t′ = 0 and
//! t′ = 1: the affected probes compose lerp(S^b₀, S^b₁, t′), their live tiles
//! hold lerp(D₀, D₁, t′), and every other texel holds still. The closet gates
//! no direct light (its id 57 has no direct block), so no direct atlas moves.
//! See: context/lib/rendering_pipeline.md §4 (Mover state lighting)
use super::*;
use crate::render::MoverLightState;

/// The door's t′ per frame: first hand-over at 0, rising, held across two
/// frames, reversed at mid-travel, closing, then held at 0.
const SEQUENCE: [f32; 9] = [0.0, 0.25, 0.5, 0.5, 0.375, 0.25, 0.125, 0.0, 0.0];

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
        self.renderer.set_mover_light_states(&[MoverLightState {
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
        let atlas = read_halves(
            &self.renderer,
            &level.sh_volume_resources.total_atlas_texture,
            8,
        );
        let tiles = read_halves(
            &self.renderer,
            level.sh_volume_resources.depth_tiles.texture(),
            4,
        );
        (atlas, tiles)
    }
}

/// Every half of a 16-bit-float texture, all layers, de-padded, as f32.
fn read_halves(renderer: &Renderer, texture: &wgpu::Texture, texel_bytes: u32) -> Vec<f32> {
    let device = &renderer.device;
    let extent = texture.size();
    let row = extent.width * texel_bytes;
    let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("closet channel readback"),
        size: u64::from(padded * extent.height * extent.depth_or_array_layers),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(extent.height),
            },
        },
        extent,
    );
    renderer
        .queue
        .submit_unbatched([encoder.finish()], "closet channel readback");
    let (send, receive) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            send.send(result).unwrap()
        });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    receive.recv().unwrap().unwrap();
    let bytes = buffer.slice(..).get_mapped_range().unwrap().to_vec();
    buffer.unmap();
    bytes
        .chunks(padded as usize)
        .flat_map(|line| line[..row as usize].as_chunks::<2>().0.iter().copied())
        .map(|half| f16_to_f32(u16::from_le_bytes(half)))
        .collect()
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24),
        31 => f32::INFINITY,
        _ => (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}

/// Texels that differ between the termini, and the worst miss of `found`
/// against the mix at `t` in units of the texel's f16 spacing.
fn check_mix(found: &[f32], at0: &[f32], at1: &[f32], t: f32, what: &str) -> usize {
    assert_eq!(found.len(), at0.len(), "{what}: same layout");
    let mut moving = 0;
    for (index, ((&got, &a), &b)) in found.iter().zip(at0).zip(at1).enumerate() {
        if a != b {
            moving += 1;
        } else {
            assert_eq!(
                got.to_bits(),
                a.to_bits(),
                "{what} t′ {t}: half {index} no terminus moves changed"
            );
            continue;
        }
        let expected = a + (b - a) * t;
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
    let (atlas_closed, tiles_closed) = closet.frame(0.0);
    let mut moving = (0, 0);
    for t in SEQUENCE {
        let (atlas, tiles) = closet.frame(t);
        moving = (
            check_mix(&atlas, &atlas_closed, &atlas_open, t, "indirect atlas"),
            check_mix(&tiles, &tiles_closed, &tiles_open, t, "depth tiles"),
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
