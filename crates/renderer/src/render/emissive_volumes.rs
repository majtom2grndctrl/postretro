// Emissive volume GPU resources: the forward pass's slot-2 volume-id vertex
// stream, the group-4 volume table, and the table's per-frame entry writes.
// See: context/lib/rendering_pipeline.md §7.3

use postretro_render_cpu::emissive_volume::{
    EMISSIVE_TABLE_BYTES, EMISSIVE_TABLE_ENTRIES, EMISSIVE_TABLE_ENTRY_BYTES, EmissiveTableUpload,
    authored_table_bytes,
};

use super::Renderer;
use super::renderer_types::LevelGeometry;
use crate::render::gpu_create::GpuCreate;

/// Bytes per volume-id stream vertex: one `Uint32`.
pub(crate) const EMISSIVE_VOLUME_STRIDE: usize = 4;
/// The forward shader's `VertexInput` location for the volume id.
pub(crate) const EMISSIVE_VOLUME_SHADER_LOCATION: u32 = 8;

/// One level's emissive volume buffers, written whole at install.
pub(crate) struct EmissiveVolumeBuffers {
    /// Slot 2 of the forward pass: one volume id per world vertex.
    pub(crate) vertex: wgpu::Buffer,
    /// Group 4's volume table: entry 0 "no volume", entry `i + 1` volume `i`.
    pub(crate) table: wgpu::Buffer,
}

/// The volume-id stream as bytes, at least one entry so the buffer is never
/// empty. A stream that does not match `vertex_count` packs all-zero, so no
/// vertex reads another vertex's volume. An empty stream is the normal
/// no-member case; a non-empty mismatch means the loader's stream disagrees
/// with the geometry, so it warns.
pub(crate) fn emissive_volume_vertex_bytes(stream: &[u32], vertex_count: usize) -> Vec<u8> {
    let count = vertex_count.max(1);
    if stream.len() != vertex_count {
        if !stream.is_empty() {
            log::warn!(
                "[Renderer] emissive volume stream holds {} entries for {vertex_count} vertices; \
                 every face presents its texture strength alone",
                stream.len()
            );
        }
        return vec![0; count * EMISSIVE_VOLUME_STRIDE];
    }
    let mut bytes = Vec::with_capacity(count * EMISSIVE_VOLUME_STRIDE);
    for &id in stream {
        bytes.extend_from_slice(&id.to_ne_bytes());
    }
    bytes.resize(count * EMISSIVE_VOLUME_STRIDE, 0);
    bytes
}

/// Upload the level's volume-id stream and seed its table with every volume's
/// no-script glow, so a frame drawn before the volume bridge's first write
/// already presents the authored look.
pub(crate) fn install_emissive_volume_buffers(
    device: &wgpu::Device,
    geometry: &LevelGeometry<'_>,
) -> EmissiveVolumeBuffers {
    let vertex = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("World Emissive Volume Buffer"),
        contents: &emissive_volume_vertex_bytes(
            geometry.vertex_emissive_volumes,
            geometry.vertices.len(),
        ),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let table = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Emissive Volume Table"),
        contents: &authored_table_bytes(geometry.emissive_volumes),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    EmissiveVolumeBuffers { vertex, table }
}

impl Renderer {
    /// Write the volume table's pending entries: each `upload.runs` range of
    /// entries copies from `upload.table`, the whole CPU table. Returns
    /// `false`, writing nothing, when no level is installed, so the caller
    /// keeps them pending.
    ///
    /// Writes go through the frame's upload batch, so they ride the next
    /// submission even when the surface acquire fails.
    pub fn upload_emissive_volume_table(&mut self, upload: EmissiveTableUpload<'_>) -> bool {
        let EmissiveTableUpload { table, runs } = upload;
        debug_assert_eq!(table.len(), EMISSIVE_TABLE_BYTES);
        let Self { queue, full, .. } = self;
        let Some(level) = full.as_mut().and_then(|full| full.level.as_mut()) else {
            return false;
        };
        for run in runs {
            debug_assert!(run.end as usize <= EMISSIVE_TABLE_ENTRIES);
            let bytes = run.start as usize * EMISSIVE_TABLE_ENTRY_BYTES
                ..run.end as usize * EMISSIVE_TABLE_ENTRY_BYTES;
            // Runs come from the table's own dirty ranges within its fixed length
            // (the debug_assert above guards it), so the lookup cannot fail.
            let Some(data) = table.get(bytes.clone()) else {
                continue;
            };
            queue.write_buffer(&level.emissive_volumes.table, bytes.start as u64, data);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_stream_packs_one_u32_per_vertex_and_zeroes_a_mismatch() {
        let bytes = emissive_volume_vertex_bytes(&[0, 3, 256], 3);
        let ids: Vec<u32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| u32::from_ne_bytes(*word))
            .collect();
        assert_eq!(ids, [0, 3, 256]);
        assert_eq!(emissive_volume_vertex_bytes(&[], 3), vec![0; 12]);
        assert_eq!(emissive_volume_vertex_bytes(&[1, 1], 3), vec![0; 12]);
        assert_eq!(
            emissive_volume_vertex_bytes(&[], 0).len(),
            EMISSIVE_VOLUME_STRIDE,
            "never an empty buffer"
        );
    }
}
