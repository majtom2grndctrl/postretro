// The world bake's specular slot: its dimension bails and its R8, RG8 or RGBA8 interleave.
// See: context/lib/resource_management.md §4.6, §4.7 · context/lib/build_pipeline.md §Baked texture mips

use std::path::{Path, PathBuf};

use super::{
    PrmFormat, PrmSlot, SMOOTHNESS_ABSENT, SMOOTHNESS_MIN, SurfaceMarkers, TINT_ABSENT, TINT_MAX,
    build_specular_chain, build_surface_chain, decode_png_rgba, expected_level_count,
    png_dimensions,
};

/// Stored G where a material has no `_h.png`: the mid-gray plane (128)
/// inverted as the bake inverts height. The runtime measures an all-plane
/// slot as an empty relief band, the same bytes as no height map.
const HEIGHT_PLANE_STORED: u8 = 255 - 128;

/// One present source PNG: its raw bytes and where they came from.
#[derive(Clone, Copy)]
pub(super) struct SourcePng<'a> {
    pub bytes: &'a [u8],
    pub path: &'a Path,
}

impl<'a> SourcePng<'a> {
    /// The source, when its sibling resolved and was read.
    pub fn of(bytes: &'a Option<Vec<u8>>, path: &'a Option<PathBuf>) -> Option<Self> {
        Some(Self {
            bytes: bytes.as_deref()?,
            path: path.as_deref()?,
        })
    }
}

/// The siblings that bake into the specular slot, `None` where absent.
pub(super) struct SpecularSources<'a> {
    pub specular: Option<SourcePng<'a>>,
    pub height: Option<SourcePng<'a>>,
    pub smoothness: Option<SourcePng<'a>>,
    pub tint: Option<SourcePng<'a>>,
}

/// Bail unless every sibling interleaved beside `_s.png` matches the diffuse
/// and each earlier slot sibling: `_h` against diffuse and `_s`; `_p` against
/// diffuse, `_s` and `_h`; `_t` against diffuse, `_s`, `_h` and `_p`. These are
/// hard bails, the way `_e` is: the siblings share one texture, and the shader
/// reads them on the diffuse's lattice. `_s` and `_n` against the diffuse
/// alone stay unenforced, so a mismatched `_s` still builds without `_h`, `_p`
/// or `_t`. The message names both files.
pub(super) fn require_matching_slot_dimensions(
    name: &str,
    diffuse: Option<SourcePng<'_>>,
    slot: &SpecularSources<'_>,
) -> anyhow::Result<()> {
    let inputs = [
        ("diffuse", "diffuse", diffuse),
        ("specular", "_s.png", slot.specular),
        ("height", "_h.png", slot.height),
        ("smoothness", "_p.png", slot.smoothness),
        ("tint", "_t.png", slot.tint),
    ];
    for (index, (kind, suffix, png)) in inputs.iter().enumerate().skip(2) {
        let Some(png) = png else {
            continue;
        };
        let (width, height) = png_dimensions(png.bytes, png.path)?;
        for (other_kind, other_suffix, other) in &inputs[..index] {
            let Some(other) = other else {
                continue;
            };
            let (other_width, other_height) = png_dimensions(other.bytes, other.path)?;
            if (width, height) == (other_width, other_height) {
                continue;
            }
            let rule = if *other_kind == "diffuse" {
                format!("{suffix} dimensions must match diffuse")
            } else {
                format!(
                    "{suffix} and {other_suffix} share one specular slot and must have \
                     identical dimensions"
                )
            };
            anyhow::bail!(
                "{kind} texture {} is {width}x{height}, but {other_kind} texture {} for \
                 '{name}' is {other_width}x{other_height}; {rule}",
                png.path.display(),
                other.path.display(),
            );
        }
    }
    Ok(())
}

/// Slot 1 in whichever form its siblings call for, or `None` when none is
/// present. The SPECULAR slot-mask bit follows `Some`: any of `_s`, `_h`, `_p`
/// or `_t` bakes the slot.
///
/// - `_s` alone: single-channel `R8Unorm`, byte-identical to what it has
///   always baked.
/// - `_h` without `_p` or `_t`: two-channel `Rg8Unorm`, R specular (0 without
///   `_s`), G the inverted height byte (`255 - height`).
/// - `_p` or `_t`: four-channel `Rgba8Unorm`, R and G as above (G the stored
///   plane without `_h`), B smoothness as max(p, 1) (0 without `_p`), A tint as
///   min(t, 254) (255 without `_t`). Every mip keeps B and A off or on their
///   markers.
///
/// Height carries no zero point at bake time: the runtime re-centers on
/// mid-gray. The bundle hash covers source PNG bytes only, so a changed
/// transform would silently reuse stale sidecars; changing the height
/// inversion, either marker or either marker clamp bumps `STAGE_VERSION`.
/// Callers run [`require_matching_slot_dimensions`] first.
pub(super) fn bake_specular_slot(slot: &SpecularSources<'_>) -> anyhow::Result<Option<PrmSlot>> {
    let markers = SurfaceMarkers {
        smoothness: slot.smoothness.is_some(),
        tint: slot.tint.is_some(),
    };
    let (channels, format) = if markers.smoothness || markers.tint {
        (4, PrmFormat::Rgba8Unorm)
    } else if slot.height.is_some() {
        (2, PrmFormat::Rg8Unorm)
    } else {
        let Some(specular) = slot.specular else {
            return Ok(None);
        };
        // PNG authoring is typically L8 or RGBA8 with the spec data in R; we
        // accept either.
        let (red, width, height) = decode_red(specular)?;
        return Ok(Some(PrmSlot {
            format: PrmFormat::R8Unorm,
            width: width as u16,
            height: height as u16,
            level_count: expected_level_count(width as u16, height as u16),
            payload: build_specular_chain(&red, width, height),
        }));
    };

    let specular = slot.specular.map(decode_red).transpose()?;
    let height = slot.height.map(decode_red).transpose()?;
    let smoothness = slot.smoothness.map(decode_red).transpose()?;
    let tint = slot.tint.map(decode_red).transpose()?;
    // The dimension bails proved every present sibling agrees, and a widened
    // slot has at least one of `_h`, `_p`, `_t`.
    let Some((width, height_texels)) = [&specular, &height, &smoothness, &tint]
        .into_iter()
        .flatten()
        .map(|(_, width, height)| (*width, *height))
        .next()
    else {
        unreachable!("a widened specular slot has a height, smoothness or tint sibling");
    };

    let byte = |map: &Option<(Vec<u8>, u32, u32)>, texel: usize| {
        map.as_ref().map(|(red, _, _)| red[texel])
    };
    let texel_count = width as usize * height_texels as usize;
    let mut texels = Vec::with_capacity(texel_count * channels);
    for texel in 0..texel_count {
        texels.push(byte(&specular, texel).unwrap_or(0));
        texels.push(byte(&height, texel).map_or(HEIGHT_PLANE_STORED, |h| 255 - h));
        if channels == 4 {
            texels.push(
                byte(&smoothness, texel).map_or(SMOOTHNESS_ABSENT, |p| p.max(SMOOTHNESS_MIN)),
            );
            texels.push(byte(&tint, texel).map_or(TINT_ABSENT, |t| t.min(TINT_MAX)));
        }
    }

    Ok(Some(PrmSlot {
        format,
        width: width as u16,
        height: height_texels as u16,
        level_count: expected_level_count(width as u16, height_texels as u16),
        payload: build_surface_chain(&texels, channels, width, height_texels, markers),
    }))
}

/// The R channel of a slot sibling, with its dimensions.
fn decode_red(png: SourcePng<'_>) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    let (rgba, width, height) = decode_png_rgba(png.bytes, png.path)?;
    let red = rgba.as_chunks::<4>().0.iter().map(|c| c[0]).collect();
    Ok((red, width, height))
}
