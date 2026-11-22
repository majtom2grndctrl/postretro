// Emissive volume glow factors and the forward shader's volume table, packed on
// CPU with per-entry change tracking so a static frame uploads nothing.
// See: context/lib/rendering_pipeline.md §7.3

use std::ops::Range;

use postretro_level_format::area_glow::AreaGlowLight;
pub use postretro_level_format::area_glow::{AREA_GLOW_FIRST_ENTRY, GLOW_TABLE_ENTRIES};
use postretro_level_format::emissive::{
    EMISSIVE_VALUE_CEILING, EmissiveVolumeRecord, MAX_EMISSIVE_VOLUMES,
};

use crate::sh_compose::{
    AnimatedLightScaleDescriptor, animation_curve_t, sample_color_catmull_rom,
    sample_curve_catmull_rom,
};

/// Table entries: entry 0 is "no volume", entry `i + 1` is volume `i`, so the
/// per-vertex stream's `volume index + 1` indexes it directly.
pub const EMISSIVE_TABLE_ENTRIES: usize = MAX_EMISSIVE_VOLUMES + 1;
/// One `vec4<f32>` per entry: the rgb factor, then an unused lane.
pub const EMISSIVE_TABLE_ENTRY_BYTES: usize = 16;
/// The whole uniform the forward shader declares (`array<vec4<f32>, 257>`).
pub const EMISSIVE_TABLE_BYTES: usize = EMISSIVE_TABLE_ENTRIES * EMISSIVE_TABLE_ENTRY_BYTES;
/// The factor a face in no volume presents: its texture strength unchanged.
pub const NO_VOLUME_FACTOR: [f32; 3] = [1.0; 3];
/// The whole uniform the forward shader declares (`array<vec4<f32>, 4096>`):
/// the [`EMISSIVE_TABLE_ENTRIES`] volume entries the per-frame packer owns,
/// then one entry per area-light face, written once at install, then zeros.
pub const GLOW_TABLE_BYTES: usize = GLOW_TABLE_ENTRIES * EMISSIVE_TABLE_ENTRY_BYTES;
/// An area entry's `w` lane when the face draws its static glow: no compose
/// descriptor slot. A slotted entry stores its slot as an exact float.
pub const AREA_GLOW_STATIC: f32 = -1.0;

/// One area-light face's table entry, as `forward.wgsl` `area_glow_factor`
/// reads it.
///
/// - Static (no compose slot): `(glow × r, glow × g, glow × b, -1)`, drawn
///   unchanged on every frame (research.md O16).
/// - Slotted: `(glow, 1 / intensity, 0, slot)`. The shader evaluates the
///   light's compose descriptor, whose base colour carries colour ×
///   intensity, and divides the intensity back out, so the glow follows the
///   light's live colour and brightness but never `light` itself (O2, O13).
/// - Slotted at intensity 0 (`light 0`): `(glow, 0, packed colour, slot)`.
///   The descriptor's base colour is 0, so the `1 / intensity` lane is 0 and
///   the colour rides lane z instead, from [`pack_area_glow_color`]. The
///   glow still follows the light's active state and brightness curve, and a
///   colour curve while one plays.
pub fn area_glow_entry(light: &AreaGlowLight) -> [f32; 4] {
    match light.compose_slot {
        None => [
            light.glow * light.color[0],
            light.glow * light.color[1],
            light.glow * light.color[2],
            AREA_GLOW_STATIC,
        ],
        Some(slot) if light.intensity > 0.0 => {
            [light.glow, 1.0 / light.intensity, 0.0, slot as f32]
        }
        Some(slot) => [
            light.glow,
            0.0,
            pack_area_glow_color(light.color),
            slot as f32,
        ],
    }
}

/// A `light 0` entry's colour lane: each channel as an 8-bit integer, red in
/// the low byte, as an exact float (at most 2²⁴ − 1). `_color` is authored
/// in 8 bits per channel, so the packing loses nothing.
pub fn pack_area_glow_color(color: [f32; 3]) -> f32 {
    let byte = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round() as u32;
    (byte(color[0]) | byte(color[1]) << 8 | byte(color[2]) << 16) as f32
}

/// [`pack_area_glow_color`]'s inverse, as `forward.wgsl` decodes lane z.
pub fn unpack_area_glow_color(lane: f32) -> [f32; 3] {
    let packed = lane as u32;
    std::array::from_fn(|channel| ((packed >> (8 * channel)) & 0xff) as f32 / 255.0)
}

/// The table entries past the volumes: one per emitting face of id 62's
/// `lights`, in the section's entry order, so face entry `k` is table entry
/// `AREA_GLOW_FIRST_ENTRY + k`.
pub fn area_glow_entries(lights: &[AreaGlowLight]) -> Vec<[f32; 4]> {
    lights
        .iter()
        .flat_map(|light| std::iter::repeat_n(area_glow_entry(light), light.faces.len()))
        .collect()
}

/// The whole table a freshly installed level binds: the volumes at their
/// no-script state, then `area_entries`, then zeros. Entries past the table
/// are dropped; the compiler and the loader refuse a section that has them.
pub fn glow_table_bytes(volumes: &[EmissiveVolumeRecord], area_entries: &[[f32; 4]]) -> Vec<u8> {
    let mut bytes = authored_table_bytes(volumes);
    bytes.resize(GLOW_TABLE_BYTES, 0);
    for (k, entry) in area_entries
        .iter()
        .take(GLOW_TABLE_ENTRIES - AREA_GLOW_FIRST_ENTRY)
        .enumerate()
    {
        let at = (AREA_GLOW_FIRST_ENTRY + k) * EMISSIVE_TABLE_ENTRY_BYTES;
        for (lane, value) in entry.iter().enumerate() {
            bytes[at + lane * 4..at + lane * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

/// CPU mirror of `forward.wgsl` `area_glow_factor`: the rgb factor an area
/// face's `_e` sample is multiplied by. `descriptor` is the entry's compose
/// descriptor (ignored for a static entry). A static entry's channels are
/// `_glow × _color`, at most 64 since `_color` is at most 1. A slotted entry
/// reads 0 while its light is inactive; otherwise `min(glow × brightness,
/// 64) ×` its live colour, each channel capped at 64, the emissive ceiling
/// that keeps bloom under the half-float range even when a curve runs past
/// one. The live colour is the descriptor's, intensity divided out, or for a
/// `light 0` entry its packed `_color`, or its colour curve while one plays.
pub fn area_glow_factor(
    entry: [f32; 4],
    descriptor: Option<AnimatedLightScaleDescriptor<'_>>,
    time: f32,
) -> [f32; 3] {
    if entry[3] < 0.0 {
        return [entry[0], entry[1], entry[2]];
    }
    let Some(descriptor) = descriptor.filter(|descriptor| descriptor.is_active) else {
        return [0.0; 3];
    };
    let cycle_t = animation_curve_t(descriptor.period, descriptor.phase, time);
    let brightness = sample_curve_catmull_rom(descriptor.brightness, cycle_t).max(0.0);
    // A lit light's descriptor carries colour × intensity, which lane y
    // (1 / intensity) divides out. A `light 0` light's carries nothing.
    let (base, unit) = if entry[1] > 0.0 {
        (descriptor.base_color, entry[1])
    } else if descriptor.color.is_empty() {
        (unpack_area_glow_color(entry[2]), 1.0)
    } else {
        ([1.0; 3], 1.0)
    };
    let color = if descriptor.color.is_empty() {
        base
    } else {
        let sampled = sample_color_catmull_rom(descriptor.color, cycle_t);
        std::array::from_fn(|channel| sampled[channel].max(0.0) * base[channel])
    };
    let scale = (entry[0] * brightness).min(EMISSIVE_VALUE_CEILING) * unit;
    color.map(|channel| (channel * scale).min(EMISSIVE_VALUE_CEILING))
}

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
    use postretro_level_format::area_glow::AreaGlowSection;

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

    fn light(glow: f32, color: [f32; 3], intensity: f32, slot: Option<u32>) -> AreaGlowLight {
        AreaGlowLight {
            glow,
            color,
            intensity,
            compose_slot: slot,
            faces: vec![0],
        }
    }

    /// A compose descriptor as the compiler bakes one: base colour × intensity,
    /// or the intensity splat under a colour curve.
    fn descriptor<'a>(
        color: [f32; 3],
        intensity: f32,
        brightness: &'a [f32],
        colors: &'a [[f32; 3]],
        is_active: bool,
    ) -> AnimatedLightScaleDescriptor<'a> {
        AnimatedLightScaleDescriptor {
            period: 1.0,
            phase: 0.0,
            base_color: if colors.is_empty() {
                color.map(|channel| channel * intensity)
            } else {
                [intensity; 3]
            },
            brightness,
            color: colors,
            is_active,
        }
    }

    fn close_rel(a: [f32; 3], b: [f32; 3], tolerance: f32) {
        for (x, y) in a.into_iter().zip(b) {
            assert!(
                (x - y).abs() <= tolerance * y.abs().max(1e-6),
                "{a:?} vs {b:?}"
            );
        }
    }

    // Acceptance row 59 (research.md O16): a static area light's entry draws
    // `_glow × _color` on every frame, whatever any descriptor does, while an
    // animated light's follows its pulse.
    #[test]
    fn a_static_entry_holds_its_glow_while_an_animated_one_pulses() {
        let red = [1.0, 0.0, 0.0];
        let fixed = area_glow_entry(&light(4.0, red, 1.0, None));
        assert_eq!(fixed, [4.0, 0.0, 0.0, AREA_GLOW_STATIC]);
        let pulsing = area_glow_entry(&light(4.0, [1.0; 3], 2.0, Some(0)));
        let curve = [0.2, 1.0];
        let slot0 = descriptor([1.0; 3], 2.0, &curve, &[], true);
        let mut seen = Vec::new();
        for step in 0..8 {
            let time = step as f32 / 8.0;
            assert_eq!(area_glow_factor(fixed, Some(slot0), time), [4.0, 0.0, 0.0]);
            seen.push(area_glow_factor(pulsing, Some(slot0), time)[0]);
        }
        let (low, high) = seen
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        assert!(
            low < 1.0 && high > 3.9,
            "the animated face pulses: {seen:?}"
        );
    }

    // Acceptance row 49: under a colour shift the glow's colour and the room's
    // colour move together: the glow is the room's live scale × `_glow / L`.
    #[test]
    fn a_colour_shift_moves_the_glow_and_the_room_together() {
        let intensity = 1.5;
        let glow = 6.0;
        let entry = area_glow_entry(&light(glow, [1.0; 3], intensity, Some(0)));
        let colors = [[1.0, 1.0, 1.0], [1.0, 0.6, 0.1], [0.2, 0.4, 1.0]];
        let shifting = descriptor([1.0; 3], intensity, &[], &colors, true);
        for step in 0..12 {
            let time = step as f32 / 12.0;
            let room = crate::sh_compose::animated_light_scale(Some(shifting), time);
            let expected = room.map(|channel| channel * glow / intensity);
            close_rel(
                area_glow_factor(entry, Some(shifting), time),
                expected,
                1e-5,
            );
        }
    }

    // Acceptance rows 50 and 48 (glow half, research.md O2): doubling `light`
    // doubles the room's descriptor scale and leaves the glow unchanged.
    #[test]
    fn doubling_light_leaves_the_glow_unchanged() {
        let color = [1.0, 0.5, 0.25];
        let curve = [0.25, 1.0, 0.5];
        for time in [0.0, 0.3, 0.7] {
            let one = descriptor(color, 1.0, &curve, &[], true);
            let two = descriptor(color, 2.0, &curve, &[], true);
            let glow_one = area_glow_factor(
                area_glow_entry(&light(4.0, color, 1.0, Some(0))),
                Some(one),
                time,
            );
            let glow_two = area_glow_factor(
                area_glow_entry(&light(4.0, color, 2.0, Some(0))),
                Some(two),
                time,
            );
            close_rel(glow_two, glow_one, 1e-6);
            let room_one = crate::sh_compose::animated_light_scale(Some(one), time);
            let room_two = crate::sh_compose::animated_light_scale(Some(two), time);
            close_rel(room_two, room_one.map(|channel| channel * 2.0), 1e-6);
        }
    }

    // A dark light draws dark faces; the live factor stops at the emissive
    // ceiling even when a curve runs past 1.
    #[test]
    fn inactive_lights_are_dark_and_the_live_glow_stops_at_the_ceiling() {
        let entry = area_glow_entry(&light(4.0, [1.0; 3], 1.0, Some(3)));
        let dark = descriptor([1.0; 3], 1.0, &[], &[], false);
        assert_eq!(area_glow_factor(entry, Some(dark), 0.0), [0.0; 3]);
        assert_eq!(area_glow_factor(entry, None, 0.0), [0.0; 3]);
        let hot = area_glow_entry(&light(64.0, [1.0, 0.5, 0.0], 1.0, Some(0)));
        let over = descriptor([1.0, 0.5, 0.0], 1.0, &[3.0], &[], true);
        assert_eq!(area_glow_factor(hot, Some(over), 0.0), [64.0, 32.0, 0.0]);
    }

    // Every 8-bit `_color` packs into lane z and back exactly.
    #[test]
    fn a_light_zero_colour_packs_exactly() {
        for color in [[0, 0, 0], [255, 128, 0], [1, 254, 255], [255; 3]] {
            let linear = color.map(|channel: u32| channel as f32 / 255.0);
            let lane = pack_area_glow_color(linear);
            assert_eq!(lane, (color[0] | color[1] << 8 | color[2] << 16) as f32);
            assert_eq!(unpack_area_glow_color(lane), linear);
        }
    }

    // A `light 0` light keeps its slot, so its glow still follows the light:
    // dark while inactive, scaled by its brightness curve and capped at the
    // ceiling as a lit light's is, coloured by `_color`, and by a colour
    // curve while one plays. Its descriptor's base colour is 0 (colour ×
    // intensity), so nothing is divided out.
    #[test]
    fn a_light_zero_entry_follows_its_light_in_its_own_colour() {
        let amber = [1.0, 128.0 / 255.0, 0.0];
        let unlit = light(4.0, amber, 0.0, Some(3));
        let entry = area_glow_entry(&unlit);
        assert_eq!(
            entry,
            [4.0, 0.0, pack_area_glow_color(amber), 3.0],
            "a slotted entry with no intensity lane"
        );

        // Starts inactive: dark, faces and room together.
        let dark = descriptor(amber, 0.0, &[], &[], false);
        assert_eq!(area_glow_factor(entry, Some(dark), 0.0), [0.0; 3]);
        assert_eq!(
            crate::sh_compose::animated_light_scale(Some(dark), 0.0),
            [0.0; 3]
        );

        // Lit and curveless: `_glow × _color`.
        let lit = descriptor(amber, 0.0, &[], &[], true);
        assert_eq!(
            area_glow_factor(entry, Some(lit), 0.0),
            amber.map(|channel| channel * 4.0)
        );

        // A pulse scales it as it scales a lit light's glow.
        let curve = [0.25, 1.0, 0.5];
        let pulse = descriptor(amber, 0.0, &curve, &[], true);
        let lit_pulse = descriptor(amber, 2.0, &curve, &[], true);
        let lit_entry = area_glow_entry(&light(4.0, amber, 2.0, Some(3)));
        for step in 0..12 {
            let time = step as f32 / 12.0;
            close_rel(
                area_glow_factor(entry, Some(pulse), time),
                area_glow_factor(lit_entry, Some(lit_pulse), time),
                1e-6,
            );
        }

        // A colour curve replaces `_color`, as on a lit light.
        let colors = [[1.0, 1.0, 1.0], [1.0, 0.6, 0.1], [0.2, 0.4, 1.0]];
        let shifting = descriptor(amber, 0.0, &[], &colors, true);
        let lit_shifting = descriptor(amber, 1.5, &[], &colors, true);
        let lit_entry = area_glow_entry(&light(4.0, amber, 1.5, Some(3)));
        for step in 0..12 {
            let time = step as f32 / 12.0;
            close_rel(
                area_glow_factor(entry, Some(shifting), time),
                area_glow_factor(lit_entry, Some(lit_shifting), time),
                1e-5,
            );
        }

        // The ceiling holds as on a lit light.
        let hot = area_glow_entry(&light(64.0, [1.0, 0.5, 0.0], 0.0, Some(0)));
        let over = descriptor([1.0, 0.5, 0.0], 0.0, &[3.0], &[], true);
        let half = 128.0 / 255.0;
        assert_eq!(
            area_glow_factor(hot, Some(over), 0.0),
            [64.0, 64.0 * half, 0.0]
        );
    }

    // The install table: volumes first, area entries from entry 257, zeros
    // after, 64 KiB in all.
    #[test]
    fn the_install_table_places_area_entries_after_the_volumes() {
        let section = AreaGlowSection {
            lights: vec![
                AreaGlowLight {
                    faces: vec![7, 9],
                    ..light(2.0, [1.0, 0.0, 0.0], 1.0, None)
                },
                AreaGlowLight {
                    faces: vec![8],
                    ..light(4.0, [1.0; 3], 2.0, Some(5))
                },
            ],
        };
        let entries = area_glow_entries(&section.lights);
        assert_eq!(entries.len(), 3);
        let bytes = glow_table_bytes(&[], &entries);
        assert_eq!(bytes.len(), GLOW_TABLE_BYTES);
        assert_eq!(GLOW_TABLE_BYTES, 65536);
        let entry = |index: usize| -> [f32; 4] {
            let at = index * EMISSIVE_TABLE_ENTRY_BYTES;
            std::array::from_fn(|lane| {
                f32::from_le_bytes(bytes[at + lane * 4..at + lane * 4 + 4].try_into().unwrap())
            })
        };
        assert_eq!(entry(0)[..3], NO_VOLUME_FACTOR);
        assert_eq!(
            entry(AREA_GLOW_FIRST_ENTRY),
            [2.0, 0.0, 0.0, AREA_GLOW_STATIC]
        );
        assert_eq!(
            entry(AREA_GLOW_FIRST_ENTRY + 1),
            [2.0, 0.0, 0.0, AREA_GLOW_STATIC]
        );
        assert_eq!(entry(AREA_GLOW_FIRST_ENTRY + 2), [4.0, 0.5, 0.0, 5.0]);
        assert_eq!(entry(AREA_GLOW_FIRST_ENTRY + 3), [0.0; 4]);
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
