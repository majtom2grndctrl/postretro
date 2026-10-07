//! Signed height encoding: the stored G byte → a signed, quantized fraction of
//! the material's depth, and the per-material relief band computed at load.
//!
//! The bake stores `G = 255 − h`, where `h` is the authored `_h.png` value.
//! Mid-gray (`h = 128`) is the polygon plane; darker sinks below it, lighter
//! rises above it. The signed fraction is linear in `h`:
//!
//! ```text
//! s = (h − 128) / 128        // s ∈ [−1, 127/128]; positive = RAISED
//! ```
//!
//! Black is exactly −1. White is 127/128, not 1 — that is the price of 128
//! being exactly representable as the plane. Do not "fix" it.

/// Largest stored byte: the unorm scale of the specular slot's G channel.
pub const SURFACE_HEIGHT_BYTE_MAX: f32 = 255.0;
/// Authored byte that sits exactly on the polygon plane (`#808080`), and the
/// divisor that turns a byte offset from it into a fraction of the depth.
pub const SURFACE_HEIGHT_PLANE_BYTE: f32 = 128.0;
/// Highest signed fraction any texel can carry (authored white).
pub const SURFACE_HEIGHT_MAX_RAISE: f32 = 127.0 / 128.0;

/// Signed height fraction of one texel from its stored G value.
///
/// `stored_g` is what `textureLoad(..).g` returns for an `Rg8Unorm` slot:
/// `G / 255`. The byte is recovered with `floor(x + 0.5)` first, so every step
/// after it is integer arithmetic over a power-of-two divisor and therefore
/// exact on every backend. Computing `255 · (1 − g)` directly would not be:
/// `g` is not exactly representable, and a fused multiply-add on the GPU would
/// leave mid-gray a hair off zero.
pub fn surface_height_fraction(stored_g: f32) -> f32 {
    let stored = (stored_g * SURFACE_HEIGHT_BYTE_MAX + 0.5).floor();
    let authored = SURFACE_HEIGHT_BYTE_MAX - stored;
    (authored - SURFACE_HEIGHT_PLANE_BYTE) / SURFACE_HEIGHT_PLANE_BYTE
}

/// Signed height fraction of one texel from its stored G BYTE. Identical to
/// [`surface_height_fraction`] of `byte / 255`; this is the load-time path.
pub fn surface_height_fraction_from_byte(stored_byte: u8) -> f32 {
    let authored = SURFACE_HEIGHT_BYTE_MAX - f32::from(stored_byte);
    (authored - SURFACE_HEIGHT_PLANE_BYTE) / SURFACE_HEIGHT_PLANE_BYTE
}

/// Quantize a signed fraction onto `levels` terraces PER DIRECTION.
///
/// `floor(x + 0.5)`, never a builtin rounding call: WGSL rounds half to even,
/// Rust rounds half away from zero, and an exact half step is reachable
/// (`s = −0.25` at `levels = 6` is `−1.5`). Both sides must agree bit for bit.
///
/// Mid-gray (`s = 0`) quantizes to exactly `0.0` at every level count. `levels`
/// is an f32 because the material uniform carries it as one; below 1 the value
/// passes through untouched.
pub fn surface_height_quantize(s: f32, levels: f32) -> f32 {
    if levels >= 1.0 {
        return ((s * levels + 0.5).floor() / levels).clamp(-1.0, 1.0);
    }
    s
}

/// A material's vertical extent, as signed fractions of its depth.
///
/// `peak_raise` is the highest raise of any texel (`≥ 0`), `trough` the lowest
/// sink (`≤ 0`). The march walks only `[trough, peak_raise]` (P1) and starts at
/// the peak (D6). Raw values come from [`surface_relief_from_rg8_levels`] at
/// load; [`Self::quantized`] applies the material's level count when the
/// uniform is built, and that quantized pair is what the shader reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceRelief {
    pub peak_raise: f32,
    pub trough: f32,
}

impl SurfaceRelief {
    /// No relief: no surface map, or an all-mid-gray one. Marches nothing (P4).
    pub const FLAT: Self = Self {
        peak_raise: 0.0,
        trough: 0.0,
    };

    /// The widest band any map can carry: authored white to authored black.
    pub const FULL_RANGE: Self = Self {
        peak_raise: SURFACE_HEIGHT_MAX_RAISE,
        trough: -1.0,
    };

    /// Quantize both ends with the same rule the texels use.
    ///
    /// The rule is monotonic, so the quantized peak is the peak of the
    /// quantized texels — the band still bounds every texel the march reads.
    /// Out-of-range and NaN inputs are folded into the legal ranges first
    /// (`f32::max`/`min` discard a NaN), so a bad plan degrades to a narrower
    /// band, never to an unbounded one.
    // `max`/`min` rather than `clamp`: `clamp` would keep a NaN.
    #[allow(clippy::manual_clamp)]
    pub fn quantized(self, levels: f32) -> Self {
        let peak = self.peak_raise.max(0.0).min(1.0);
        let trough = self.trough.min(0.0).max(-1.0);
        Self {
            peak_raise: surface_height_quantize(peak, levels),
            trough: surface_height_quantize(trough, levels),
        }
    }

    /// Whether the band has any height at all. An empty band marches nothing.
    pub fn is_flat(self) -> bool {
        self.peak_raise == 0.0 && self.trough == 0.0
    }
}

/// Peak raise and trough, as raw signed fractions, of an `Rg8Unorm` surface
/// map across EVERY uploaded mip level.
///
/// `levels` is the slot's chain as `(width, height, bytes)`, interleaved
/// `[R specular, G stored]` per texel — the shape the renderer already slices
/// for upload. Every level counts: the bake's Mitchell-Netravali filter has
/// negative lobes, so a coarser mip can overshoot the base level's range, and
/// residency may later make any level the one the march reads.
///
/// That overshoot interacts with AO. If a coarse mip rises past the base
/// level's highest texel by enough to move the QUANTIZED peak, the peak sits
/// above every texel the march reads at the resident level. The march stays
/// correct — it starts a little higher than it needs to — but no texel is at
/// the peak any more: stone tops take nonzero AO (D5 measures from the peak),
/// and a top hit no longer skips the self-shadow march, so it spends a budget
/// slot on a march that cannot find an occluder. The shipped `_h.png` assets
/// do not overshoot that far.
///
/// An empty chain is [`SurfaceRelief::FLAT`]. A level whose byte count is not
/// two per texel is a debug assertion; a release build ignores a trailing odd
/// byte.
pub fn surface_relief_from_rg8_levels(levels: &[(u32, u32, &[u8])]) -> SurfaceRelief {
    let mut min_stored = u8::MAX;
    let mut max_stored = u8::MIN;
    let mut any = false;
    for &(width, height, bytes) in levels {
        debug_assert_eq!(
            bytes.len(),
            2 * width as usize * height as usize,
            "an Rg8Unorm level is two bytes per texel",
        );
        for texel in bytes.as_chunks::<2>().0 {
            min_stored = min_stored.min(texel[1]);
            max_stored = max_stored.max(texel[1]);
            any = true;
        }
    }
    if !any {
        return SurfaceRelief::FLAT;
    }
    // Stored G is inverted: the SMALLEST stored byte is the highest raise.
    SurfaceRelief {
        peak_raise: surface_height_fraction_from_byte(min_stored).max(0.0),
        trough: surface_height_fraction_from_byte(max_stored).min(0.0),
    }
}

/// A piecewise-constant height field: one stored G value per texel, tiled
/// (`AddressMode::Repeat`, no atlas — marching `base_uv` freely is safe).
#[derive(Debug, Clone, Copy)]
pub struct SurfaceDepthField<'a> {
    pub width: i32,
    pub height: i32,
    /// Row-major, `width * height` entries: the specular slot's G channel as
    /// the shader reads it, `G / 255`.
    pub stored_g: &'a [f32],
    /// Terraces per direction; see [`surface_height_quantize`].
    pub quantize_levels: u32,
}

impl SurfaceDepthField<'_> {
    /// Wrapped, quantized signed height fraction of texel `(x, y)`.
    ///
    /// Quantization does not affect the DDA's exactness — that comes from
    /// per-texel constancy, not from the value being on a plateau.
    pub fn texel_height(&self, x: i32, y: i32) -> f32 {
        let wrap = |v: i32, n: i32| {
            let m = v % n;
            if m < 0 { m + n } else { m }
        };
        let x = wrap(x, self.width);
        let y = wrap(y, self.height);
        let stored = self.stored_g[(y * self.width + x) as usize];
        surface_height_quantize(surface_height_fraction(stored), self.quantize_levels as f32)
    }
}
