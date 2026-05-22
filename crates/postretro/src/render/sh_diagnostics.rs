// SH volume diagnostic overlay: emits debug-line segments visualizing baked SH
// irradiance volumes. Gated on `dev-tools`. See: context/lib/rendering_pipeline.md §11
//
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use glam::Vec3;

use super::debug_lines::DebugLineRenderer;
use super::sh_compose::f16_bits_to_f32;
use super::sh_volume::{DeltaVolumeMeta, ShVolumeResources};
use crate::prl::LevelWorld;

/// Coloring mode for per-probe markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerMode {
    /// Green for `validity != 0`, red for invalid probes.
    Validity,
    /// All probes drawn with the same neutral color.
    Uniform,
    /// Each marker tinted by the probe's baked ambient light color (L0 band).
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
/// Cell whose center sits in a leaf that the portal-reachable set covers
/// for the current frame (i.e., visible per portal traversal / frustum).
const COLOR_CELL_VISIBLE: [u8; 4] = [0, 230, 60, 200];
/// Cell whose center sits in a leaf culled by portal traversal / frustum
/// for the current frame, or in a solid leaf with no portal reach.
const COLOR_CELL_CULLED: [u8; 4] = [0, 220, 220, 200];
const COLOR_PROBE_VALID: [u8; 4] = [60, 230, 80, 255];
const COLOR_PROBE_INVALID: [u8; 4] = [230, 60, 60, 255];
const COLOR_PROBE_UNIFORM: [u8; 4] = [230, 230, 230, 255];

/// Real spherical-harmonic normalization for the L0 band: `1 / (2 * sqrt(pi))`.
/// The L0 coefficient times this constant is the constant ambient irradiance the
/// probe reconstructs in every direction — i.e. the average light color there.
/// Matches the `0.282095` used by the shader and the bake reference.
const SH_L0_BASIS: f32 = 0.282095;

/// Map a probe's L0 (DC) coefficient to a marker color. The reconstructed
/// ambient irradiance is HDR, so a luminance-preserving Reinhard compresses it
/// into `[0, 1]` without washing hue toward white the way per-channel tonemap
/// would. The debug-line target is sRGB and the shader passes vertex color
/// through untouched, so emit *linear* values here — the hardware encodes.
fn irradiance_marker_color(l0: [f32; 3]) -> [u8; 4] {
    let rgb = [
        (l0[0] * SH_L0_BASIS).max(0.0),
        (l0[1] * SH_L0_BASIS).max(0.0),
        (l0[2] * SH_L0_BASIS).max(0.0),
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
    visible_leaf_mask: &[bool],
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
            visible_leaf_mask,
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

fn emit_cells(
    state: &ShDiagnosticsState,
    dims: [u32; 3],
    origin: Vec3,
    cell: Vec3,
    camera_pos: Vec3,
    world: &LevelWorld,
    visible_leaf_mask: &[bool],
    lines: &mut DebugLineRenderer,
) {
    // Cell color reflects the portal-reachable leaf set built from
    // `fog_reachable` — not the frustum+portal cull used by the wireframe
    // overlay. Leaves reachable via portals are colored visible; all others
    // are colored culled. No frustum check is applied here. An empty mask
    // is the DrawAll sentinel — fallback paths (no portals, solid-leaf,
    // exterior camera, empty world) don't compute a portal set, so every
    // cell is treated as visible to avoid misleadingly cyan overlays.
    let r2 = state.cell_radius * state.cell_radius;
    let draw_all = visible_leaf_mask.is_empty();
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
                let leaf_idx = world.find_leaf(center);
                let visible = if draw_all {
                    true
                } else {
                    visible_leaf_mask.get(leaf_idx).copied().unwrap_or(false)
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
                    MarkerMode::Irradiance => {
                        let idx = probe_index(x, y, z, dims);
                        let l0 = sh.probe_l0.get(idx).copied().unwrap_or([0.0; 3]);
                        irradiance_marker_color(l0)
                    }
                };
                lines.push_marker(pos, state.marker_scale, color);
            }
        }
    }
}

/// Async GPU readback of the SH "total" band-0 (L0) 3D texture, so the
/// irradiance probe markers reflect the live composed lighting (baked base plus
/// animated-light deltas) instead of only the static bake.
///
/// One band suffices: L0 is the constant ambient term whose color the markers
/// display. The state machine guarantees each map reads a freshly-copied frame
/// — a copy is encoded into the frame's command buffer, then mapped on a later
/// frame once the GPU has finished. The result lands ~2 frames late, invisible
/// on a debug crosshair. All work is gated on `wanted` so non-irradiance frames
/// pay nothing.
pub struct ShProbeReadback {
    buffer: wgpu::Buffer,
    buffer_size: u64,
    grid_dimensions: [u32; 3],
    /// Row stride in the readback buffer: `grid_x * 8` rounded up to
    /// `COPY_BYTES_PER_ROW_ALIGNMENT`. The decode skips the per-row padding.
    padded_bytes_per_row: u32,
    /// Set by the renderer each frame: true only while the irradiance marker
    /// overlay is actually being drawn. Stops all copies/maps otherwise.
    wanted: bool,
    /// A copy was encoded and submitted; awaiting its map kickoff in `post_submit`.
    copied_pending: bool,
    /// A `map_async` is in flight — the buffer is busy, so no copy may target it.
    map_pending: Arc<AtomicBool>,
    /// Decoded per-probe L0 RGB (z-major), populated by the map callback.
    map_result: Arc<Mutex<Option<Vec<[f32; 3]>>>>,
}

impl ShProbeReadback {
    /// 8 bytes per `Rgba16Float` texel (4 halves).
    const BYTES_PER_TEXEL: u32 = 8;

    pub fn new(device: &wgpu::Device, grid_dimensions: [u32; 3]) -> Self {
        let nx = grid_dimensions[0].max(1);
        let ny = grid_dimensions[1].max(1);
        let nz = grid_dimensions[2].max(1);
        let unpadded = nx * Self::BYTES_PER_TEXEL;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = unpadded.div_ceil(align) * align;
        let buffer_size = padded_bytes_per_row as u64 * ny as u64 * nz as u64;

        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SH Probe L0 Readback"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        Self {
            buffer,
            buffer_size,
            grid_dimensions,
            padded_bytes_per_row,
            wanted: false,
            copied_pending: false,
            map_pending: Arc::new(AtomicBool::new(false)),
            map_result: Arc::new(Mutex::new(None)),
        }
    }

    /// Flag whether the irradiance overlay needs live data this frame. Called
    /// before the frame's render encoding.
    pub fn set_wanted(&mut self, wanted: bool) {
        self.wanted = wanted;
    }

    /// Copy band 0 of the "total" SH volume into the readback buffer. No-op
    /// unless the overlay is wanted, no map is in flight, and no copy is already
    /// awaiting its map. Must be encoded after the compose dispatch so it
    /// captures this frame's composed result.
    pub fn encode_copy(&mut self, encoder: &mut wgpu::CommandEncoder, total_band0: &wgpu::Texture) {
        if !self.wanted || self.copied_pending || self.map_pending.load(Ordering::Acquire) {
            return;
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: total_band0,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bytes_per_row),
                    rows_per_image: Some(self.grid_dimensions[1].max(1)),
                },
            },
            wgpu::Extent3d {
                width: self.grid_dimensions[0].max(1),
                height: self.grid_dimensions[1].max(1),
                depth_or_array_layers: self.grid_dimensions[2].max(1),
            },
        );
        self.copied_pending = true;
    }

    /// Drive the async map state machine. Call once per frame after
    /// `queue.submit`. Returns the decoded per-probe L0 RGB (z-major) when a
    /// readback has completed this frame, for the caller to swap into the
    /// probe-marker source.
    pub fn post_submit(&mut self, device: &wgpu::Device) -> Option<Vec<[f32; 3]>> {
        let _ = device.poll(wgpu::PollType::Poll);

        let out = self.map_result.lock().unwrap().take();
        if out.is_some() {
            self.buffer.unmap();
            self.map_pending.store(false, Ordering::Release);
        }

        // Kick off a map only for a buffer we actually copied into this cycle.
        if self.copied_pending && !self.map_pending.load(Ordering::Acquire) {
            self.copied_pending = false;
            self.map_pending.store(true, Ordering::Release);
            let result_slot = Arc::clone(&self.map_result);
            let pending = Arc::clone(&self.map_pending);
            let buf = self.buffer.clone();
            let size = self.buffer_size;
            let dims = self.grid_dimensions;
            let stride = self.padded_bytes_per_row;
            self.buffer
                .slice(0..size)
                .map_async(wgpu::MapMode::Read, move |res| match res {
                    Ok(()) => {
                        let view = buf.slice(0..size).get_mapped_range();
                        let decoded = decode_l0(&view, dims, stride);
                        drop(view);
                        // Buffer stays mapped; the main thread unmaps it in the
                        // next `post_submit` after consuming the result.
                        *result_slot.lock().unwrap() = Some(decoded);
                    }
                    Err(err) => {
                        log::warn!("[sh-readback] band-0 map failed: {err:?}");
                        pending.store(false, Ordering::Release);
                    }
                });
        }

        out
    }
}

/// Decode a mapped band-0 readback into per-probe L0 RGB, z-major
/// (`x + y*Nx + z*Nx*Ny`). Skips the per-row alignment padding. Probes are
/// pushed in x→y→z order, which is exactly the z-major probe index.
fn decode_l0(bytes: &[u8], dims: [u32; 3], padded_bytes_per_row: u32) -> Vec<[f32; 3]> {
    let nx = dims[0].max(1) as usize;
    let ny = dims[1].max(1) as usize;
    let nz = dims[2].max(1) as usize;
    let stride = padded_bytes_per_row as usize;
    let mut out = Vec::with_capacity(nx * ny * nz);
    for z in 0..nz {
        for y in 0..ny {
            let row = z * stride * ny + y * stride;
            for x in 0..nx {
                let o = row + x * 8;
                let r = f16_bits_to_f32(u16::from_le_bytes([bytes[o], bytes[o + 1]]));
                let g = f16_bits_to_f32(u16::from_le_bytes([bytes[o + 2], bytes[o + 3]]));
                let b = f16_bits_to_f32(u16::from_le_bytes([bytes[o + 4], bytes[o + 5]]));
                out.push([r, g, b]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn decode_l0_skips_row_padding_and_is_z_major() {
        use crate::render::sh_volume::f32_to_f16_bits;

        // 2×1×2 grid: row stride padded to 256 bytes (real data is 2*8 = 16).
        let dims = [2u32, 1, 2];
        let stride = 256usize;
        let mut bytes = vec![0u8; stride * 2]; // ny=1, nz=2 → 2 rows.

        // Probe values keyed by z-major index so the ordering assertion is exact.
        // Layout: row r covers z=r (since ny=1); within a row, x advances by 8 bytes.
        let write = |bytes: &mut [u8], off: usize, rgb: [f32; 3]| {
            for (i, &c) in rgb.iter().enumerate() {
                bytes[off + i * 2..off + i * 2 + 2]
                    .copy_from_slice(&f32_to_f16_bits(c).to_le_bytes());
            }
        };
        write(&mut bytes, 0, [1.0, 0.0, 0.0]); // z=0,x=0 → idx 0
        write(&mut bytes, 8, [0.0, 1.0, 0.0]); // z=0,x=1 → idx 1
        write(&mut bytes, stride, [0.0, 0.0, 1.0]); // z=1,x=0 → idx 2
        write(&mut bytes, stride + 8, [0.5, 0.5, 0.5]); // z=1,x=1 → idx 3

        let out = decode_l0(&bytes, dims, stride as u32);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0], [1.0, 0.0, 0.0]);
        assert_eq!(out[1], [0.0, 1.0, 0.0]);
        assert_eq!(out[2], [0.0, 0.0, 1.0]);
        assert_eq!(out[3], [0.5, 0.5, 0.5]);
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
