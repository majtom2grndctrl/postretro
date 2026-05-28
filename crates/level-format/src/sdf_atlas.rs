// SDF atlas section (ID 33): sparse brick grid of quantized signed distances
// to static world geometry, used by the runtime SDF static-occluder shadow
// pass. Revives the brick-atlas + top-level + coarse-fallback design from the
// retired `sdf-shadows` tag.
//
// Layout summary:
// - `top_level` is one `u32` per brick cell of the world grid. A slot value of
//   `BRICK_SLOT_EMPTY` (`u32::MAX`) marks an entirely-empty brick (open space);
//   `BRICK_SLOT_INTERIOR` (`u32::MAX - 1`) marks an entirely-solid interior
//   brick. Any other value is an index into the packed `atlas` brick array
//   (each surface brick contributes `brick_size_voxels^3` `i16` distance
//   samples).
// - `atlas` stores quantized distances at `voxel_size_m / 256` per unit, i.e.
//   `i16` = `round(distance_m / (voxel_size_m / 256))`, clamped to the `i16`
//   range. The runtime decodes with the inverse scale.
// - `coarse_distances` is one `f32` (meters) per brick cell. It backs the
//   sphere-tracer in open/interior space where the fine atlas is not present.
//
// Empty-geometry encoding: a section with zero `grid_dims` and empty arrays
// (mirroring `ShVolumeSection`'s empty-volume convention). Section absence in
// the PRL is also valid — the runtime treats "no atlas" as a degradation
// path, not an error.
//
// See: context/plans/in-progress/sdf-static-occluder-shadows/index.md
//      context/plans/in-progress/sdf-static-occluder-shadows/research.md

use crate::FormatError;

/// Section-internal version written as the first u32 of every SdfAtlas section
/// payload. Bumped on any on-disk layout change so the loader can reject stale
/// `.prl` files with a clear error rather than silently misread them.
pub const SDF_ATLAS_VERSION: u32 = 1;

/// Top-level slot sentinel: the brick cell is entirely empty (open space).
/// The runtime tracer reads only the coarse distance for these bricks.
pub const BRICK_SLOT_EMPTY: u32 = u32::MAX;

/// Top-level slot sentinel: the brick cell is entirely inside solid geometry.
/// The runtime tracer treats these as fully occluded / negative-distance.
pub const BRICK_SLOT_INTERIOR: u32 = u32::MAX - 1;

/// SDF atlas section (ID 33).
///
/// On-disk layout (all little-endian):
///
/// ```text
///   Header (64 bytes):
///     u32      version                (= SDF_ATLAS_VERSION)
///     f32 × 3  world_min              (world-space min corner, meters)
///     f32 × 3  world_max              (world-space max corner, meters)
///     f32      voxel_size_m           (meters per voxel)
///     u32      brick_size_voxels      (voxels per brick edge)
///     u32 × 3  grid_dims              (brick count along x/y/z)
///     u32 × 3  atlas_bricks_per_axis  (3D packing of surface bricks)
///     u32      surface_brick_count    (number of surface bricks packed)
///     u32      top_level_len          (== prod(grid_dims))
///     u32      atlas_len              (== voxels_per_brick * surface_brick_count)
///     u32      coarse_len             (== prod(grid_dims))
///
///   Top-level index (top_level_len × u32):
///     u32 per cell, EMPTY/INTERIOR sentinels or 0-based atlas brick index.
///
///   Atlas distances (atlas_len × i16):
///     Quantized signed distances, unit = voxel_size_m / 256.
///     Layout per brick is the bake's responsibility (z-major-within-brick is
///     the historical convention) — this section treats the array as opaque.
///
///   Coarse distances (coarse_len × f32):
///     One per brick cell, in meters. Provides an open-space fallback for the
///     sphere-tracer when the fine atlas is not present for that brick.
/// ```
///
/// Empty-geometry case: a section with `grid_dims == [0, 0, 0]` and empty
/// arrays serializes to just the 64-byte header (length fields all zero) and
/// round-trips. This mirrors `ShVolumeSection`'s empty-volume convention.
#[derive(Debug, Clone, PartialEq)]
pub struct SdfAtlasSection {
    /// World-space min corner of the volumetric grid (meters).
    pub world_min: [f32; 3],
    /// World-space max corner of the volumetric grid (meters).
    pub world_max: [f32; 3],
    /// Side length of one voxel, in meters.
    pub voxel_size_m: f32,
    /// Side length of one brick, in voxels. Total samples per brick is the
    /// cube of this value.
    pub brick_size_voxels: u32,
    /// Brick count along x/y/z of the world grid.
    pub grid_dims: [u32; 3],
    /// 3D packing of surface bricks inside the runtime atlas texture. The bake
    /// chooses the dimensions; the runtime reads them to size the 3D atlas.
    pub atlas_bricks_per_axis: [u32; 3],
    /// Number of surface bricks packed into `atlas`. Equals the count of
    /// non-sentinel entries in `top_level`.
    pub surface_brick_count: u32,
    /// One slot per brick cell (z-major then y, then x). `EMPTY`/`INTERIOR`
    /// sentinels or a 0-based atlas brick index. Length == prod(grid_dims).
    pub top_level: Vec<u32>,
    /// Packed quantized atlas distances. Length ==
    /// `brick_size_voxels^3 * surface_brick_count`. Unit per `i16` step is
    /// `voxel_size_m / 256`.
    pub atlas: Vec<i16>,
    /// One coarse `f32` distance (meters) per brick cell. Length ==
    /// prod(grid_dims).
    pub coarse_distances: Vec<f32>,
}

impl SdfAtlasSection {
    /// Size of the fixed header in bytes:
    /// 4 (version) + 12 (world_min) + 12 (world_max) + 4 (voxel_size_m)
    /// + 4 (brick_size_voxels) + 12 (grid_dims) + 12 (atlas_bricks_per_axis)
    /// + 4 (surface_brick_count) + 4 (top_level_len) + 4 (atlas_len)
    /// + 4 (coarse_len) = 76 bytes.
    pub const HEADER_SIZE: usize = 4 + 12 + 12 + 4 + 4 + 12 + 12 + 4 + 4 + 4 + 4;

    /// Construct an empty-geometry section: zero grid dims, no bricks, no
    /// coarse distances. Matches the on-disk encoding the spec calls out for
    /// "no SDF" / degenerate-bake cases.
    pub fn empty() -> Self {
        Self {
            world_min: [0.0; 3],
            world_max: [0.0; 3],
            voxel_size_m: 0.0,
            brick_size_voxels: 0,
            grid_dims: [0, 0, 0],
            atlas_bricks_per_axis: [0, 0, 0],
            surface_brick_count: 0,
            top_level: Vec::new(),
            atlas: Vec::new(),
            coarse_distances: Vec::new(),
        }
    }

    /// Total brick-cell count derived from `grid_dims`.
    pub fn total_bricks(&self) -> usize {
        (self.grid_dims[0] as usize)
            .saturating_mul(self.grid_dims[1] as usize)
            .saturating_mul(self.grid_dims[2] as usize)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let top_level_len = self.top_level.len();
        let atlas_len = self.atlas.len();
        let coarse_len = self.coarse_distances.len();

        let mut buf = Vec::with_capacity(
            Self::HEADER_SIZE + top_level_len * 4 + atlas_len * 2 + coarse_len * 4,
        );

        // Header.
        buf.extend_from_slice(&SDF_ATLAS_VERSION.to_le_bytes());
        for v in &self.world_min {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        for v in &self.world_max {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf.extend_from_slice(&self.voxel_size_m.to_le_bytes());
        buf.extend_from_slice(&self.brick_size_voxels.to_le_bytes());
        for v in &self.grid_dims {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        for v in &self.atlas_bricks_per_axis {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf.extend_from_slice(&self.surface_brick_count.to_le_bytes());
        buf.extend_from_slice(&(top_level_len as u32).to_le_bytes());
        buf.extend_from_slice(&(atlas_len as u32).to_le_bytes());
        buf.extend_from_slice(&(coarse_len as u32).to_le_bytes());

        // Top-level index.
        for slot in &self.top_level {
            buf.extend_from_slice(&slot.to_le_bytes());
        }

        // Atlas quantized distances.
        for d in &self.atlas {
            buf.extend_from_slice(&d.to_le_bytes());
        }

        // Coarse per-brick distances.
        for d in &self.coarse_distances {
            buf.extend_from_slice(&d.to_le_bytes());
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < Self::HEADER_SIZE {
            return Err(truncated("header"));
        }

        let mut o = 0;
        let version = read_u32(data, o);
        o += 4;
        if version != SDF_ATLAS_VERSION {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "sdf atlas section version {version}, expected {SDF_ATLAS_VERSION} — \
                     recompile the .prl with the current `prl-build`"
                ),
            )));
        }

        let world_min = [
            read_f32(data, o),
            read_f32(data, o + 4),
            read_f32(data, o + 8),
        ];
        o += 12;
        let world_max = [
            read_f32(data, o),
            read_f32(data, o + 4),
            read_f32(data, o + 8),
        ];
        o += 12;
        let voxel_size_m = read_f32(data, o);
        o += 4;
        let brick_size_voxels = read_u32(data, o);
        o += 4;
        let grid_dims = [
            read_u32(data, o),
            read_u32(data, o + 4),
            read_u32(data, o + 8),
        ];
        o += 12;
        let atlas_bricks_per_axis = [
            read_u32(data, o),
            read_u32(data, o + 4),
            read_u32(data, o + 8),
        ];
        o += 12;
        let surface_brick_count = read_u32(data, o);
        o += 4;
        let top_level_len = read_u32(data, o) as usize;
        o += 4;
        let atlas_len = read_u32(data, o) as usize;
        o += 4;
        let coarse_len = read_u32(data, o) as usize;
        o += 4;
        debug_assert_eq!(o, Self::HEADER_SIZE);

        let top_level_bytes = top_level_len.checked_mul(4).ok_or_else(|| {
            FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("sdf atlas top_level_len ({top_level_len}) overflow"),
            ))
        })?;
        let atlas_bytes = atlas_len.checked_mul(2).ok_or_else(|| {
            FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("sdf atlas atlas_len ({atlas_len}) overflow"),
            ))
        })?;
        let coarse_bytes = coarse_len.checked_mul(4).ok_or_else(|| {
            FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("sdf atlas coarse_len ({coarse_len}) overflow"),
            ))
        })?;

        let body_bytes = top_level_bytes
            .checked_add(atlas_bytes)
            .and_then(|n| n.checked_add(coarse_bytes))
            .ok_or_else(|| {
                FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "sdf atlas body byte count overflow",
                ))
            })?;
        if data.len() < Self::HEADER_SIZE + body_bytes {
            return Err(truncated("body"));
        }

        let mut top_level = Vec::with_capacity(top_level_len);
        for i in 0..top_level_len {
            top_level.push(read_u32(data, o + i * 4));
        }
        o += top_level_bytes;

        let mut atlas = Vec::with_capacity(atlas_len);
        for i in 0..atlas_len {
            atlas.push(read_i16(data, o + i * 2));
        }
        o += atlas_bytes;

        let mut coarse_distances = Vec::with_capacity(coarse_len);
        for i in 0..coarse_len {
            coarse_distances.push(read_f32(data, o + i * 4));
        }
        // Final advance kept for parity with sibling decoders, even though we
        // do not read past it.
        let _ = o + coarse_bytes;

        Ok(Self {
            world_min,
            world_max,
            voxel_size_m,
            brick_size_voxels,
            grid_dims,
            atlas_bricks_per_axis,
            surface_brick_count,
            top_level,
            atlas,
            coarse_distances,
        })
    }
}

fn truncated(what: &str) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        format!("sdf atlas section truncated: {what}"),
    ))
}

fn read_f32(data: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn read_i16(data: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([data[at], data[at + 1]])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn populated_section() -> SdfAtlasSection {
        // A 2×2×2 brick grid: 8 cells total. Two surface bricks, one empty,
        // one interior, four with arbitrary indices that exercise the slot
        // encoding round trip.
        let brick_size = 4u32;
        let voxels_per_brick = (brick_size * brick_size * brick_size) as usize;
        let surface_brick_count = 2u32;
        let atlas_len = voxels_per_brick * surface_brick_count as usize;

        let top_level: Vec<u32> = vec![
            0,                    // surface brick 0
            1,                    // surface brick 1
            BRICK_SLOT_EMPTY,
            BRICK_SLOT_INTERIOR,
            BRICK_SLOT_EMPTY,
            BRICK_SLOT_INTERIOR,
            BRICK_SLOT_EMPTY,
            BRICK_SLOT_EMPTY,
        ];

        let atlas: Vec<i16> = (0..atlas_len)
            .map(|i| {
                // Pattern that includes negatives, zero, and positives — the
                // sphere-tracer reads signed distances, so the round trip
                // must preserve sign across all of them.
                let base = (i as i32 % 511) - 255;
                base.clamp(i16::MIN as i32, i16::MAX as i32) as i16
            })
            .collect();

        let coarse_distances: Vec<f32> =
            (0..top_level.len()).map(|i| 0.25 + i as f32 * 0.125).collect();

        SdfAtlasSection {
            world_min: [-8.0, -2.0, -8.0],
            world_max: [8.0, 6.0, 8.0],
            voxel_size_m: 0.0625,
            brick_size_voxels: brick_size,
            grid_dims: [2, 2, 2],
            atlas_bricks_per_axis: [2, 1, 1],
            surface_brick_count,
            top_level,
            atlas,
            coarse_distances,
        }
    }

    #[test]
    fn round_trip_populated() {
        let section = populated_section();
        let bytes = section.to_bytes();
        let restored = SdfAtlasSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
        // Re-encoding the decoded section reproduces the exact same bytes —
        // the AC's "byte-identical round trip" property.
        assert_eq!(restored.to_bytes(), bytes);
    }

    #[test]
    fn round_trip_empty_geometry() {
        let section = SdfAtlasSection::empty();
        let bytes = section.to_bytes();
        // Empty geometry encodes to just the fixed header — no body bytes.
        assert_eq!(bytes.len(), SdfAtlasSection::HEADER_SIZE);
        let restored = SdfAtlasSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
        assert_eq!(restored.to_bytes(), bytes);
        // The empty marker round-trips: zero grid dims and empty arrays.
        assert_eq!(restored.grid_dims, [0, 0, 0]);
        assert!(restored.top_level.is_empty());
        assert!(restored.atlas.is_empty());
        assert!(restored.coarse_distances.is_empty());
    }

    #[test]
    fn empty_slots_preserve_sentinels() {
        let section = populated_section();
        let bytes = section.to_bytes();
        let restored = SdfAtlasSection::from_bytes(&bytes).unwrap();
        // Spot-check both sentinels survive the round trip.
        assert!(restored.top_level.contains(&BRICK_SLOT_EMPTY));
        assert!(restored.top_level.contains(&BRICK_SLOT_INTERIOR));
    }

    #[test]
    fn rejects_truncated_header() {
        let err = SdfAtlasSection::from_bytes(&[0u8; 8]).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)));
    }

    #[test]
    fn rejects_truncated_body() {
        let section = populated_section();
        let bytes = section.to_bytes();
        let truncated = &bytes[..bytes.len() - 4];
        let err = SdfAtlasSection::from_bytes(truncated).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)));
    }

    #[test]
    fn rejects_mismatched_section_version() {
        let section = populated_section();
        let mut bytes = section.to_bytes();
        // Stamp a bogus version to confirm the loader rejects rather than
        // silently misreading a stale layout.
        bytes[0..4].copy_from_slice(&999u32.to_le_bytes());
        let err = SdfAtlasSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("version"),
            "expected version-mismatch error, got: {msg}",
        );
    }

    /// Loader-side degradation contract: a PRL with the SdfAtlas section
    /// absent from its section table must read without error and yield
    /// `None` for the section lookup. Section absence is a valid "no SDF"
    /// load, not an error.
    #[test]
    fn prl_container_returns_none_for_missing_sdf_atlas_section() {
        use crate::{SectionBlob, SectionId, read_container, read_section_data, write_prl};

        let sections = vec![SectionBlob {
            section_id: SectionId::Geometry as u32,
            version: 1,
            data: vec![0xAA, 0xBB, 0xCC],
        }];
        let mut buf = Vec::new();
        write_prl(&mut buf, &sections).unwrap();

        let mut cursor = std::io::Cursor::new(&buf);
        let meta = read_container(&mut cursor).unwrap();
        assert!(meta.find_section(SectionId::SdfAtlas as u32).is_none());
        let result = read_section_data(&mut cursor, &meta, SectionId::SdfAtlas as u32).unwrap();
        assert!(result.is_none(), "missing SDF atlas must return None");
    }

    /// The section round-trips through the PRL container as a blob (this is
    /// how `prl-build` and the runtime will produce/consume it). Confirms
    /// the new `SectionId::SdfAtlas = 33` registration is wired.
    #[test]
    fn round_trips_through_prl_container() {
        use crate::{SectionBlob, SectionId, read_container, read_section_data, write_prl};

        let section = populated_section();
        let section_bytes = section.to_bytes();
        let sections = vec![SectionBlob {
            section_id: SectionId::SdfAtlas as u32,
            version: SDF_ATLAS_VERSION as u16,
            data: section_bytes.clone(),
        }];
        let mut buf = Vec::new();
        write_prl(&mut buf, &sections).unwrap();

        let mut cursor = std::io::Cursor::new(&buf);
        let meta = read_container(&mut cursor).unwrap();
        let raw = read_section_data(&mut cursor, &meta, SectionId::SdfAtlas as u32)
            .unwrap()
            .expect("SdfAtlas section present in container");
        assert_eq!(raw, section_bytes);
        let restored = SdfAtlasSection::from_bytes(&raw).unwrap();
        assert_eq!(section, restored);
    }

    /// Pin the `SectionId::SdfAtlas` discriminant to 33 — the wire-format
    /// contract. Drift here would silently break runtime/bake co-deserialization.
    #[test]
    fn section_id_is_thirty_three() {
        use crate::SectionId;
        assert_eq!(SectionId::SdfAtlas as u32, 33);
        assert_eq!(SectionId::from_u32(33), Some(SectionId::SdfAtlas));
    }
}
