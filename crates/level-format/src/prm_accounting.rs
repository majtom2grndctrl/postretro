// Per-material, per-slot, per-mip byte accounting for baked `.prm` texture
// bundles.
//
// `prl-build` reports a summary of this at its `TextureMips` stage; an asset
// streaming system is the anticipated second consumer, which is why the
// granularity is per-mip rather than a single total — residency is decided one
// mip at a time (`resource_management.md` §4.6, Surface Depth design D6.3).
//
// A `.prm` slot stores its mip chain as one concatenated payload with a
// `level_count` and a `payload_bytes` count, so per-mip offsets and sizes are
// *derived* from `(format, width, height, level_count)`. Nothing here reads
// payload bytes and nothing here needs the wire format to change.
//
// ## Why this lives in `postretro-level-format`
//
// `render-cpu::loaded_texture::level_byte_size` is the engine's only
// pre-existing sizing primitive and would be the natural neighbour, but
// `render-cpu` is layer 3 and `level-compiler` (which builds `prl-build`) is
// layer 1. A `level-compiler → render-cpu` edge is an upward edge across three
// layers and would drag the whole render stack into compiler build times —
// exactly what `development_guide.md` §Workspace forbids. `level-format` is
// layer 0, already owns `PrmFormat`/`PrmSlot`, and is already a dependency of
// both sides, so it is the only home that serves the compiler and the runtime
// without a new edge. `level_byte_size` delegates here so the format → bytes
// table has one definition.
//
// This module is deliberately accounting only: no budget, no cap, no eviction
// policy, no residency decision.
//
// See: context/lib/resource_management.md §4, §8 · context/lib/build_pipeline.md §Baked texture mips

use crate::prm::{PrmFormat, PrmReadError, PrmSlot};

/// Number of material slots in a `.prm` bundle: diffuse, specular, normal, emissive.
pub const SLOT_COUNT: usize = 4;

/// Wire-order label for a slot index, for reporting. An index beyond the four
/// material slots reports as `"unknown"` rather than panicking — this is a
/// reporting helper, not a contract check.
pub fn slot_label(slot_index: u8) -> &'static str {
    match slot_index {
        0 => "diffuse",
        1 => "specular",
        2 => "normal",
        3 => "emissive",
        _ => "unknown",
    }
}

/// Bytes one mip level of `format` occupies at `width` × `height`, for a
/// single array layer.
///
/// Uncompressed formats are `bytes_per_pixel * width * height`. `Bc5RgUnorm` is
/// block-compressed at 16 bytes per 4×4 texel block, so it is sized
/// `ceil(w/4) * ceil(h/4) * 16` — a 2×2 BC5 level still costs one whole block.
/// Zero dimensions clamp to 1, matching how the mip pyramid itself clamps.
pub fn mip_level_bytes(format: PrmFormat, width: u32, height: u32) -> u64 {
    let w = u64::from(width.max(1));
    let h = u64::from(height.max(1));
    match format {
        PrmFormat::Rgba8UnormSrgb | PrmFormat::Rgba8Unorm => 4 * w * h,
        // The two-channel surface map: R specular, G depth (Surface Depth D1).
        PrmFormat::Rg8Unorm => 2 * w * h,
        PrmFormat::R8Unorm => w * h,
        PrmFormat::Bc5RgUnorm => w.div_ceil(4) * h.div_ceil(4) * 16,
    }
}

/// Dimensions of mip `level` of a `width` × `height` chain: each axis halves
/// per level and clamps at 1.
pub fn mip_level_dimensions(width: u32, height: u32, level: u8) -> (u32, u32) {
    (
        (width >> u32::from(level)).max(1),
        (height >> u32::from(level)).max(1),
    )
}

/// One mip level's slice of a slot's payload, for a single array layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MipBytes {
    /// Mip level index; 0 is the full-resolution level.
    pub level: u8,
    pub width: u32,
    pub height: u32,
    /// Byte offset of this level within **one** layer's concatenated mip chain.
    /// Layered payloads are layer-major, so layer `l` adds `l * layer_bytes`.
    pub offset: u64,
    pub bytes: u64,
}

/// Byte accounting for one slot of one material's `.prm` bundle.
///
/// Built from the slot's declared `(format, width, height, level_count)`; the
/// payload is never read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotBytes {
    /// Wire-order slot index: 0 diffuse, 1 specular, 2 normal, 3 emissive.
    pub slot_index: u8,
    pub format: PrmFormat,
    pub width: u16,
    pub height: u16,
    /// Shared across every slot of a bundle; 1 for world and model materials.
    pub layer_count: u16,
    /// One layer's mip chain, level 0 first.
    pub levels: Vec<MipBytes>,
}

impl SlotBytes {
    /// Account one parsed or freshly baked slot.
    pub fn from_slot(slot_index: u8, slot: &PrmSlot, layer_count: u16) -> Self {
        let mut levels = Vec::with_capacity(usize::from(slot.level_count));
        let mut offset = 0u64;
        for level in 0..slot.level_count {
            let (width, height) =
                mip_level_dimensions(u32::from(slot.width), u32::from(slot.height), level);
            let bytes = mip_level_bytes(slot.format, width, height);
            levels.push(MipBytes {
                level,
                width,
                height,
                offset,
                bytes,
            });
            offset += bytes;
        }
        Self {
            slot_index,
            format: slot.format,
            width: slot.width,
            height: slot.height,
            layer_count,
            levels,
        }
    }

    /// Number of mip levels in the chain. `Bc5RgUnorm` chains are truncated to
    /// levels whose dimensions are both ≥ 4, so this is not always
    /// `expected_level_count`.
    pub fn level_count(&self) -> u8 {
        self.levels.len() as u8
    }

    /// Bytes for mip `level` of this slot, in one array layer. `None` when the
    /// chain has no such level — a streaming system asking for a mip past a
    /// truncated BC5 chain gets an answer, not a panic.
    pub fn mip_bytes(&self, level: u8) -> Option<u64> {
        self.levels
            .iter()
            .find(|mip| mip.level == level)
            .map(|mip| mip.bytes)
    }

    /// Bytes for mip `level` across every array layer.
    pub fn mip_bytes_all_layers(&self, level: u8) -> Option<u64> {
        self.mip_bytes(level)
            .map(|bytes| bytes * u64::from(self.layer_count))
    }

    /// Bytes for mip `level` and every smaller level, in one array layer —
    /// what a slot costs once its top `level` mips are dropped.
    pub fn bytes_from_level(&self, level: u8) -> u64 {
        self.levels
            .iter()
            .filter(|mip| mip.level >= level)
            .map(|mip| mip.bytes)
            .sum()
    }

    /// Bytes for one complete layer's mip chain.
    pub fn layer_bytes(&self) -> u64 {
        self.levels.iter().map(|mip| mip.bytes).sum()
    }

    /// Bytes for the whole slot payload: one layer's chain times `layer_count`.
    /// Equals the slot's `payload_bytes` on the wire.
    pub fn total_bytes(&self) -> u64 {
        self.layer_bytes() * u64::from(self.layer_count)
    }
}

/// Byte accounting for one material's whole `.prm` bundle.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MaterialBytes {
    slots: [Option<SlotBytes>; SLOT_COUNT],
}

impl MaterialBytes {
    /// Account a bundle the baker has just built. Absent slots contribute
    /// nothing.
    pub fn from_slots(slots: &[Option<PrmSlot>; SLOT_COUNT], layer_count: u16) -> Self {
        let mut out = Self::default();
        for (index, slot) in slots.iter().enumerate() {
            if let Some(slot) = slot {
                out.slots[index] = Some(SlotBytes::from_slot(index as u8, slot, layer_count));
            }
        }
        out
    }

    /// Account a bundle read back through `PrmFile::from_bytes_partial`. Slots
    /// that are absent or failed to parse contribute nothing — a corrupt slot
    /// is not charged bytes the runtime will not upload.
    pub fn from_parsed_slots(
        slots: &[Result<PrmSlot, PrmReadError>; SLOT_COUNT],
        layer_count: u16,
    ) -> Self {
        let mut out = Self::default();
        for (index, slot) in slots.iter().enumerate() {
            if let Ok(slot) = slot {
                out.slots[index] = Some(SlotBytes::from_slot(index as u8, slot, layer_count));
            }
        }
        out
    }

    /// The accounting for one slot, or `None` when the bundle has no usable
    /// slot at that index.
    pub fn slot(&self, slot_index: u8) -> Option<&SlotBytes> {
        self.slots.get(usize::from(slot_index))?.as_ref()
    }

    /// Present slots in ascending wire order.
    pub fn slots(&self) -> impl Iterator<Item = &SlotBytes> {
        self.slots.iter().flatten()
    }

    /// Bytes for mip `level` of slot `slot_index`, across every array layer —
    /// the per-mip, per-slot question a residency decision asks.
    pub fn mip_bytes(&self, slot_index: u8, level: u8) -> Option<u64> {
        self.slot(slot_index)?.mip_bytes_all_layers(level)
    }

    /// Total bytes across every present slot and every array layer.
    pub fn total_bytes(&self) -> u64 {
        self.slots().map(SlotBytes::total_bytes).sum()
    }

    /// Deepest mip chain across the bundle's slots.
    pub fn max_level_count(&self) -> u8 {
        self.slots().map(SlotBytes::level_count).max().unwrap_or(0)
    }
}

/// A byte report over a set of baked material bundles, in a deterministic
/// order.
///
/// Entries are kept sorted by material name. `prl-build` guarantees
/// byte-identical output for identical inputs, and its stage report must not
/// vary run to run — so ordering is never driven by hash-map iteration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextureByteSummary {
    /// Invariant: sorted by material name, with at most one entry per name.
    entries: Vec<(String, MaterialBytes)>,
}

impl TextureByteSummary {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one material's accounting, keeping `entries` sorted by name.
    ///
    /// Recording the same name twice replaces the earlier entry rather than
    /// adding a second one: a material is one bundle and must be charged once,
    /// however many times a caller walks it.
    pub fn record(&mut self, name: impl Into<String>, bytes: MaterialBytes) {
        let name = name.into();
        match self
            .entries
            .binary_search_by(|(other, _)| other.as_str().cmp(name.as_str()))
        {
            Ok(at) => self.entries[at] = (name, bytes),
            Err(at) => self.entries.insert(at, (name, bytes)),
        }
    }

    /// Recorded materials, ordered by name.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &MaterialBytes)> {
        self.entries
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes))
    }

    pub fn material_count(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Total bytes across every recorded material.
    pub fn total_bytes(&self) -> u64 {
        self.entries
            .iter()
            .map(|(_, bytes)| bytes.total_bytes())
            .sum()
    }

    /// Total bytes per slot, indexed by wire-order slot index.
    pub fn slot_totals(&self) -> [u64; SLOT_COUNT] {
        let mut totals = [0u64; SLOT_COUNT];
        for (_, material) in &self.entries {
            for slot in material.slots() {
                let index = usize::from(slot.slot_index);
                if index < SLOT_COUNT {
                    totals[index] += slot.total_bytes();
                }
            }
        }
        totals
    }

    /// Total bytes per mip level across every material and slot; the vector is
    /// indexed by level and is as long as the deepest chain recorded.
    ///
    /// This is the shape a streaming system reads: "what would dropping every
    /// mip 0 save", answered without re-walking the bundles.
    pub fn mip_totals(&self) -> Vec<u64> {
        let depth = self
            .entries
            .iter()
            .map(|(_, material)| material.max_level_count())
            .max()
            .unwrap_or(0);
        let mut totals = vec![0u64; usize::from(depth)];
        for (_, material) in &self.entries {
            for slot in material.slots() {
                for mip in &slot.levels {
                    totals[usize::from(mip.level)] += mip.bytes * u64::from(slot.layer_count);
                }
            }
        }
        totals
    }

    /// The `limit` largest materials by total bytes, descending. Ties break on
    /// name ascending so the ranking is deterministic.
    pub fn largest_materials(&self, limit: usize) -> Vec<(&str, u64)> {
        let mut ranked: Vec<(&str, u64)> = self
            .entries
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.total_bytes()))
            .collect();
        ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
        ranked.truncate(limit);
        ranked
    }

    /// A deterministic multi-line report: totals, a per-slot breakdown, a
    /// per-mip breakdown, and the largest materials. Callers log these lines.
    pub fn report_lines(&self, largest: usize) -> Vec<String> {
        if self.is_empty() {
            return vec!["texture bytes: no material bundles".to_string()];
        }

        let total = self.total_bytes();
        let mut lines = vec![format!(
            "texture bytes: {} across {} material bundle(s)",
            format_bytes(total),
            self.material_count(),
        )];

        let slot_totals = self.slot_totals();
        let by_slot: Vec<String> = (0..SLOT_COUNT)
            .filter(|index| slot_totals[*index] > 0)
            .map(|index| {
                format!(
                    "{} {}",
                    slot_label(index as u8),
                    format_bytes(slot_totals[index])
                )
            })
            .collect();
        lines.push(format!("  by slot: {}", by_slot.join(", ")));

        let mip_totals = self.mip_totals();
        let by_mip: Vec<String> = mip_totals
            .iter()
            .enumerate()
            .map(|(level, bytes)| format!("L{level} {}", format_bytes(*bytes)))
            .collect();
        lines.push(format!("  by mip: {}", by_mip.join(", ")));

        for (name, bytes) in self.largest_materials(largest) {
            lines.push(format!("  largest: {name} {}", format_bytes(bytes)));
        }

        lines
    }
}

/// Render a byte count as an exact count plus a scaled unit, e.g.
/// `"5865216 B (5.59 MiB)"`. Exact bytes first so a report can be diffed
/// without float rounding hiding a change.
pub fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let scaled = bytes as f64;
    if scaled >= MIB {
        format!("{bytes} B ({:.2} MiB)", scaled / MIB)
    } else if scaled >= KIB {
        format!("{bytes} B ({:.2} KiB)", scaled / KIB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prm::{
        PrmFile, PrmHeader, PrmSlots, STAGE_VERSION, bc5_level_count, expected_level_count,
    };

    fn slot(format: PrmFormat, width: u16, height: u16) -> PrmSlot {
        let level_count = match format {
            PrmFormat::Bc5RgUnorm => bc5_level_count(width, height),
            _ => expected_level_count(width, height),
        };
        let payload_len: u64 = (0..level_count)
            .map(|level| {
                let (w, h) = mip_level_dimensions(u32::from(width), u32::from(height), level);
                mip_level_bytes(format, w, h)
            })
            .sum();
        PrmSlot {
            format,
            width,
            height,
            level_count,
            payload: vec![0u8; payload_len as usize],
        }
    }

    #[test]
    fn mip_level_bytes_matches_each_format_footprint() {
        assert_eq!(mip_level_bytes(PrmFormat::Rgba8UnormSrgb, 8, 8), 256);
        assert_eq!(mip_level_bytes(PrmFormat::Rgba8Unorm, 8, 8), 256);
        // The surface map costs exactly twice the single-channel specular it
        // replaces — the whole cost of Surface Depth's data format.
        assert_eq!(mip_level_bytes(PrmFormat::Rg8Unorm, 8, 8), 128);
        assert_eq!(mip_level_bytes(PrmFormat::R8Unorm, 8, 8), 64);
        // BC5: 2x2 blocks of 16 bytes.
        assert_eq!(mip_level_bytes(PrmFormat::Bc5RgUnorm, 8, 8), 64);
        // A sub-block level still costs one whole 4x4 block.
        assert_eq!(mip_level_bytes(PrmFormat::Bc5RgUnorm, 2, 2), 16);
        // Zero clamps to 1 the way the mip pyramid does.
        assert_eq!(mip_level_bytes(PrmFormat::R8Unorm, 0, 0), 1);
    }

    #[test]
    fn slot_bytes_offsets_walk_the_chain_in_order() {
        let accounted = SlotBytes::from_slot(1, &slot(PrmFormat::Rg8Unorm, 8, 8), 1);
        assert_eq!(accounted.level_count(), 4);
        let sizes: Vec<u64> = accounted.levels.iter().map(|mip| mip.bytes).collect();
        assert_eq!(sizes, vec![128, 32, 8, 2]);
        let offsets: Vec<u64> = accounted.levels.iter().map(|mip| mip.offset).collect();
        assert_eq!(offsets, vec![0, 128, 160, 168]);
        assert_eq!(accounted.layer_bytes(), 170);
        assert_eq!(accounted.total_bytes(), 170);
        assert_eq!(accounted.mip_bytes(2), Some(8));
        assert_eq!(accounted.mip_bytes(9), None);
        // Dropping mip 0 leaves the tail of the chain.
        assert_eq!(accounted.bytes_from_level(1), 42);
        assert_eq!(accounted.bytes_from_level(0), accounted.layer_bytes());
    }

    #[test]
    fn slot_total_bytes_equals_the_real_payload_length() {
        // The derived accounting must agree with what the wire format actually
        // stores, for every format including the truncated BC5 chain and a
        // layered payload.
        for format in [
            PrmFormat::Rgba8UnormSrgb,
            PrmFormat::Rgba8Unorm,
            PrmFormat::R8Unorm,
            PrmFormat::Rg8Unorm,
            PrmFormat::Bc5RgUnorm,
        ] {
            for layer_count in [1u16, 3] {
                let mut base = slot(format, 16, 8);
                base.payload = base.payload.repeat(usize::from(layer_count));
                let accounted = SlotBytes::from_slot(0, &base, layer_count);
                assert_eq!(
                    accounted.total_bytes(),
                    base.payload.len() as u64,
                    "{format:?} with {layer_count} layer(s)"
                );
                assert_eq!(
                    accounted.mip_bytes_all_layers(0),
                    accounted.mip_bytes(0).map(|b| b * u64::from(layer_count)),
                );
            }
        }
    }

    #[test]
    fn material_bytes_round_trip_a_real_prm_payload() {
        // Build a real bundle, serialize it, read it back, and prove the
        // accounting derived from the header alone matches the bytes on disk.
        let slots = [
            Some(slot(PrmFormat::Rgba8UnormSrgb, 16, 16)),
            Some(slot(PrmFormat::Rg8Unorm, 16, 16)),
            Some(slot(PrmFormat::Bc5RgUnorm, 16, 16)),
            None,
        ];
        let baked = MaterialBytes::from_slots(&slots, 1);

        let file = PrmFile {
            header: PrmHeader {
                stage_version: STAGE_VERSION,
                slot_mask: PrmSlots::DIFFUSE | PrmSlots::SPECULAR | PrmSlots::NORMAL,
                bundle_hash: [0u8; 32],
                total_body_bytes: 0,
                layer_count: 1,
            },
            slots,
        };
        let bytes = file.to_bytes().expect("fixture serializes");
        let (header, parsed) = PrmFile::from_bytes_partial(&bytes);
        let header = header.expect("fixture header parses");
        let read_back = MaterialBytes::from_parsed_slots(&parsed, header.layer_count);

        assert_eq!(baked, read_back, "bake-time and read-back accounting agree");
        for index in 0..SLOT_COUNT as u8 {
            let expected = parsed[usize::from(index)]
                .as_ref()
                .ok()
                .map(|slot| slot.payload.len() as u64);
            assert_eq!(
                read_back.slot(index).map(SlotBytes::total_bytes),
                expected,
                "slot {index} accounting must equal its payload length"
            );
        }
        assert_eq!(read_back.slot(3), None, "absent slot is charged nothing");
        // 16x16: diffuse 4bpp = 1364, surface map 2bpp = 682, BC5 truncated to
        // levels 16, 8, 4 = 256 + 64 + 16 = 336.
        assert_eq!(read_back.total_bytes(), 1364 + 682 + 336);
        assert_eq!(read_back.mip_bytes(1, 0), Some(512));
        assert_eq!(read_back.mip_bytes(2, 3), None, "BC5 chain stops before L3");
    }

    #[test]
    fn a_height_sibling_costs_exactly_the_specular_slot_again() {
        // Surface Depth's whole memory story: packing depth into the specular
        // slot doubles that slot and touches nothing else.
        let flat = MaterialBytes::from_slots(
            &[
                Some(slot(PrmFormat::Rgba8UnormSrgb, 64, 64)),
                Some(slot(PrmFormat::R8Unorm, 64, 64)),
                None,
                None,
            ],
            1,
        );
        let deep = MaterialBytes::from_slots(
            &[
                Some(slot(PrmFormat::Rgba8UnormSrgb, 64, 64)),
                Some(slot(PrmFormat::Rg8Unorm, 64, 64)),
                None,
                None,
            ],
            1,
        );
        let specular = flat.slot(1).expect("R8 specular").total_bytes();
        assert_eq!(
            deep.slot(1).expect("surface map").total_bytes(),
            specular * 2
        );
        assert_eq!(deep.total_bytes() - flat.total_bytes(), specular);
        assert_eq!(deep.slot(0), flat.slot(0), "diffuse is untouched");
    }

    #[test]
    fn summary_orders_materials_by_name_not_insertion() {
        let mut summary = TextureByteSummary::new();
        let one_slot =
            |format| MaterialBytes::from_slots(&[Some(slot(format, 8, 8)), None, None, None], 1);
        summary.record("zebra", one_slot(PrmFormat::Rgba8UnormSrgb));
        summary.record("alpha", one_slot(PrmFormat::Rgba8UnormSrgb));
        summary.record("mid", one_slot(PrmFormat::Rgba8UnormSrgb));

        let names: Vec<&str> = summary.entries().map(|(name, _)| name).collect();
        assert_eq!(names, vec!["alpha", "mid", "zebra"]);

        // Recording the same set in a different order yields an identical
        // report — the determinism `prl-build` output ordering depends on.
        let mut other = TextureByteSummary::new();
        other.record("mid", one_slot(PrmFormat::Rgba8UnormSrgb));
        other.record("zebra", one_slot(PrmFormat::Rgba8UnormSrgb));
        other.record("alpha", one_slot(PrmFormat::Rgba8UnormSrgb));
        assert_eq!(summary.report_lines(5), other.report_lines(5));
        assert_eq!(summary, other);
    }

    #[test]
    fn summary_aggregates_per_slot_and_per_mip() {
        let mut summary = TextureByteSummary::new();
        summary.record(
            "stone",
            MaterialBytes::from_slots(
                &[
                    Some(slot(PrmFormat::Rgba8UnormSrgb, 8, 8)),
                    Some(slot(PrmFormat::Rg8Unorm, 8, 8)),
                    None,
                    None,
                ],
                1,
            ),
        );
        summary.record(
            "metal",
            MaterialBytes::from_slots(
                &[
                    Some(slot(PrmFormat::Rgba8UnormSrgb, 8, 8)),
                    Some(slot(PrmFormat::R8Unorm, 8, 8)),
                    None,
                    None,
                ],
                1,
            ),
        );

        // 8x8 chain sums: 4bpp = 340, 2bpp = 170, 1bpp = 85.
        assert_eq!(summary.slot_totals(), [680, 255, 0, 0]);
        assert_eq!(summary.total_bytes(), 935);
        // Per level across both materials: L0 = 256+256 + 128 + 64 = 704.
        assert_eq!(summary.mip_totals(), vec![704, 176, 44, 11]);
        assert_eq!(
            summary.mip_totals().iter().sum::<u64>(),
            summary.total_bytes(),
            "per-mip totals must reconcile with the grand total"
        );
        assert_eq!(
            summary.largest_materials(1),
            vec![("stone", 510)],
            "the material carrying a surface map is the larger one"
        );
    }

    #[test]
    fn recording_the_same_material_twice_charges_it_once() {
        let mut summary = TextureByteSummary::new();
        let material =
            MaterialBytes::from_slots(&[Some(slot(PrmFormat::R8Unorm, 8, 8)), None, None, None], 1);
        summary.record("stone", material.clone());
        summary.record("stone", material);
        assert_eq!(summary.material_count(), 1);
        assert_eq!(summary.total_bytes(), 85);
    }

    #[test]
    fn largest_materials_breaks_ties_on_name() {
        let mut summary = TextureByteSummary::new();
        let same =
            MaterialBytes::from_slots(&[Some(slot(PrmFormat::R8Unorm, 8, 8)), None, None, None], 1);
        summary.record("b", same.clone());
        summary.record("a", same.clone());
        summary.record("c", same);
        assert_eq!(
            summary
                .largest_materials(3)
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            vec!["a", "b", "c"],
        );
    }

    #[test]
    fn empty_summary_reports_cleanly() {
        let summary = TextureByteSummary::new();
        assert!(summary.is_empty());
        assert_eq!(summary.total_bytes(), 0);
        assert_eq!(summary.mip_totals(), Vec::<u64>::new());
        assert_eq!(summary.report_lines(5).len(), 1);
    }

    #[test]
    fn format_bytes_scales_units() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2048 B (2.00 KiB)");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5242880 B (5.00 MiB)");
    }
}
