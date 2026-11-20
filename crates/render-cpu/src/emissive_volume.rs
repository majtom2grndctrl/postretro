// Emissive volume glow factors and the forward shader's volume table, packed on
// CPU with per-entry change tracking so a static frame uploads nothing.
// See: context/lib/rendering_pipeline.md §7.3

use std::ops::Range;

use postretro_level_format::emissive::{EmissiveVolumeRecord, MAX_EMISSIVE_VOLUMES};

/// Table entries: entry 0 is "no volume", entry `i + 1` is volume `i`, so the
/// per-vertex stream's `volume index + 1` indexes it directly.
pub const EMISSIVE_TABLE_ENTRIES: usize = MAX_EMISSIVE_VOLUMES + 1;
/// One `vec4<f32>` per entry: the rgb factor, then an unused lane.
pub const EMISSIVE_TABLE_ENTRY_BYTES: usize = 16;
/// The whole uniform the forward shader declares (`array<vec4<f32>, 257>`).
pub const EMISSIVE_TABLE_BYTES: usize = EMISSIVE_TABLE_ENTRIES * EMISSIVE_TABLE_ENTRY_BYTES;
/// The factor a face in no volume presents: its texture strength unchanged.
pub const NO_VOLUME_FACTOR: [f32; 3] = [1.0; 3];

/// One volume's glow before it packs: the scalar multiplier the brightness
/// maps to and the colour it tints by, kept apart so the flash limiter can
/// judge the scalar and the colour separately (`emissive_flash.rs`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VolumeGlow {
    /// `min × (1 − brightness) + max × brightness`: the authored range at
    /// the scripted brightness, exactly `max` at brightness 1.
    pub multiplier: f32,
    /// Linear tint, each channel in `[0, 1]`.
    pub color: [f32; 3],
}

impl VolumeGlow {
    /// The glow a script's values present.
    ///
    /// Scripts own `brightness` and `color`, so both clamp here to `[0, 1]`:
    /// a script never pushes a surface past the mapper's `max`. Each reads a
    /// bad value as its primitive fixes it: a NaN brightness reads 0 and an
    /// infinite one clamps (`setEmissiveBrightness`); a non-finite colour
    /// channel reads 0 (`setEmissiveColor`).
    ///
    /// The multiplier blends the endpoints rather than adding a scaled span,
    /// so brightness 1 presents exactly `max`: the bake's reference state is
    /// `max × colour`, and the no-script glow matches it bit for bit.
    pub fn scripted(min: f32, max: f32, brightness: f32, color: [f32; 3]) -> Self {
        let brightness = unit_brightness(brightness);
        Self {
            multiplier: min * (1.0 - brightness) + max * brightness,
            color: color.map(unit_channel),
        }
    }

    /// The rgb factor the volume table packs: `multiplier × color`.
    pub fn factor(self) -> [f32; 3] {
        self.color.map(|channel| channel * self.multiplier)
    }
}

/// One volume's glow factor: `(min × (1 − brightness) + max × brightness) ×
/// color`, clamped as [`VolumeGlow::scripted`] clamps.
pub fn volume_factor(min: f32, max: f32, brightness: f32, color: [f32; 3]) -> [f32; 3] {
    VolumeGlow::scripted(min, max, brightness, color).factor()
}

/// A brightness clamped to `[0, 1]`, as `setEmissiveBrightness` and the
/// animation sample clamp fix it: NaN reads 0, ±inf clamps to 1 or 0.
fn unit_brightness(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

/// A colour channel clamped to `[0, 1]`, as `setEmissiveColor` fixes it: any
/// non-finite channel reads 0.
fn unit_channel(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The table a freshly installed level binds before any frame packs: every
/// volume at its no-script state (brightness 1, authored colour).
pub fn authored_table_bytes(volumes: &[EmissiveVolumeRecord]) -> Vec<u8> {
    let mut table = EmissiveVolumeTable::new();
    let factors: Vec<[f32; 3]> = volumes
        .iter()
        .take(MAX_EMISSIVE_VOLUMES)
        .map(|volume| volume_factor(volume.min, volume.max, 1.0, volume.color))
        .collect();
    table.pack(&factors);
    table.bytes().to_vec()
}

/// One frame's volume-table writes: copy each `runs` range of entries from
/// `table`, at the same entry offset. Byte ranges are the entry range times
/// [`EMISSIVE_TABLE_ENTRY_BYTES`].
#[derive(Debug, Clone, Copy)]
pub struct EmissiveTableUpload<'a> {
    pub table: &'a [u8],
    pub runs: &'a [Range<u32>],
}

impl EmissiveTableUpload<'_> {
    /// Bytes these writes send.
    pub fn byte_count(&self) -> usize {
        self.runs
            .iter()
            .map(|run| (run.end - run.start) as usize * EMISSIVE_TABLE_ENTRY_BYTES)
            .sum()
    }
}

/// The CPU copy of the forward shader's volume table, plus which entries
/// differ from what the GPU last received.
///
/// Sized once; [`pack`](Self::pack), [`pending`](Self::pending) and
/// [`mark_uploaded`](Self::mark_uploaded) never allocate, so the per-frame
/// path stays allocation-free.
#[derive(Debug, Clone)]
pub struct EmissiveVolumeTable {
    bytes: Vec<u8>,
    dirty: Vec<bool>,
    /// Contiguous dirty entry ranges, rebuilt by `dirty_runs`.
    runs: Vec<Range<u32>>,
}

impl Default for EmissiveVolumeTable {
    fn default() -> Self {
        Self::new()
    }
}

impl EmissiveVolumeTable {
    /// Every entry "no volume", nothing pending.
    pub fn new() -> Self {
        let mut table = Self {
            bytes: vec![0; EMISSIVE_TABLE_BYTES],
            dirty: vec![false; EMISSIVE_TABLE_ENTRIES],
            runs: Vec::with_capacity(EMISSIVE_TABLE_ENTRIES),
        };
        for entry in 0..EMISSIVE_TABLE_ENTRIES {
            table.write_entry(entry, NO_VOLUME_FACTOR);
        }
        table
    }

    /// Back to the new state with every entry pending, so the next upload
    /// replaces whatever a previous level left on the GPU.
    pub fn reset(&mut self) {
        for entry in 0..EMISSIVE_TABLE_ENTRIES {
            self.write_entry(entry, NO_VOLUME_FACTOR);
        }
        self.dirty.fill(true);
    }

    /// Write volume `i`'s factor to entry `i + 1` for each `factors[i]`,
    /// marking an entry pending only when its bytes change. Factors past the
    /// volume cap are ignored; the compiler never writes such a volume.
    pub fn pack(&mut self, factors: &[[f32; 3]]) {
        for (volume, &factor) in factors.iter().take(MAX_EMISSIVE_VOLUMES).enumerate() {
            self.pack_volume(volume, factor);
        }
    }

    /// Write volume `volume`'s factor to entry `volume + 1`, marking it
    /// pending only when its bytes change. A volume past the cap is ignored.
    /// For a volume [`pack`](Self::pack) does not reach, such as one the
    /// caller packs once and never animates.
    pub fn pack_volume(&mut self, volume: usize, factor: [f32; 3]) {
        if volume >= MAX_EMISSIVE_VOLUMES {
            return;
        }
        let entry = volume + 1;
        if self.write_entry(entry, factor) {
            self.dirty[entry] = true;
        }
    }

    /// The whole table, as the uniform binding reads it.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The writes the GPU table is missing: the whole CPU table and the entry
    /// ranges changed since the last upload.
    pub fn pending(&mut self) -> EmissiveTableUpload<'_> {
        self.collect_runs();
        EmissiveTableUpload {
            table: &self.bytes,
            runs: &self.runs,
        }
    }

    /// Entry ranges changed since the last upload, ascending; each pending
    /// entry appears once, adjacent ones merged.
    fn collect_runs(&mut self) {
        self.runs.clear();
        let mut entry = 0;
        while entry < EMISSIVE_TABLE_ENTRIES {
            if !self.dirty[entry] {
                entry += 1;
                continue;
            }
            let start = entry;
            while entry < EMISSIVE_TABLE_ENTRIES && self.dirty[entry] {
                entry += 1;
            }
            self.runs.push(start as u32..entry as u32);
        }
    }

    /// The GPU now holds every pending entry.
    pub fn mark_uploaded(&mut self) {
        self.dirty.fill(false);
    }

    /// The rgb factor entry `entry` holds.
    pub fn entry(&self, entry: usize) -> [f32; 3] {
        let at = entry * EMISSIVE_TABLE_ENTRY_BYTES;
        std::array::from_fn(|lane| {
            let bytes = &self.bytes[at + lane * 4..at + lane * 4 + 4];
            f32::from_le_bytes(bytes.try_into().expect("four-byte lane"))
        })
    }

    /// Store `factor` at `entry`; whether its bytes changed.
    fn write_entry(&mut self, entry: usize, factor: [f32; 3]) -> bool {
        let at = entry * EMISSIVE_TABLE_ENTRY_BYTES;
        let mut packed = [0u8; EMISSIVE_TABLE_ENTRY_BYTES];
        for (lane, value) in factor.into_iter().enumerate() {
            packed[lane * 4..lane * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        let slot = &mut self.bytes[at..at + EMISSIVE_TABLE_ENTRY_BYTES];
        if *slot == packed {
            return false;
        }
        slot.copy_from_slice(&packed);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pending entry runs as `(start, end)` pairs.
    fn spans(table: &mut EmissiveVolumeTable) -> Vec<(u32, u32)> {
        table
            .pending()
            .runs
            .iter()
            .map(|run| (run.start, run.end))
            .collect()
    }

    fn close(a: [f32; 3], b: [f32; 3]) {
        for (x, y) in a.into_iter().zip(b) {
            assert!((x - y).abs() < 1e-6, "{a:?} vs {b:?}");
        }
    }

    // A3 runtime half: the formula at brightness 0, 0.5 and 1 with a
    // non-white colour.
    #[test]
    fn volume_factor_maps_brightness_across_the_authored_range_and_tints() {
        let color = [1.0, 0.5, 0.25];
        close(volume_factor(2.0, 6.0, 0.0, color), [2.0, 1.0, 0.5]);
        close(volume_factor(2.0, 6.0, 0.5, color), [4.0, 2.0, 1.0]);
        close(volume_factor(2.0, 6.0, 1.0, color), [6.0, 3.0, 1.5]);
    }

    // P10 and P49: brightness and colour clamp to [0, 1] as their primitives
    // fix them: NaN brightness reads 0 and +inf clamps to 1; any non-finite
    // colour channel reads 0.
    #[test]
    fn volume_factor_clamps_script_brightness_and_colour() {
        let white = [1.0; 3];
        close(volume_factor(0.0, 4.0, 1.5, white), [4.0; 3]);
        close(volume_factor(0.0, 4.0, -0.5, white), [0.0; 3]);
        close(volume_factor(1.0, 4.0, f32::NAN, white), [1.0; 3]);
        close(volume_factor(1.0, 4.0, f32::INFINITY, white), [4.0; 3]);
        close(
            volume_factor(0.0, 1.0, 1.0, [100.0, -1.0, f32::NAN]),
            [1.0, 0.0, 0.0],
        );
        close(volume_factor(0.0, 1.0, 1.0, [f32::INFINITY; 3]), [0.0; 3]);
        close(
            volume_factor(0.0, 1.0, 1.0, [f32::NEG_INFINITY, 0.5, 1.0]),
            [0.0, 0.5, 1.0],
        );
    }

    // The bake's reference state is max × colour: brightness 1 presents
    // exactly `max` and brightness 0 exactly `min`, with no rounding from a
    // scaled span, wherever the range sits.
    #[test]
    fn the_range_endpoints_present_exactly() {
        for (min, max) in [(0.1, 0.7), (0.3, 64.0), (2.5, 2.5), (0.0, 1.0), (7.0, 13.1)] {
            assert_eq!(
                VolumeGlow::scripted(min, max, 1.0, [1.0; 3]).multiplier,
                max
            );
            assert_eq!(
                VolumeGlow::scripted(min, max, 0.0, [1.0; 3]).multiplier,
                min
            );
        }
        let color = [0.9, 0.06, 0.04];
        let expected = color.map(|channel| channel * 13.1);
        assert_eq!(volume_factor(7.0, 13.1, 1.0, color), expected);
    }

    // No script means brightness 1: max × the authored colour. Entry 0 is
    // the identity a non-member face reads.
    #[test]
    fn authored_table_presents_max_times_colour_and_entry_zero_is_identity() {
        let volume = |max: f32, color: [f32; 3]| EmissiveVolumeRecord {
            min: 0.5,
            max,
            color,
            origin: [0.0; 3],
            faces: Vec::new(),
            tags: Vec::new(),
        };
        let bytes = authored_table_bytes(&[volume(3.0, [1.0, 0.0, 0.5]), volume(1.0, [1.0; 3])]);
        assert_eq!(bytes.len(), EMISSIVE_TABLE_BYTES);
        let mut table = EmissiveVolumeTable::new();
        table.bytes.copy_from_slice(&bytes);
        close(table.entry(0), NO_VOLUME_FACTOR);
        close(table.entry(1), [3.0, 0.0, 1.5]);
        close(table.entry(2), [1.0; 3]);
        close(table.entry(3), NO_VOLUME_FACTOR);
    }

    // Ms3: a static frame packs the same factors and leaves nothing pending;
    // one changed volume pends exactly its one entry.
    #[test]
    fn a_static_pack_pends_nothing_and_one_change_pends_one_entry() {
        let mut table = EmissiveVolumeTable::new();
        let mut factors = vec![[1.0; 3]; 3];
        factors[0] = [2.0, 1.0, 0.5];
        table.pack(&factors);
        assert_eq!(spans(&mut table), [(1, 2)]);
        table.mark_uploaded();

        table.pack(&factors);
        assert!(table.pending().runs.is_empty());
        assert_eq!(table.pending().byte_count(), 0);

        factors[2] = [0.0; 3];
        table.pack(&factors);
        assert_eq!(spans(&mut table), [(3, 4)]);
        assert_eq!(table.pending().byte_count(), EMISSIVE_TABLE_ENTRY_BYTES);
        close(table.entry(3), [0.0; 3]);
    }

    // Separate changes upload separately; adjacent ones merge into one run.
    #[test]
    fn dirty_runs_merge_adjacent_entries_only() {
        let mut table = EmissiveVolumeTable::new();
        let mut factors = vec![[1.0; 3]; 6];
        for volume in [0, 1, 4] {
            factors[volume] = [0.5; 3];
        }
        table.pack(&factors);
        assert_eq!(spans(&mut table), [(1, 3), (5, 6)]);
        assert_eq!(table.pending().byte_count(), 3 * EMISSIVE_TABLE_ENTRY_BYTES);
    }

    // A volume packed on its own lands in its entry, pends only on a change,
    // and a later `pack` of fewer volumes leaves it alone; past the cap is
    // ignored.
    #[test]
    fn pack_volume_writes_one_entry_that_a_shorter_pack_leaves_alone() {
        let mut table = EmissiveVolumeTable::new();
        table.pack_volume(4, [2.0, 1.0, 0.5]);
        assert_eq!(spans(&mut table), [(5, 6)]);
        table.mark_uploaded();
        table.pack_volume(4, [2.0, 1.0, 0.5]);
        assert!(table.pending().runs.is_empty());

        table.pack(&[[0.5; 3]; 2]);
        assert_eq!(spans(&mut table), [(1, 3)]);
        close(table.entry(5), [2.0, 1.0, 0.5]);

        table.mark_uploaded();
        table.pack_volume(MAX_EMISSIVE_VOLUMES, [9.0; 3]);
        assert!(table.pending().runs.is_empty());
    }

    // A level change re-sends the whole table once, whatever matched before.
    #[test]
    fn reset_pends_every_entry_back_at_identity() {
        let mut table = EmissiveVolumeTable::new();
        table.pack(&[[3.0; 3]]);
        table.mark_uploaded();
        table.reset();
        assert_eq!(spans(&mut table), [(0, EMISSIVE_TABLE_ENTRIES as u32)]);
        close(table.entry(1), NO_VOLUME_FACTOR);
    }
}
