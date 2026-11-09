//! Test-only readback of the composed SH atlases by probe. A whole load and a
//! streamed load store a probe's tiles at different slots, in atlases of
//! different shapes and tile strides. This reads each composed atlas whole and
//! cuts one probe's logical 6×6 tile out through that load's own indirection
//! word, so the two paths compare probe by probe.
//! See: context/lib/rendering_pipeline.md §4 (Cluster SH residency, Mover
//! state lighting)

use std::ops::Range;

use super::Renderer;
use super::sh_indirection::decode_probe_indirection_word;
use super::sh_volume::ShVolumeResources;
use postretro_render_cpu::sh_volume::LEGACY_SH_PHYSICAL_TILE_STRIDE;

/// The logical tile edge every SH atlas stores per stored slot.
pub(crate) const TILE_EDGE: u32 = 6;
/// f16 halves in one RGBA16F logical tile.
pub(crate) const TILE_HALVES: usize = (TILE_EDGE * TILE_EDGE * 4) as usize;
/// The composed atlases, in [`ComposedReadback::atlases`] order.
pub(crate) const ATLAS_NAMES: [&str; 3] = ["indirect", "direct promotion-free", "direct handoff"];

/// One composed atlas and the geometry that turns a slot into its texels.
#[derive(Clone, Copy)]
pub(crate) struct ComposedAtlas<'a> {
    pub(crate) texture: &'a wgpu::Texture,
    pub(crate) tiles_per_row: u32,
    pub(crate) tiles_per_layer: u32,
    pub(crate) stride: u32,
}

/// Every composed atlas of the installed level, and the words that place
/// each probe's tile in them.
pub(crate) struct ComposedAtlases<'a> {
    /// One indirection word per dense probe, as the compose passes read them.
    pub(crate) words: &'a [u32],
    pub(crate) indirect: ComposedAtlas<'a>,
    /// The promotion-free direct atlas, when the level composes one.
    pub(crate) direct: Option<ComposedAtlas<'a>>,
    /// The promotion handoff direct atlas, when the level composes one.
    pub(crate) handoff: Option<ComposedAtlas<'a>>,
}

impl<'a> ComposedAtlases<'a> {
    pub(crate) fn of(renderer: &'a Renderer) -> Self {
        let level = renderer.level();
        match &level.sh_streaming {
            Some(streaming) => streaming.composed_atlases(),
            None => Self::whole_load(&level.sh_volume_resources),
        }
    }

    fn whole_load(sh: &'a ShVolumeResources) -> Self {
        let indirect = ComposedAtlas {
            texture: &sh.total_atlas_texture,
            tiles_per_row: sh.atlas_tiles_per_row,
            tiles_per_layer: sh.tiles_per_layer,
            stride: LEGACY_SH_PHYSICAL_TILE_STRIDE,
        };
        let composed = sh.direct.atlas_storage_view.is_some();
        let direct_atlas = |view: &'a wgpu::TextureView| {
            let layout = sh
                .direct
                .compose_layout
                .expect("a composed direct atlas has a compose layout");
            ComposedAtlas {
                texture: view.texture(),
                tiles_per_row: layout.atlas_tiles_per_row,
                tiles_per_layer: layout.tiles_per_layer,
                stride: LEGACY_SH_PHYSICAL_TILE_STRIDE,
            }
        };
        Self {
            words: &sh.probe_indirection_words,
            indirect,
            direct: composed.then(|| direct_atlas(&sh.direct.atlas_view)),
            handoff: composed.then(|| direct_atlas(&sh.direct.handoff_atlas_view)),
        }
    }

    /// Reads every composed atlas back after the GPU drains.
    pub(crate) fn read(&self, renderer: &Renderer) -> ComposedReadback {
        ComposedReadback {
            words: self.words.to_vec(),
            atlases: [
                Some(read_atlas(renderer, self.indirect)),
                self.direct.map(|atlas| read_atlas(renderer, atlas)),
                self.handoff.map(|atlas| read_atlas(renderer, atlas)),
            ],
        }
    }
}

/// One composed atlas read back: every texel's four f16 halves, as bits.
pub(crate) struct AtlasReadback {
    halves: Vec<u16>,
    width: u32,
    height: u32,
    tiles_per_row: u32,
    tiles_per_layer: u32,
    stride: u32,
}

impl AtlasReadback {
    /// The logical tile at stored slot `slot`, row-major RGBA halves. A
    /// streamed tile's padding texels past the 6×6 logical tile are left out.
    pub(crate) fn tile(&self, slot: u32) -> [u16; TILE_HALVES] {
        let tile_slot = slot % self.tiles_per_layer;
        let layer = slot / self.tiles_per_layer;
        let x0 = (tile_slot % self.tiles_per_row) * self.stride;
        let y0 = (tile_slot / self.tiles_per_row) * self.stride;
        assert!(
            x0 + TILE_EDGE <= self.width && y0 + TILE_EDGE <= self.height,
            "slot {slot} lies outside the atlas"
        );
        let mut tile = [0u16; TILE_HALVES];
        for y in 0..TILE_EDGE {
            for x in 0..TILE_EDGE {
                let texel = ((layer * self.height + y0 + y) * self.width + x0 + x) as usize * 4;
                let at = ((y * TILE_EDGE + x) * 4) as usize;
                tile[at..at + 4].copy_from_slice(&self.halves[texel..texel + 4]);
            }
        }
        tile
    }
}

/// The composed atlases of one frame and the words that address them.
pub(crate) struct ComposedReadback {
    pub(crate) words: Vec<u32>,
    /// Indirect, promotion-free direct, handoff direct ([`ATLAS_NAMES`]).
    pub(crate) atlases: [Option<AtlasReadback>; 3],
}

impl ComposedReadback {
    /// The stored slots probe `probe`'s word names: its own slot at L0, the
    /// node's eight corner slots at L1 and its one mean slot at L2. Empty for
    /// an invalid probe, or one whose word is not yet published.
    pub(crate) fn stored_slots(&self, probe: u32) -> Range<u32> {
        let word = decode_probe_indirection_word(self.words[probe as usize]);
        if !word.valid {
            return 0..0;
        }
        let span = if word.level == 1 { 8 } else { 1 };
        word.slot..word.slot + span
    }

    /// Probe `probe`'s own tile in atlas `atlas`, when it stores one at L0.
    pub(crate) fn l0_tile(&self, atlas: usize, probe: u32) -> Option<[u16; TILE_HALVES]> {
        let word = decode_probe_indirection_word(self.words[probe as usize]);
        if !word.valid || word.level != 0 {
            return None;
        }
        Some(self.atlases[atlas].as_ref()?.tile(word.slot))
    }
}

fn read_atlas(renderer: &Renderer, atlas: ComposedAtlas<'_>) -> AtlasReadback {
    assert_eq!(
        atlas.texture.format(),
        wgpu::TextureFormat::Rgba16Float,
        "composed atlases are RGBA16F"
    );
    let extent = atlas.texture.size();
    AtlasReadback {
        halves: read_halves(renderer, atlas.texture),
        width: extent.width,
        height: extent.height,
        tiles_per_row: atlas.tiles_per_row,
        tiles_per_layer: atlas.tiles_per_layer,
        stride: atlas.stride,
    }
}

/// The installed level's probe depth-tile texture (id 59's tiles, then id
/// 57's live tiles), every half as bits. Both SH paths bind the same texture.
pub(crate) fn read_depth_tiles(renderer: &Renderer) -> Vec<u16> {
    read_halves(
        renderer,
        renderer.level().sh_volume_resources.depth_tiles.texture(),
    )
}

/// Every f16 half of a 16-bit-float texture, all layers, de-padded, as bits.
pub(crate) fn read_halves(renderer: &Renderer, texture: &wgpu::Texture) -> Vec<u16> {
    let device = &renderer.device;
    let extent = texture.size();
    let texel_bytes = texture
        .format()
        .block_copy_size(None)
        .expect("an uncompressed texture");
    let row = extent.width * texel_bytes;
    let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("composed tile readback"),
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
        .submit_unbatched([encoder.finish()], "composed tile readback");
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
        .map(u16::from_le_bytes)
        .collect()
}

/// An f16's value.
pub(crate) fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24),
        31 => f32::INFINITY,
        _ => (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}

/// One f16 step at `value`'s magnitude (the spacing of halves there).
pub(crate) fn f16_step(value: f32) -> f32 {
    let magnitude = value.abs().max(6.1e-5);
    2f32.powi(magnitude.log2().floor() as i32 - 10)
}
