// SH volume diagnostic overlay: emits debug-line segments visualizing baked SH
// irradiance volumes. Gated on `dev-tools`. See: context/lib/rendering_pipeline.md §12
//
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use glam::Vec3;
use postretro_level_format::delta_sh_volumes::AFFINITY_FACTOR;
use postretro_level_format::sh_reconstruct::{corner_locals, local_xyz, trilinear_weight};

use super::debug_lines::DebugLineRenderer;
use super::sh_indirection::decode_probe_indirection_word;
use super::sh_volume::{DeltaVolumeMeta, ShVolumeResources};
use postretro_level_loader::LevelWorld;
use postretro_render_cpu::sh_compose::f16_bits_to_f32;

/// Coloring mode for per-probe markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerMode {
    /// Green for `validity != 0`, red for invalid probes.
    Validity,
    /// Color each probe from its baked 4×4×4 brick's stored density level.
    DensityLevel,
    /// All probes drawn with the same neutral color.
    Uniform,
    /// Each marker tinted by the probe's averaged baked irradiance.
    Irradiance,
}

/// Panel-bound diagnostic state. Mirrors `DiagnosticsState::seeded` discipline:
/// `seeded` flips true on first panel open so the panel can pull live defaults
/// without snapping the world. `per_light_visible` resets on map load.
pub struct ShDiagnosticsState {
    pub show_base_aabb: bool,
    pub show_cells: bool,
    pub show_markers: bool,
    pub marker_mode: MarkerMode,
    pub marker_scale: f32,
    pub cell_radius: f32,
    pub per_light_visible: Vec<bool>,
    pub seeded: bool,
}

impl Default for ShDiagnosticsState {
    fn default() -> Self {
        // All toggles default off so overlay geometry only appears in response
        // to an explicit user action in the panel. Without this, opening a map
        // for the first time would render the base AABB before the user has
        // touched the inspector.
        Self {
            show_base_aabb: false,
            show_cells: false,
            show_markers: false,
            marker_mode: MarkerMode::Irradiance,
            marker_scale: 0.10,
            cell_radius: 30.0,
            per_light_visible: Vec::new(),
            seeded: false,
        }
    }
}

/// Probe storage is z-major: `idx = x + y*Nx + z*Nx*Ny`. Centralized here so
/// the SH bake layout and the diagnostic reader cannot drift apart silently.
fn probe_index(x: u32, y: u32, z: u32, dims: [u32; 3]) -> usize {
    let nx = dims[0] as usize;
    let ny = dims[1] as usize;
    (x as usize) + (y as usize) * nx + (z as usize) * nx * ny
}

/// Whether delta volume `index` is currently shown. Before the panel seeds
/// `per_light_visible`, missing entries default to visible so a freshly-loaded
/// level renders all delta volumes until the user toggles them off.
fn delta_volume_visible(state: &ShDiagnosticsState, index: usize) -> bool {
    state.per_light_visible.get(index).copied().unwrap_or(true)
}

const COLOR_BASE_AABB: [u8; 4] = [255, 220, 80, 255];
const COLOR_DELTA_AABB: [u8; 4] = [200, 120, 255, 255];
/// Cell whose center sits in a runtime cell that the portal-reachable set covers
/// for the current frame (i.e., visible per portal traversal / frustum).
const COLOR_CELL_VISIBLE: [u8; 4] = [0, 230, 60, 200];
/// Cell whose center sits in a cell culled by portal traversal / frustum
/// for the current frame, or in a solid cell with no portal reach.
const COLOR_CELL_CULLED: [u8; 4] = [0, 220, 220, 200];
const COLOR_PROBE_VALID: [u8; 4] = [60, 230, 80, 255];
const COLOR_PROBE_INVALID: [u8; 4] = [230, 60, 60, 255];
const COLOR_PROBE_UNIFORM: [u8; 4] = [230, 230, 230, 255];
const COLOR_PROBE_DENSITY_L0: [u8; 4] = [60, 230, 80, 255];
const COLOR_PROBE_DENSITY_L1: [u8; 4] = [255, 210, 60, 255];
const COLOR_PROBE_DENSITY_L2: [u8; 4] = [90, 150, 255, 255];

/// Map an id-34 density-level byte to the marker color. The loader validates
/// the byte, but rendering an unexpected value as red makes a malformed CPU
/// mirror obvious without adding any diagnostic work to the frame loop.
fn density_level_marker_color(level: u8) -> [u8; 4] {
    match level {
        0 => COLOR_PROBE_DENSITY_L0,
        1 => COLOR_PROBE_DENSITY_L1,
        2 => COLOR_PROBE_DENSITY_L2,
        _ => COLOR_PROBE_INVALID,
    }
}

/// Map a probe's average irradiance to a marker color. The irradiance is HDR,
/// so a luminance-preserving Reinhard compresses it into `[0, 1]` without
/// washing hue toward white the way per-channel tonemap would. The debug-line
/// target is sRGB and the shader passes vertex color through untouched, so emit
/// *linear* values here — the hardware encodes.
fn irradiance_marker_color(irradiance: [f32; 3]) -> [u8; 4] {
    let rgb = [
        irradiance[0].max(0.0),
        irradiance[1].max(0.0),
        irradiance[2].max(0.0),
    ];
    let lum = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
    let scale = 1.0 / (1.0 + lum);
    let to_u8 = |c: f32| ((c * scale).clamp(0.0, 1.0) * 255.0).round() as u8;
    [to_u8(rgb[0]), to_u8(rgb[1]), to_u8(rgb[2]), 255]
}

/// Emit one frame of SH diagnostic line segments. Driven entirely by the
/// toggles in `state` — enabled overlays continue rendering after the debug
/// panel is dismissed, and only un-checking a toggle hides its geometry.
pub(super) fn emit(
    state: &ShDiagnosticsState,
    sh: &ShVolumeResources,
    delta_vols: &[DeltaVolumeMeta],
    camera_pos: Vec3,
    world: &LevelWorld,
    visible_cell_mask: &[bool],
    lines: &mut DebugLineRenderer,
) {
    // The frame loop clears the debug-line buffer unconditionally before
    // calling `emit`, so this function is purely additive — it never owns
    // the buffer lifecycle and never clobbers segments produced by other
    // debug-line producers in the same frame.
    if !sh.present {
        return;
    }

    let dims = sh.grid_dimensions;
    let origin = Vec3::from(sh.grid_origin);
    let cell = Vec3::from(sh.cell_size);
    let extent = Vec3::new(
        cell.x * dims[0] as f32,
        cell.y * dims[1] as f32,
        cell.z * dims[2] as f32,
    );

    if state.show_base_aabb {
        // Bounding AABBs render x-ray so the shape stays visible from inside
        // the world — its faces sit at the geometry hull and would otherwise
        // be fully occluded by opaque world depth.
        lines.push_aabb_overlay(origin, origin + extent, COLOR_BASE_AABB);
    }

    if state.show_cells && state.cell_radius > 0.0 {
        emit_cells(
            state,
            dims,
            origin,
            cell,
            camera_pos,
            world,
            visible_cell_mask,
            lines,
        );
    }

    if state.show_markers && state.cell_radius > 0.0 {
        emit_markers(state, sh, dims, origin, cell, camera_pos, lines);
    }

    for (i, meta) in delta_vols.iter().enumerate() {
        if !delta_volume_visible(state, i) {
            continue;
        }
        let d_origin = Vec3::from(meta.origin);
        let d_extent = Vec3::new(
            meta.cell_size[0] * meta.grid_dimensions[0] as f32,
            meta.cell_size[1] * meta.grid_dimensions[1] as f32,
            meta.cell_size[2] * meta.grid_dimensions[2] as f32,
        );
        lines.push_aabb_overlay(d_origin, d_origin + d_extent, COLOR_DELTA_AABB);
    }
}

// Cohesive single-call overlay params; grouping would add an abstraction with
// one caller and break parallelism with the sibling `emit_markers`.
#[allow(clippy::too_many_arguments)]
fn emit_cells(
    state: &ShDiagnosticsState,
    dims: [u32; 3],
    origin: Vec3,
    cell: Vec3,
    camera_pos: Vec3,
    world: &LevelWorld,
    visible_cell_mask: &[bool],
    lines: &mut DebugLineRenderer,
) {
    // Cell color reflects the portal-reachable cell set built from
    // `fog_reachable` — not the frustum+portal cull used by the wireframe
    // overlay. Cells reachable via portals are colored visible; all others
    // are colored culled. No frustum check is applied here. An empty mask
    // is the DrawAll sentinel — fallback paths (no portals, solid-cell,
    // exterior camera, empty world) don't compute a portal set, so every
    // cell is treated as visible to avoid misleadingly cyan overlays.
    let r2 = state.cell_radius * state.cell_radius;
    let draw_all = visible_cell_mask.is_empty();
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                let cell_min =
                    origin + Vec3::new(x as f32 * cell.x, y as f32 * cell.y, z as f32 * cell.z);
                let cell_max = cell_min + cell;
                let center = (cell_min + cell_max) * 0.5;
                if (center - camera_pos).length_squared() > r2 {
                    continue;
                }
                let cell_idx = world.locate_cell(center);
                let visible = if draw_all {
                    true
                } else {
                    visible_cell_mask.get(cell_idx).copied().unwrap_or(false)
                };
                let color = if visible {
                    COLOR_CELL_VISIBLE
                } else {
                    COLOR_CELL_CULLED
                };
                lines.push_aabb(cell_min, cell_max, color);
            }
        }
    }
}

fn emit_markers(
    state: &ShDiagnosticsState,
    sh: &ShVolumeResources,
    dims: [u32; 3],
    origin: Vec3,
    cell: Vec3,
    camera_pos: Vec3,
    lines: &mut DebugLineRenderer,
) {
    // Radius gate mirrors `emit_cells`: without it, dense probe grids blow past
    // the debug-line segment cap and whole rooms vanish from the overlay.
    let r2 = state.cell_radius * state.cell_radius;
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                // Probe (x,y,z) sits at the cell corner `origin + (x,y,z)*cell`
                // — the bake plants probes at integer multiples of cell_size and
                // the runtime samples them there (see sh_compose.wgsl world_pos
                // and forward.wgsl sample_sh_indirect). Draw the marker exactly
                // on the probe it colors, not at the cell center.
                let pos =
                    origin + Vec3::new(x as f32 * cell.x, y as f32 * cell.y, z as f32 * cell.z);
                if (pos - camera_pos).length_squared() > r2 {
                    continue;
                }
                let color = match state.marker_mode {
                    MarkerMode::Uniform => COLOR_PROBE_UNIFORM,
                    MarkerMode::Validity => {
                        let idx = probe_index(x, y, z, dims);
                        // Out-of-range entries (validity slice shorter than the
                        // probe count) are treated as invalid rather than
                        // panicking — keeps the overlay tolerant of partial
                        // bakes.
                        let valid = sh.validity.get(idx).copied().unwrap_or(0) != 0;
                        if valid {
                            COLOR_PROBE_VALID
                        } else {
                            COLOR_PROBE_INVALID
                        }
                    }
                    MarkerMode::DensityLevel => {
                        let idx = probe_index(x, y, z, dims);
                        // `density_levels` is copied from the validated id-34
                        // metadata at load. In particular it retains a level
                        // for invalid probes, unlike the all-zero indirection
                        // word those probes receive for GPU sampling.
                        density_level_marker_color(
                            sh.density_levels.get(idx).copied().unwrap_or_default(),
                        )
                    }
                    MarkerMode::Irradiance => {
                        let idx = probe_index(x, y, z, dims);
                        let irradiance = sh.probe_irradiance.get(idx).copied().unwrap_or([0.0; 3]);
                        irradiance_marker_color(irradiance)
                    }
                };
                lines.push_marker(pos, state.marker_scale, color);
            }
        }
    }
}

/// Async GPU readback of the SH "total" atlas, so the irradiance probe markers
/// reflect the live composed lighting (baked base plus animated-light deltas)
/// instead of only the static bake.
///
/// The copied atlas is reduced to one average interior irradiance color per
/// probe tile. The state machine guarantees each map reads a freshly-copied
/// frame — a copy is encoded into the frame's command buffer, then mapped on a
/// later frame once the GPU has finished. The result lands ~2 frames late,
/// invisible on a debug crosshair. All work is gated on `wanted` so
/// non-irradiance frames pay nothing.
pub struct ShProbeReadback {
    buffer: wgpu::Buffer,
    buffer_size: u64,
    grid_dimensions: [u32; 3],
    /// Stored-tile atlas extent, not dense grid-derived geometry. The copy and
    /// all following decode use this exact extent.
    stored_atlas_dimensions: [u32; 2],
    tile_dimension: u32,
    tile_border: u32,
    atlas_tiles_per_row: u32,
    tiles_per_layer: u32,
    atlas_layer_count: u32,
    /// One load-derived word for each dense-grid probe. It maps probe markers
    /// back into the compact stored atlas without a GPU readback of metadata.
    probe_indirection_words: Vec<u32>,
    /// Row stride in the readback buffer: `atlas_width * 8` rounded up to
    /// `COPY_BYTES_PER_ROW_ALIGNMENT`. The decode skips the per-row padding.
    padded_bytes_per_row: u32,
    /// Set by the renderer each frame: true only while the irradiance marker
    /// overlay is actually being drawn. Stops all copies/maps otherwise.
    wanted: bool,
    /// A copy was encoded and submitted; awaiting its map kickoff in `post_submit`.
    copied_pending: bool,
    /// A `map_async` is in flight — the buffer is busy, so no copy may target it.
    map_pending: Arc<AtomicBool>,
    /// Set by the map callback when the buffer is ready for the live owner to
    /// decode. The callback deliberately does not capture the buffer: renderer
    /// teardown can dispatch a pending callback after its native resource is gone.
    map_ready: Arc<AtomicBool>,
}

impl ShProbeReadback {
    /// 8 bytes per `Rgba16Float` texel (4 halves).
    const BYTES_PER_TEXEL: u32 = 8;

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        grid_dimensions: [u32; 3],
        stored_atlas_dimensions: [u32; 2],
        tile_dimension: u32,
        tile_border: u32,
        atlas_tiles_per_row: u32,
        tiles_per_layer: u32,
        atlas_layer_count: u32,
        probe_indirection_words: &[u32],
    ) -> Self {
        let atlas_width = stored_atlas_dimensions[0].max(1);
        let atlas_height = stored_atlas_dimensions[1].max(1);
        let layer_count = atlas_layer_count.max(1);
        let unpadded = atlas_width * Self::BYTES_PER_TEXEL;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = unpadded.div_ceil(align) * align;
        let buffer_size = padded_bytes_per_row as u64 * atlas_height as u64 * layer_count as u64;

        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SH Probe Irradiance Readback"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        Self {
            buffer,
            buffer_size,
            grid_dimensions,
            stored_atlas_dimensions,
            tile_dimension,
            tile_border,
            atlas_tiles_per_row,
            tiles_per_layer,
            atlas_layer_count: layer_count,
            probe_indirection_words: probe_indirection_words.to_vec(),
            padded_bytes_per_row,
            wanted: false,
            copied_pending: false,
            map_pending: Arc::new(AtomicBool::new(false)),
            map_ready: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Flag whether the irradiance overlay needs live data this frame. Called
    /// before the frame's render encoding.
    pub fn set_wanted(&mut self, wanted: bool) {
        self.wanted = wanted;
    }

    /// Whether `encode_copy` would actually encode a copy this frame. Lets the
    /// caller skip creating and submitting an otherwise-empty command buffer.
    pub fn wants_copy(&self) -> bool {
        self.wanted && !self.copied_pending && !self.map_pending.load(Ordering::Acquire)
    }

    /// Copy the "total" SH atlas into the readback buffer. No-op
    /// unless the overlay is wanted, no map is in flight, and no copy is already
    /// awaiting its map. Must be encoded after the compose dispatch so it
    /// captures this frame's composed result.
    pub fn encode_copy(&mut self, encoder: &mut wgpu::CommandEncoder, total_atlas: &wgpu::Texture) {
        if !self.wants_copy() {
            return;
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: total_atlas,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bytes_per_row),
                    rows_per_image: Some(self.stored_atlas_dimensions[1].max(1)),
                },
            },
            wgpu::Extent3d {
                width: self.stored_atlas_dimensions[0].max(1),
                height: self.stored_atlas_dimensions[1].max(1),
                depth_or_array_layers: self.atlas_layer_count,
            },
        );
        self.copied_pending = true;
    }

    /// Drive the async map state machine. Call once per frame after
    /// `queue.submit`. Returns the decoded per-probe irradiance RGB (z-major)
    /// when a readback has completed this frame, for the caller to swap into
    /// the probe-marker source.
    pub fn post_submit(&mut self, device: &wgpu::Device) -> Option<Vec<[f32; 3]>> {
        let _ = device.poll(wgpu::PollType::Poll);

        let out = if self.map_ready.swap(false, Ordering::AcqRel) {
            let view = self.buffer.slice(0..self.buffer_size).get_mapped_range();
            let decoded = decode_probe_irradiance_atlas(
                &view,
                self.grid_dimensions,
                self.stored_atlas_dimensions,
                self.tile_dimension,
                self.tile_border,
                self.atlas_tiles_per_row,
                self.tiles_per_layer,
                self.atlas_layer_count,
                &self.probe_indirection_words,
                self.padded_bytes_per_row,
            );
            drop(view);
            self.buffer.unmap();
            self.map_pending.store(false, Ordering::Release);
            Some(decoded)
        } else {
            None
        };

        // Kick off a map only for a buffer we actually copied into this cycle.
        if self.copied_pending && !self.map_pending.load(Ordering::Acquire) {
            self.copied_pending = false;
            self.map_pending.store(true, Ordering::Release);
            let ready = Arc::clone(&self.map_ready);
            let pending = Arc::clone(&self.map_pending);
            let size = self.buffer_size;
            self.buffer
                .slice(0..size)
                .map_async(wgpu::MapMode::Read, move |res| match res {
                    Ok(()) => {
                        // Regression: accessing this buffer in the callback
                        // panicked if wgpu dispatched it after renderer teardown.
                        // Only the live owner decodes and unmaps the mapped range.
                        ready.store(true, Ordering::Release);
                    }
                    Err(err) => {
                        log::warn!("[sh-readback] atlas map failed: {err:?}");
                        pending.store(false, Ordering::Release);
                    }
                });
        }

        out
    }
}

/// Decode a mapped stored-atlas readback into per-probe average irradiance RGB,
/// z-major (`x + y*Nx + z*Nx*Ny`). Each dense-grid probe first resolves through
/// its load-derived indirection word: L0 reads its stored slot, L1 reconstructs
/// from the brick's eight canonical corner slots, and L2 reads the brick mean.
///
/// The readback averages each stored tile's interior before reconstruction.
/// That commutes with the shared reconstruction's weighted tile sum, so marker
/// colors have the same L1/L2 semantics as the composed field without retaining
/// every texel on the CPU.
#[allow(clippy::too_many_arguments)]
fn decode_probe_irradiance_atlas(
    bytes: &[u8],
    dims: [u32; 3],
    atlas_dimensions: [u32; 2],
    tile_dimension: u32,
    tile_border: u32,
    atlas_tiles_per_row: u32,
    tiles_per_layer: u32,
    atlas_layer_count: u32,
    probe_indirection_words: &[u32],
    padded_bytes_per_row: u32,
) -> Vec<[f32; 3]> {
    let nx = dims[0].max(1) as usize;
    let ny = dims[1].max(1) as usize;
    let nz = dims[2].max(1) as usize;
    let atlas_width = atlas_dimensions[0].max(1);
    let atlas_height = atlas_dimensions[1].max(1);
    let layer_count = atlas_layer_count.max(1);
    let stride = padded_bytes_per_row as usize;
    let layer_stride = stride * atlas_height as usize;
    let mut out = Vec::with_capacity(nx * ny * nz);
    let interior_start = tile_border.min(tile_dimension);
    let interior_end = tile_dimension
        .saturating_sub(tile_border)
        .max(interior_start);
    for probe in 0..nx * ny * nz {
        let Some(&word) = probe_indirection_words.get(probe) else {
            out.push([0.0; 3]);
            continue;
        };
        let indirection = decode_probe_indirection_word(word);
        if !indirection.valid {
            out.push([0.0; 3]);
            continue;
        }

        let value = match indirection.level {
            0 | 2 => read_stored_tile_average(
                bytes,
                indirection.slot,
                atlas_width,
                atlas_height,
                layer_count,
                tile_dimension,
                interior_start,
                interior_end,
                atlas_tiles_per_row,
                tiles_per_layer,
                stride,
                layer_stride,
            ),
            1 => reconstruct_l1_probe_average(
                bytes,
                probe,
                dims,
                probe_indirection_words,
                indirection.slot,
                atlas_width,
                atlas_height,
                layer_count,
                tile_dimension,
                interior_start,
                interior_end,
                atlas_tiles_per_row,
                tiles_per_layer,
                stride,
                layer_stride,
            ),
            _ => [0.0; 3],
        };
        out.push(value);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn read_stored_tile_average(
    bytes: &[u8],
    slot: u32,
    atlas_width: u32,
    atlas_height: u32,
    atlas_layer_count: u32,
    tile_dimension: u32,
    interior_start: u32,
    interior_end: u32,
    atlas_tiles_per_row: u32,
    tiles_per_layer: u32,
    stride: usize,
    layer_stride: usize,
) -> [f32; 3] {
    let [layer, tile_x, tile_y] =
        postretro_level_format::octahedral::irradiance_array_tile_location(
            slot as usize,
            tiles_per_layer,
            atlas_tiles_per_row,
        );
    if layer >= atlas_layer_count {
        return [0.0; 3];
    }
    let origin = [
        tile_x.saturating_mul(tile_dimension),
        tile_y.saturating_mul(tile_dimension),
    ];
    if origin[0] >= atlas_width || origin[1] >= atlas_height {
        return [0.0; 3];
    }

    let layer_offset = layer as usize * layer_stride;
    let mut sum = [0.0f32; 3];
    let mut count = 0u32;
    for local_y in interior_start..interior_end {
        for local_x in interior_start..interior_end {
            let x = origin[0].saturating_add(local_x).min(atlas_width - 1);
            let y = origin[1].saturating_add(local_y).min(atlas_height - 1);
            let o = layer_offset + y as usize * stride + x as usize * 8;
            // Guard against a readback buffer shorter than the stored-atlas
            // geometry implies — e.g. a stale buffer during a level reload.
            // We read 6 bytes (RGB f16), so skip a texel that would overrun.
            if o + 5 >= bytes.len() {
                continue;
            }
            sum[0] += f16_bits_to_f32(u16::from_le_bytes([bytes[o], bytes[o + 1]]));
            sum[1] += f16_bits_to_f32(u16::from_le_bytes([bytes[o + 2], bytes[o + 3]]));
            sum[2] += f16_bits_to_f32(u16::from_le_bytes([bytes[o + 4], bytes[o + 5]]));
            count += 1;
        }
    }
    if count == 0 {
        return [0.0; 3];
    }
    let inv = 1.0 / count as f32;
    [sum[0] * inv, sum[1] * inv, sum[2] * inv]
}

#[allow(clippy::too_many_arguments)]
fn reconstruct_l1_probe_average(
    bytes: &[u8],
    probe: usize,
    dims: [u32; 3],
    probe_indirection_words: &[u32],
    brick_base_slot: u32,
    atlas_width: u32,
    atlas_height: u32,
    atlas_layer_count: u32,
    tile_dimension: u32,
    interior_start: u32,
    interior_end: u32,
    atlas_tiles_per_row: u32,
    tiles_per_layer: u32,
    stride: usize,
    layer_stride: usize,
) -> [f32; 3] {
    let nx = dims[0].max(1) as usize;
    let ny = dims[1].max(1) as usize;
    let x = probe % nx;
    let y = (probe / nx) % ny;
    let z = probe / (nx * ny);
    let factor = AFFINITY_FACTOR as usize;
    let target_local = (x % factor) + (y % factor) * factor + (z % factor) * factor * factor;
    let brick_origin = [
        x / factor * factor,
        y / factor * factor,
        z / factor * factor,
    ];

    let mut sum = [0.0; 3];
    let mut weight_sum = 0.0f32;
    for (corner, corner_local) in corner_locals().into_iter().enumerate() {
        let (corner_x, corner_y, corner_z) = local_xyz(corner_local);
        let corner_global = [
            brick_origin[0] + corner_x,
            brick_origin[1] + corner_y,
            brick_origin[2] + corner_z,
        ];
        if corner_global[0] >= dims[0] as usize
            || corner_global[1] >= dims[1] as usize
            || corner_global[2] >= dims[2] as usize
        {
            continue;
        }
        let corner_probe = corner_global[0] + corner_global[1] * nx + corner_global[2] * nx * ny;
        let Some(&corner_word) = probe_indirection_words.get(corner_probe) else {
            continue;
        };
        if !decode_probe_indirection_word(corner_word).valid {
            continue;
        }
        let weight = trilinear_weight(local_xyz(target_local), local_xyz(corner_local));
        if weight <= 0.0 {
            continue;
        }
        let rgb = read_stored_tile_average(
            bytes,
            brick_base_slot + corner as u32,
            atlas_width,
            atlas_height,
            atlas_layer_count,
            tile_dimension,
            interior_start,
            interior_end,
            atlas_tiles_per_row,
            tiles_per_layer,
            stride,
            layer_stride,
        );
        for (channel, value) in sum.iter_mut().zip(rgb) {
            *channel += value * weight;
        }
        weight_sum += weight;
    }
    if weight_sum <= 0.0 {
        return [0.0; 3];
    }
    for channel in &mut sum {
        *channel /= weight_sum;
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    fn indirection_word(level: u32, slot: u32) -> u32 {
        (slot << 3) | 0b100 | level
    }

    #[test]
    fn marker_mode_defaults_to_irradiance() {
        let s = ShDiagnosticsState::default();
        assert_eq!(s.marker_mode, MarkerMode::Irradiance);
        assert_eq!(s.marker_scale, 0.10);
        assert_eq!(s.cell_radius, 30.0);
        assert!(!s.show_base_aabb);
        assert!(!s.show_cells);
        assert!(!s.show_markers);
        assert!(!s.seeded);
        assert!(s.per_light_visible.is_empty());
    }

    #[test]
    fn irradiance_color_is_black_for_dark_probe_and_preserves_hue() {
        // A zero probe maps to a black marker.
        assert_eq!(irradiance_marker_color([0.0; 3]), [0, 0, 0, 255]);

        // A red-dominant probe stays red-dominant after tonemapping, even when
        // the HDR magnitude is well above 1.
        let c = irradiance_marker_color([40.0, 4.0, 4.0]);
        assert!(
            c[0] > c[1] && c[0] > c[2],
            "expected red-dominant, got {c:?}"
        );
        assert_eq!(c[1], c[2], "equal G/B input should stay equal");
        assert_eq!(c[3], 255);
    }

    #[test]
    fn density_level_markers_color_an_all_l0_map_as_l0() {
        let all_l0 = [0u8; 64];
        assert!(
            all_l0
                .into_iter()
                .all(|level| density_level_marker_color(level) == COLOR_PROBE_DENSITY_L0)
        );
        assert_ne!(COLOR_PROBE_DENSITY_L0, COLOR_PROBE_DENSITY_L1);
        assert_ne!(COLOR_PROBE_DENSITY_L1, COLOR_PROBE_DENSITY_L2);
    }

    #[test]
    fn decode_probe_irradiance_atlas_averages_tile_interiors_in_probe_order() {
        use crate::render::sh_volume::f32_to_f16_bits;

        // Stored L0 slots remain a 2D tile sheet; marker colors come from each
        // stored tile's interior average, not a grid-indexed texture location.
        let dims = [2u32, 1, 1];
        let tile_dimension = 4u32;
        let tile_border = 1u32;
        let atlas_tiles_per_row = 2u32;
        let atlas_dimensions = [8u32, 4u32];
        let stride = 256usize;
        let mut bytes = vec![0u8; stride * atlas_dimensions[1] as usize];

        // Probe values keyed by z-major index so the ordering assertion is exact.
        let write = |bytes: &mut [u8], off: usize, rgb: [f32; 3]| {
            for (i, &c) in rgb.iter().enumerate() {
                bytes[off + i * 2..off + i * 2 + 2]
                    .copy_from_slice(&f32_to_f16_bits(c).to_le_bytes());
            }
        };
        // tile_dim=4, border=1 → interior texels are local 1..3 on each axis.
        write(&mut bytes, stride + 8, [1.0, 0.0, 0.0]); // probe 0 interior
        write(&mut bytes, stride + 2 * 8, [3.0, 0.0, 0.0]);
        write(&mut bytes, 2 * stride + 8, [1.0, 0.0, 0.0]);
        write(&mut bytes, 2 * stride + 2 * 8, [3.0, 0.0, 0.0]);

        write(&mut bytes, stride + 5 * 8, [0.0, 2.0, 0.0]); // probe 1 interior
        write(&mut bytes, stride + 6 * 8, [0.0, 4.0, 0.0]);
        write(&mut bytes, 2 * stride + 5 * 8, [0.0, 2.0, 0.0]);
        write(&mut bytes, 2 * stride + 6 * 8, [0.0, 4.0, 0.0]);

        let out = decode_probe_irradiance_atlas(
            &bytes,
            dims,
            atlas_dimensions,
            tile_dimension,
            tile_border,
            atlas_tiles_per_row,
            2,
            1,
            &[indirection_word(0, 0), indirection_word(0, 1)],
            stride as u32,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], [2.0, 0.0, 0.0]);
        assert_eq!(out[1], [0.0, 3.0, 0.0]);
    }

    #[test]
    fn decode_probe_irradiance_atlas_reads_layer_major_probe_tiles() {
        use crate::render::sh_volume::f32_to_f16_bits;

        // Regression: multi-layer SH atlases store later probes in subsequent
        // array layers. Decode must use the same layer-major tile placement as
        // the runtime sampler instead of reading every probe from layer 0.
        let dims = [3u32, 1, 1];
        let tile_dimension = 2u32;
        let tile_border = 0u32;
        let atlas_tiles_per_row = 2u32;
        let tiles_per_layer = 2u32;
        let atlas_layer_count = 2u32;
        let atlas_dimensions = [4u32, 2u32];
        let stride = 256usize;
        let layer_stride = stride * atlas_dimensions[1] as usize;
        let mut bytes = vec![0u8; layer_stride * atlas_layer_count as usize];

        let write_tile = |bytes: &mut [u8], layer: usize, tile_x: usize, rgb: [f32; 3]| {
            for local_y in 0..tile_dimension as usize {
                for local_x in 0..tile_dimension as usize {
                    let x = tile_x * tile_dimension as usize + local_x;
                    let y = local_y;
                    let off = layer * layer_stride + y * stride + x * 8;
                    for (i, &c) in rgb.iter().enumerate() {
                        bytes[off + i * 2..off + i * 2 + 2]
                            .copy_from_slice(&f32_to_f16_bits(c).to_le_bytes());
                    }
                }
            }
        };

        write_tile(&mut bytes, 0, 0, [1.0, 0.0, 0.0]);
        write_tile(&mut bytes, 0, 1, [0.0, 2.0, 0.0]);
        write_tile(&mut bytes, 1, 0, [0.0, 0.0, 3.0]);

        let out = decode_probe_irradiance_atlas(
            &bytes,
            dims,
            atlas_dimensions,
            tile_dimension,
            tile_border,
            atlas_tiles_per_row,
            tiles_per_layer,
            atlas_layer_count,
            &[
                indirection_word(0, 0),
                indirection_word(0, 1),
                indirection_word(0, 2),
            ],
            stride as u32,
        );

        assert_eq!(out, vec![[1.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 3.0]]);
    }

    #[test]
    fn decode_probe_irradiance_atlas_reconstructs_l1_from_canonical_corner_slots() {
        use crate::render::sh_volume::f32_to_f16_bits;

        let dims = [4u32, 4, 4];
        let stride = 256usize;
        let mut bytes = vec![0u8; stride];
        for slot in 0..8usize {
            let offset = slot * 8;
            bytes[offset..offset + 2].copy_from_slice(&f32_to_f16_bits(slot as f32).to_le_bytes());
        }
        let words = vec![indirection_word(1, 0); 64];

        let out = decode_probe_irradiance_atlas(
            &bytes,
            dims,
            [8, 1],
            1,
            0,
            8,
            8,
            1,
            &words,
            stride as u32,
        );

        let target_local = 1 + 4 + 16;
        let expected: f32 = corner_locals()
            .into_iter()
            .enumerate()
            .map(|(corner, local)| {
                trilinear_weight(local_xyz(target_local), local_xyz(local)) * corner as f32
            })
            .sum();
        assert!(
            (out[target_local][0] - expected).abs() < 1.0e-6,
            "L1 marker decode must use the shared corner order and weights: {:?} vs {expected}",
            out[target_local]
        );
        assert_eq!(out[target_local][1], 0.0);
        assert_eq!(out[target_local][2], 0.0);
    }

    #[test]
    fn decode_probe_irradiance_atlas_reads_l2_brick_mean_slot() {
        use crate::render::sh_volume::f32_to_f16_bits;

        let dims = [4u32, 4, 4];
        let stride = 256usize;
        let mut bytes = vec![0u8; stride];
        let mean = [0.25, 0.5, 1.0];
        for (channel, value) in mean.into_iter().enumerate() {
            bytes[channel * 2..channel * 2 + 2]
                .copy_from_slice(&f32_to_f16_bits(value).to_le_bytes());
        }
        let words = vec![indirection_word(2, 0); 64];

        let out = decode_probe_irradiance_atlas(
            &bytes,
            dims,
            [1, 1],
            1,
            0,
            1,
            1,
            1,
            &words,
            stride as u32,
        );

        assert!(out.iter().all(|&rgb| rgb == mean));
    }

    /// Probe storage layout is z-major: index = x + y*Nx + z*Nx*Ny. This
    /// asserts the contract on the actual `probe_index` helper used by
    /// `emit_markers`, so a layout change in the SH bake forces the test
    /// to be updated alongside the reader.
    #[test]
    fn probe_index_is_z_major() {
        let dims = [3u32, 4u32, 5u32];
        assert_eq!(probe_index(0, 0, 0, dims), 0);
        assert_eq!(probe_index(1, 0, 0, dims), 1);
        assert_eq!(probe_index(0, 1, 0, dims), 3);
        assert_eq!(probe_index(0, 0, 1, dims), 12);
        assert_eq!(probe_index(2, 3, 4, dims), 2 + 9 + 48);
    }

    /// Contract: before the panel seeds `per_light_visible`, every delta
    /// volume is treated as visible. After seeding, the per-index flag wins.
    #[test]
    fn delta_volume_visible_defaults_true_until_seeded() {
        let mut s = ShDiagnosticsState::default();
        // Unseeded: any index is visible.
        assert!(delta_volume_visible(&s, 0));
        assert!(delta_volume_visible(&s, 7));

        // Seeded: explicit flag is respected; out-of-range still defaults true.
        s.per_light_visible = vec![true, false, true];
        assert!(delta_volume_visible(&s, 0));
        assert!(!delta_volume_visible(&s, 1));
        assert!(delta_volume_visible(&s, 2));
        assert!(delta_volume_visible(&s, 3));
    }
}
