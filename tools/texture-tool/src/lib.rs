use image::{imageops, GenericImageView, ImageBuffer, Rgba};
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

pub const DEFAULT_SPEC_SCALE: f32 = 0.2;
pub const DEFAULT_NORMAL_STRENGTH: f32 = 0.5;
pub const DEFAULT_TEXTURE_SIZE: TextureDimensions = TextureDimensions {
    width: 128,
    height: 128,
};
const LOW_FREQUENCY_FLATTEN_STRENGTH: f32 = 0.25;

#[derive(Clone, Debug)]
pub struct TextureJob {
    pub src: PathBuf,
    pub stem: String,
    pub width: u32,
    pub height: u32,
    pub tileable: bool,
    pub spec_scale: f32,
    pub spec_profile: SpecProfile,
    pub spec_base: Option<f32>,
    pub spec_gamma: Option<f32>,
    pub spec_edge_damping: Option<f32>,
    pub normal_strength: f32,
    pub quantize_levels: Option<u8>,
    pub height_strength: Option<f32>,
    pub height_quantize_levels: Option<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpecProfile {
    Luminance,
    Matte,
    Concrete,
    PolishedStone,
    PaintedMetal,
    Glass,
    Screen,
    Water,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextureDimensions {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug)]
struct SpecProfileDefaults {
    base: f32,
    gamma: f32,
    edge_damping: f32,
    response: f32,
    luma_weight: f32,
    max_value: f32,
}

/// Per-profile defaults for `_h.png` height-map generation. Mirrors
/// `SpecProfileDefaults`: the material profile that shapes how a surface
/// reflects light also shapes how pronounced its authored depth should read.
/// A cobblestone/stone-like profile wants pronounced, plateau-like steps; a
/// smooth glass or metal profile wants almost none.
#[derive(Clone, Copy, Debug)]
struct HeightProfileDefaults {
    /// Contrast multiplier applied to the diffuse luminance signal around its
    /// mean before quantization. `1.0` passes the luminance signal through
    /// unmodified; values above `1.0` exaggerate high/low texels away from
    /// the mean (pronounced relief); values below `1.0` pull texels back
    /// toward the mean (a nearly flat read).
    strength: f32,
}

#[derive(Clone, Copy, Debug)]
struct SpecConfig {
    profile: SpecProfile,
    scale: f32,
    base: f32,
    gamma: f32,
    edge_damping: f32,
    response: f32,
    luma_weight: f32,
    max_value: f32,
    has_new_overrides: bool,
}

impl SpecProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Luminance => "luminance",
            Self::Matte => "matte",
            Self::Concrete => "concrete",
            Self::PolishedStone => "polished-stone",
            Self::PaintedMetal => "painted-metal",
            Self::Glass => "glass",
            Self::Screen => "screen",
            Self::Water => "water",
        }
    }

    fn defaults(self) -> SpecProfileDefaults {
        match self {
            Self::Luminance => SpecProfileDefaults {
                base: 0.0,
                gamma: 1.7,
                edge_damping: 0.0,
                response: 1.0,
                luma_weight: 1.0,
                max_value: 96.0 / 255.0,
            },
            Self::Matte => SpecProfileDefaults {
                base: 0.015,
                gamma: 2.4,
                edge_damping: 0.8,
                response: 0.15,
                luma_weight: 1.0,
                max_value: 0.18,
            },
            Self::Concrete => SpecProfileDefaults {
                base: 0.025,
                gamma: 2.1,
                edge_damping: 0.75,
                response: 0.25,
                luma_weight: 1.0,
                max_value: 0.24,
            },
            Self::PolishedStone => SpecProfileDefaults {
                base: 0.12,
                gamma: 1.6,
                edge_damping: 0.65,
                response: 0.75,
                luma_weight: 0.85,
                max_value: 0.65,
            },
            Self::PaintedMetal => SpecProfileDefaults {
                base: 0.22,
                gamma: 1.3,
                edge_damping: 0.45,
                response: 1.1,
                luma_weight: 0.7,
                max_value: 0.78,
            },
            Self::Glass => SpecProfileDefaults {
                base: 0.55,
                gamma: 1.0,
                edge_damping: 0.25,
                response: 1.0,
                luma_weight: 0.25,
                max_value: 1.0,
            },
            Self::Screen => SpecProfileDefaults {
                base: 0.5,
                gamma: 1.0,
                edge_damping: 0.35,
                response: 1.15,
                luma_weight: 0.15,
                max_value: 1.0,
            },
            Self::Water => SpecProfileDefaults {
                base: 0.38,
                gamma: 1.0,
                edge_damping: 0.3,
                response: 1.25,
                luma_weight: 0.35,
                max_value: 1.0,
            },
        }
    }

    /// Per-profile default height/depth strength. See `HeightProfileDefaults`.
    fn height_defaults(self) -> HeightProfileDefaults {
        match self {
            // Backward-compatible/neutral: pass the luminance signal through.
            Self::Luminance => HeightProfileDefaults { strength: 1.0 },
            Self::Matte => HeightProfileDefaults { strength: 0.5 },
            Self::Concrete => HeightProfileDefaults { strength: 1.1 },
            // Cobblestone/masonry-style profile: pronounced, plateau-like steps.
            Self::PolishedStone => HeightProfileDefaults { strength: 1.5 },
            // Smooth metal panel: almost no authored depth.
            Self::PaintedMetal => HeightProfileDefaults { strength: 0.25 },
            Self::Glass => HeightProfileDefaults { strength: 0.1 },
            Self::Screen => HeightProfileDefaults { strength: 0.15 },
            // A liquid plane reads as flat.
            Self::Water => HeightProfileDefaults { strength: 0.05 },
        }
    }
}

impl FromStr for SpecProfile {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "luminance" => Ok(Self::Luminance),
            "matte" => Ok(Self::Matte),
            "concrete" => Ok(Self::Concrete),
            "polished-stone" => Ok(Self::PolishedStone),
            "painted-metal" => Ok(Self::PaintedMetal),
            "glass" => Ok(Self::Glass),
            "screen" => Ok(Self::Screen),
            "water" => Ok(Self::Water),
            _ => Err(format!(
                "spec_profile must be one of luminance, matte, concrete, polished-stone, painted-metal, glass, screen, or water; got {value:?}"
            )),
        }
    }
}

impl SpecConfig {
    fn from_job(job: &TextureJob) -> Self {
        let defaults = job.spec_profile.defaults();
        Self {
            profile: job.spec_profile,
            scale: job.spec_scale,
            base: job.spec_base.unwrap_or(defaults.base),
            gamma: job.spec_gamma.unwrap_or(defaults.gamma),
            edge_damping: job.spec_edge_damping.unwrap_or(defaults.edge_damping),
            response: defaults.response,
            luma_weight: defaults.luma_weight,
            max_value: defaults.max_value,
            has_new_overrides: job.spec_base.is_some()
                || job.spec_gamma.is_some()
                || job.spec_edge_damping.is_some(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct HeightConfig {
    strength: f32,
    quantize_levels: u8,
}

impl HeightConfig {
    fn from_job(job: &TextureJob) -> Result<Self, String> {
        let defaults = job.spec_profile.height_defaults();
        Ok(Self {
            strength: job.height_strength.unwrap_or(defaults.strength),
            quantize_levels: job.resolved_height_quantize_levels()?,
        })
    }
}

impl TextureJob {
    pub fn resolved_quantize_levels(&self) -> Result<u8, String> {
        let levels = self.quantize_levels.unwrap_or_else(|| {
            default_quantize_levels(TextureDimensions {
                width: self.width,
                height: self.height,
            })
        });
        if levels < 2 {
            return Err(format!(
                "quantize_levels must be blank, 0, or at least 2 for {}",
                self.stem
            ));
        }
        Ok(levels)
    }

    pub fn resolved_height_quantize_levels(&self) -> Result<u8, String> {
        let levels = self.height_quantize_levels.unwrap_or_else(|| {
            default_quantize_levels(TextureDimensions {
                width: self.width,
                height: self.height,
            })
        });
        if levels < 2 {
            return Err(format!(
                "height_quantize_levels must be blank, 0, or at least 2 for {}",
                self.stem
            ));
        }
        Ok(levels)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.src.as_os_str().is_empty() {
            return Err("src must not be blank".to_string());
        }
        if self.stem.trim().is_empty() {
            return Err("stem must not be blank".to_string());
        }
        if self.width == 0 || self.height == 0 {
            return Err(format!(
                "size dimensions must be greater than 0 for {}",
                self.stem
            ));
        }
        validate_non_negative_finite("spec_scale", self.spec_scale)?;
        validate_optional_unit("spec_base", self.spec_base)?;
        validate_optional_positive_finite("spec_gamma", self.spec_gamma)?;
        validate_optional_unit("spec_edge_damping", self.spec_edge_damping)?;
        validate_non_negative_finite("normal_strength", self.normal_strength)?;
        self.resolved_quantize_levels()?;
        validate_optional_non_negative_finite("height_strength", self.height_strength)?;
        self.resolved_height_quantize_levels()?;
        Ok(())
    }
}

pub fn default_quantize_levels(size: TextureDimensions) -> u8 {
    if size.width == 64 && size.height == 64 {
        18
    } else {
        24
    }
}

pub fn parse_size(value: &str) -> Result<TextureDimensions, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("size must not be blank".to_string());
    }

    if let Some((width, height)) = value.split_once(['x', 'X']) {
        let width = parse_dimension(width, "width")?;
        let height = parse_dimension(height, "height")?;
        return Ok(TextureDimensions { width, height });
    }

    let side = parse_dimension(value, "size")?;
    Ok(TextureDimensions {
        width: side,
        height: side,
    })
}

pub fn parse_optional_size(value: &str) -> Result<Option<TextureDimensions>, String> {
    if value.trim().is_empty() {
        Ok(None)
    } else {
        parse_size(value).map(Some)
    }
}

pub fn read_manifest(path: &Path) -> Result<Vec<TextureJob>, Box<dyn Error>> {
    let contents = fs::read_to_string(path)?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new(""));
    let mut jobs = Vec::new();

    for (line_index, line) in contents.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let job = parse_manifest_line(trimmed, base_dir).map_err(|message| {
            invalid_input(format!("{}:{}: {message}", path.display(), line_index + 1))
        })?;
        jobs.push(job);
    }

    Ok(jobs)
}

pub fn process_texture(job: &TextureJob, out_dir: &Path) -> Result<(), Box<dyn Error>> {
    job.validate().map_err(invalid_input)?;

    let img = image::open(&job.src)?;
    let cropped = aspect_crop(img, job.width, job.height).to_rgba8();
    let mut diffuse = imageops::resize(
        &cropped,
        job.width,
        job.height,
        imageops::FilterType::CatmullRom,
    );
    flatten_low_frequency(&mut diffuse);
    if job.tileable {
        make_tileable(&mut diffuse);
    }
    quantize_chunks(
        &mut diffuse,
        job.resolved_quantize_levels().map_err(invalid_input)?,
    );

    save_image(&diffuse, out_dir, &job.stem, "")?;
    save_image(
        &specular(&diffuse, SpecConfig::from_job(job)),
        out_dir,
        &job.stem,
        "_s",
    )?;
    save_image(
        &normal(&diffuse, job.normal_strength),
        out_dir,
        &job.stem,
        "_n",
    )?;
    save_image(
        &height_map(
            &diffuse,
            HeightConfig::from_job(job).map_err(invalid_input)?,
        ),
        out_dir,
        &job.stem,
        "_h",
    )?;

    Ok(())
}

fn parse_manifest_line(line: &str, base_dir: &Path) -> Result<TextureJob, String> {
    let fields: Vec<&str> = line.split('|').map(str::trim).collect();
    if !(7..=13).contains(&fields.len()) {
        return Err(format!(
            "expected 7 to 13 pipe-separated fields, found {}",
            fields.len()
        ));
    }

    let src = parse_manifest_path(fields[0], base_dir)?;
    let stem = parse_required_string(fields[1], "stem")?;
    let size = parse_optional_size(fields[2])?.unwrap_or(DEFAULT_TEXTURE_SIZE);
    let tileable = parse_bool(fields[3])?;
    let spec_scale = parse_f32(fields[4], "spec_scale")?;
    let normal_strength = parse_f32(fields[5], "normal_strength")?;
    let quantize_levels = parse_optional_quantize_levels(fields[6])?;
    let spec_profile = parse_optional_spec_profile(optional_field(&fields, 7))?;
    let spec_base = parse_optional_f32(optional_field(&fields, 8), "spec_base")?;
    let spec_gamma = parse_optional_f32(optional_field(&fields, 9), "spec_gamma")?;
    let spec_edge_damping = parse_optional_f32(optional_field(&fields, 10), "spec_edge_damping")?;
    let height_strength = parse_optional_f32(optional_field(&fields, 11), "height_strength")?;
    let height_quantize_levels =
        parse_optional_quantize_levels(optional_field(&fields, 12).unwrap_or(""))?;

    let job = TextureJob {
        src,
        stem,
        width: size.width,
        height: size.height,
        tileable,
        spec_scale,
        spec_profile,
        spec_base,
        spec_gamma,
        spec_edge_damping,
        normal_strength,
        quantize_levels,
        height_strength,
        height_quantize_levels,
    };
    job.validate()?;
    Ok(job)
}

fn parse_manifest_path(value: &str, base_dir: &Path) -> Result<PathBuf, String> {
    let src = parse_required_string(value, "src")?;
    let path = PathBuf::from(src);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(base_dir.join(path))
    }
}

fn parse_required_string(value: &str, name: &str) -> Result<String, String> {
    if value.is_empty() {
        Err(format!("{name} must not be blank"))
    } else {
        Ok(value.to_string())
    }
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("tileable must be true or false, got {value:?}")),
    }
}

fn parse_dimension(value: &str, name: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .map_err(|_| format!("{name} must be an unsigned integer, got {value:?}"))
        .and_then(|dimension| {
            if dimension > 0 {
                Ok(dimension)
            } else {
                Err(format!("{name} must be greater than 0"))
            }
        })
}

fn parse_f32(value: &str, name: &str) -> Result<f32, String> {
    let parsed = value
        .parse::<f32>()
        .map_err(|_| format!("{name} must be a number, got {value:?}"))?;
    validate_non_negative_finite(name, parsed)?;
    Ok(parsed)
}

fn parse_optional_f32(value: Option<&str>, name: &str) -> Result<Option<f32>, String> {
    value
        .filter(|value| !value.is_empty())
        .map(|value| parse_f32(value, name))
        .transpose()
}

fn parse_optional_spec_profile(value: Option<&str>) -> Result<SpecProfile, String> {
    value
        .filter(|value| !value.is_empty())
        .map(SpecProfile::from_str)
        .transpose()
        .map(|profile| profile.unwrap_or(SpecProfile::Luminance))
}

fn parse_optional_quantize_levels(value: &str) -> Result<Option<u8>, String> {
    if value.is_empty() {
        return Ok(None);
    }

    let levels = value
        .parse::<u8>()
        .map_err(|_| format!("quantize_levels must be blank, 0, or a u8, got {value:?}"))?;
    if levels == 0 {
        Ok(None)
    } else {
        Ok(Some(levels))
    }
}

fn optional_field<'a>(fields: &'a [&str], index: usize) -> Option<&'a str> {
    fields.get(index).copied()
}

fn validate_non_negative_finite(name: &str, value: f32) -> Result<(), String> {
    if !value.is_finite() || value < 0.0 {
        return Err(format!("{name} must be a finite non-negative number"));
    }
    Ok(())
}

fn validate_optional_unit(name: &str, value: Option<f32>) -> Result<(), String> {
    if let Some(value) = value {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(format!("{name} must be a finite number from 0 to 1"));
        }
    }
    Ok(())
}

fn validate_optional_positive_finite(name: &str, value: Option<f32>) -> Result<(), String> {
    if let Some(value) = value {
        if !value.is_finite() || value <= 0.0 {
            return Err(format!("{name} must be a finite number greater than 0"));
        }
    }
    Ok(())
}

fn validate_optional_non_negative_finite(name: &str, value: Option<f32>) -> Result<(), String> {
    if let Some(value) = value {
        if !value.is_finite() || value < 0.0 {
            return Err(format!("{name} must be a finite non-negative number"));
        }
    }
    Ok(())
}

fn save_image(
    image: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    out_dir: &Path,
    stem: &str,
    suffix: &str,
) -> Result<(), Box<dyn Error>> {
    let path = out_dir.join(format!("{stem}{suffix}.png"));
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    image.save(path)?;
    Ok(())
}

fn luminance(p: Rgba<u8>) -> f32 {
    (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0
}

/// Sample diffuse luminance at `(x, y)`, wrapping at the edges so tiling
/// textures stay seamless. Shared by the normal map (a Sobel derivative of
/// this same field), the specular edge-damping term, and the height map
/// (this field one derivative step earlier, i.e. undifferentiated).
fn wrapped_luma(diffuse: &ImageBuffer<Rgba<u8>, Vec<u8>>, x: i32, y: i32) -> f32 {
    let (w, h) = diffuse.dimensions();
    let px = x.rem_euclid(w as i32) as u32;
    let py = y.rem_euclid(h as i32) as u32;
    luminance(*diffuse.get_pixel(px, py))
}

fn clamp_u8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

fn nearest_palette(v: u8, levels: u8) -> u8 {
    let max = (levels - 1) as f32;
    clamp_u8(((v as f32 / 255.0) * max).round() / max * 255.0)
}

fn make_tileable(img: &mut ImageBuffer<Rgba<u8>, Vec<u8>>) {
    let (w, h) = img.dimensions();
    let band = (w.min(h) / 8).max(2);
    let original = img.clone();
    for y in 0..h {
        for x in 0..w {
            let mut px = original.get_pixel(x, y).0.map(|c| c as f32);
            if x < band {
                let other = original.get_pixel(w - band + x, y).0;
                let t = (band - x) as f32 / band as f32 * 0.5;
                for c in 0..3 {
                    px[c] = px[c] * (1.0 - t) + other[c] as f32 * t;
                }
            } else if x >= w - band {
                let other = original.get_pixel(x - (w - band), y).0;
                let t = (x - (w - band) + 1) as f32 / band as f32 * 0.5;
                for c in 0..3 {
                    px[c] = px[c] * (1.0 - t) + other[c] as f32 * t;
                }
            }
            if y < band {
                let other = original.get_pixel(x, h - band + y).0;
                let t = (band - y) as f32 / band as f32 * 0.5;
                for c in 0..3 {
                    px[c] = px[c] * (1.0 - t) + other[c] as f32 * t;
                }
            } else if y >= h - band {
                let other = original.get_pixel(x, y - (h - band)).0;
                let t = (y - (h - band) + 1) as f32 / band as f32 * 0.5;
                for c in 0..3 {
                    px[c] = px[c] * (1.0 - t) + other[c] as f32 * t;
                }
            }
            img.put_pixel(
                x,
                y,
                Rgba([clamp_u8(px[0]), clamp_u8(px[1]), clamp_u8(px[2]), 255]),
            );
        }
    }
}

fn flatten_low_frequency(img: &mut ImageBuffer<Rgba<u8>, Vec<u8>>) {
    let (w, h) = img.dimensions();
    let mut total = [0.0f32; 3];
    for p in img.pixels() {
        for c in 0..3 {
            total[c] += p[c] as f32;
        }
    }
    let count = (w * h) as f32;
    let avg = [total[0] / count, total[1] / count, total[2] / count];
    let original = img.clone();
    let radius = (w.min(h) / 8).max(3) as i32;
    for y in 0..h {
        for x in 0..w {
            let mut local = [0.0f32; 3];
            let mut n = 0.0;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let sx = ((x as i32 + dx).rem_euclid(w as i32)) as u32;
                    let sy = ((y as i32 + dy).rem_euclid(h as i32)) as u32;
                    let p = original.get_pixel(sx, sy);
                    for c in 0..3 {
                        local[c] += p[c] as f32;
                    }
                    n += 1.0;
                }
            }
            let p = original.get_pixel(x, y);
            let mut out = [0u8; 4];
            for c in 0..3 {
                let low = local[c] / n;
                out[c] = clamp_u8(p[c] as f32 - (low - avg[c]) * LOW_FREQUENCY_FLATTEN_STRENGTH);
            }
            out[3] = 255;
            img.put_pixel(x, y, Rgba(out));
        }
    }
}

fn quantize_chunks(img: &mut ImageBuffer<Rgba<u8>, Vec<u8>>, levels: u8) {
    for p in img.pixels_mut() {
        p[0] = nearest_palette(p[0], levels);
        p[1] = nearest_palette(p[1], levels);
        p[2] = nearest_palette(p[2], levels);
        p[3] = 255;
    }
}

fn specular(
    diffuse: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    config: SpecConfig,
) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
    let (w, h) = diffuse.dimensions();
    ImageBuffer::from_fn(w, h, |x, y| {
        let p = diffuse.get_pixel(x, y);
        let l = luminance(*p);
        let spec = specular_value(diffuse, x, y, l, config);
        let v = clamp_u8(spec * 255.0);
        Rgba([v, v, v, 255])
    })
}

fn specular_value(
    diffuse: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    x: u32,
    y: u32,
    luma: f32,
    config: SpecConfig,
) -> f32 {
    if config.profile == SpecProfile::Luminance && !config.has_new_overrides {
        return (luma.powf(1.7) * config.scale).min(96.0 / 255.0);
    }

    let coupled_luma = luma * config.luma_weight + (1.0 - config.luma_weight);
    let mut spec = config.base + coupled_luma.powf(config.gamma) * config.scale * config.response;
    let damping = edge_damping_factor(diffuse, x, y, luma, config.edge_damping);
    spec *= damping;
    spec.clamp(0.0, config.max_value)
}

fn edge_damping_factor(
    diffuse: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    x: u32,
    y: u32,
    luma: f32,
    edge_damping: f32,
) -> f32 {
    if edge_damping <= 0.0 {
        return 1.0;
    }

    let sx = |xx: i32, yy: i32| -> f32 { wrapped_luma(diffuse, xx, yy) };
    let xi = x as i32;
    let yi = y as i32;
    let gx = (sx(xi + 1, yi) - sx(xi - 1, yi)).abs();
    let gy = (sx(xi, yi + 1) - sx(xi, yi - 1)).abs();
    let edge = ((gx + gy) * 1.5).clamp(0.0, 1.0);
    let dark = (1.0 - smoothstep(0.08, 0.35, luma)) * 0.75;
    1.0 - edge_damping * edge.max(dark).clamp(0.0, 1.0)
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn normal(
    diffuse: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    strength: f32,
) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
    let (w, h) = diffuse.dimensions();
    ImageBuffer::from_fn(w, h, |x, y| {
        let sx = |xx: i32, yy: i32| -> f32 { wrapped_luma(diffuse, xx, yy) };
        let xi = x as i32;
        let yi = y as i32;
        let gx = (-sx(xi - 1, yi - 1) + sx(xi + 1, yi - 1) - 2.0 * sx(xi - 1, yi)
            + 2.0 * sx(xi + 1, yi)
            - sx(xi - 1, yi + 1)
            + sx(xi + 1, yi + 1))
            * strength;
        let gy = (-sx(xi - 1, yi - 1) - 2.0 * sx(xi, yi - 1) - sx(xi + 1, yi - 1)
            + sx(xi - 1, yi + 1)
            + 2.0 * sx(xi, yi + 1)
            + sx(xi + 1, yi + 1))
            * strength;
        let mut nx = -gx;
        let mut ny = -gy;
        let mut nz = 1.0f32;
        let len = (nx * nx + ny * ny + nz * nz).sqrt().max(0.0001);
        nx /= len;
        ny /= len;
        nz /= len;
        Rgba([
            clamp_u8((nx * 0.5 + 0.5) * 255.0),
            clamp_u8((ny * 0.5 + 0.5) * 255.0),
            clamp_u8((nz * 0.5 + 0.5) * 255.0),
            255,
        ])
    })
}

/// Derive a conventional height map from diffuse luminance: white = raised,
/// black = recessed. This is authoring-facing and intentionally NOT inverted
/// to depth here — see `tools/texture-tool/README.md`; `prl-build` performs
/// `depth = 255 - height` at bake time so authors keep thinking in familiar
/// height-map terms.
///
/// Height is the same luminance signal `normal()` differentiates (via Sobel)
/// one derivative step earlier, so it shares `wrapped_luma` rather than
/// re-deriving luminance sampling.
fn height_map(
    diffuse: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    config: HeightConfig,
) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
    let (w, h) = diffuse.dimensions();

    // Pivot contrast around the mean luminance rather than 0.5 so a dark or
    // bright material still gets meaningful relief instead of being clipped
    // against one rail.
    let mut sum = 0.0f32;
    for p in diffuse.pixels() {
        sum += luminance(*p);
    }
    let mean = sum / (w * h).max(1) as f32;

    let mut img = ImageBuffer::from_fn(w, h, |x, y| {
        let l = wrapped_luma(diffuse, x as i32, y as i32);
        let contrasted = (mean + (l - mean) * config.strength).clamp(0.0, 1.0);
        let v = clamp_u8(contrasted * 255.0);
        Rgba([v, v, v, 255])
    });

    // Reuse the exact posterization used for the diffuse map: the engine's
    // aesthetic dial is terraced depth, so plateaus in the height map should
    // read the same way plateaus in the diffuse map do.
    quantize_chunks(&mut img, config.quantize_levels);
    img
}

fn aspect_crop(
    img: image::DynamicImage,
    target_width: u32,
    target_height: u32,
) -> image::DynamicImage {
    let (w, h) = img.dimensions();
    let source_wide = (w as u64) * (target_height as u64) > (h as u64) * (target_width as u64);
    if source_wide {
        let crop_w = ((h as u64) * (target_width as u64) / (target_height as u64)) as u32;
        let x = (w - crop_w) / 2;
        img.crop_imm(x, 0, crop_w, h)
    } else {
        let crop_h = ((w as u64) * (target_height as u64) / (target_width as u64)) as u32;
        let y = (h - crop_h) / 2;
        img.crop_imm(0, y, w, crop_h)
    }
}

fn invalid_input(message: impl Into<String>) -> Box<dyn Error> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_line_resolves_relative_src_against_manifest_directory() {
        let job = parse_manifest_line(
            "source.png|metal_panel|64|true|0.2|0.5|",
            Path::new("/tmp/manifest-dir"),
        )
        .expect("manifest line should parse");

        assert_eq!(job.src, PathBuf::from("/tmp/manifest-dir/source.png"));
        assert_eq!(job.stem, "metal_panel");
        assert_eq!(job.width, 64);
        assert_eq!(job.height, 64);
        assert!(job.tileable);
        assert_eq!(job.spec_profile, SpecProfile::Luminance);
        assert_eq!(job.spec_base, None);
        assert_eq!(job.spec_gamma, None);
        assert_eq!(job.spec_edge_damping, None);
        assert_eq!(job.quantize_levels, None);
        assert_eq!(job.resolved_quantize_levels().unwrap(), 18);
    }

    #[test]
    fn manifest_line_treats_zero_quantize_as_default() {
        let job = parse_manifest_line(
            "/tmp/source.png|concrete_panel|128|false|0.1|0.4|0",
            Path::new("/tmp"),
        )
        .expect("manifest line should parse");

        assert_eq!(job.quantize_levels, None);
        assert_eq!(job.resolved_quantize_levels().unwrap(), 24);
    }

    #[test]
    fn manifest_line_uses_default_size_when_size_is_blank() {
        let job = parse_manifest_line(
            "/tmp/source.png|default_panel||false|0.1|0.4|",
            Path::new("/tmp"),
        )
        .expect("manifest line should parse");

        assert_eq!(job.width, 128);
        assert_eq!(job.height, 128);
        assert_eq!(job.resolved_quantize_levels().unwrap(), 24);
    }

    #[test]
    fn manifest_line_parses_extended_spec_controls() {
        let job = parse_manifest_line(
            "/tmp/source.png|glass_panel|128|false|0.35|0.4|24|glass|0.6|1.2|0.25",
            Path::new("/tmp"),
        )
        .expect("manifest line should parse");

        assert_eq!(job.spec_profile, SpecProfile::Glass);
        assert_eq!(job.spec_base, Some(0.6));
        assert_eq!(job.spec_gamma, Some(1.2));
        assert_eq!(job.spec_edge_damping, Some(0.25));
    }

    #[test]
    fn manifest_line_treats_blank_extended_spec_controls_as_defaults() {
        let job = parse_manifest_line(
            "/tmp/source.png|default_panel|128|false|0.1|0.4||||",
            Path::new("/tmp"),
        )
        .expect("manifest line should parse");

        assert_eq!(job.spec_profile, SpecProfile::Luminance);
        assert_eq!(job.spec_base, None);
        assert_eq!(job.spec_gamma, None);
        assert_eq!(job.spec_edge_damping, None);
    }

    #[test]
    fn manifest_line_parses_trailing_height_controls() {
        let job = parse_manifest_line(
            "/tmp/source.png|cobble_floor_01|128|true|0.24|0.5|24|polished-stone||||1.6|6",
            Path::new("/tmp"),
        )
        .expect("manifest line should parse");

        assert_eq!(job.spec_profile, SpecProfile::PolishedStone);
        assert_eq!(job.height_strength, Some(1.6));
        assert_eq!(job.height_quantize_levels, Some(6));
    }

    #[test]
    fn manifest_line_treats_blank_trailing_height_controls_as_defaults() {
        let job = parse_manifest_line(
            "/tmp/source.png|default_panel|128|false|0.1|0.4||||||",
            Path::new("/tmp"),
        )
        .expect("manifest line should parse");

        assert_eq!(job.height_strength, None);
        assert_eq!(job.height_quantize_levels, None);
        assert_eq!(job.resolved_height_quantize_levels().unwrap(), 24);
    }

    #[test]
    fn manifest_line_omitting_height_controls_still_parses() {
        let job = parse_manifest_line(
            "/tmp/source.png|legacy_panel|128|false|0.1|0.4|24|glass|||",
            Path::new("/tmp"),
        )
        .expect("manifest line should parse");

        assert_eq!(job.height_strength, None);
        assert_eq!(job.height_quantize_levels, None);
    }

    #[test]
    fn spec_profile_parses_named_profiles() {
        assert_eq!(
            "luminance".parse::<SpecProfile>().unwrap(),
            SpecProfile::Luminance
        );
        assert_eq!("matte".parse::<SpecProfile>().unwrap(), SpecProfile::Matte);
        assert_eq!(
            "concrete".parse::<SpecProfile>().unwrap(),
            SpecProfile::Concrete
        );
        assert_eq!(
            "polished-stone".parse::<SpecProfile>().unwrap(),
            SpecProfile::PolishedStone
        );
        assert_eq!(
            "painted-metal".parse::<SpecProfile>().unwrap(),
            SpecProfile::PaintedMetal
        );
        assert_eq!("glass".parse::<SpecProfile>().unwrap(), SpecProfile::Glass);
        assert_eq!(
            "screen".parse::<SpecProfile>().unwrap(),
            SpecProfile::Screen
        );
        assert_eq!("water".parse::<SpecProfile>().unwrap(), SpecProfile::Water);
        assert!("unknown".parse::<SpecProfile>().is_err());
    }

    #[test]
    fn specular_edge_damping_lowers_dark_edge_pixels() {
        let diffuse = ImageBuffer::from_fn(5, 5, |x, y| {
            if x == 2 && y == 2 {
                Rgba([8, 8, 8, 255])
            } else {
                Rgba([220, 220, 220, 255])
            }
        });
        let config = SpecConfig {
            profile: SpecProfile::PolishedStone,
            scale: DEFAULT_SPEC_SCALE,
            base: 0.12,
            gamma: 1.6,
            edge_damping: 0.65,
            response: 0.75,
            luma_weight: 0.85,
            max_value: 0.65,
            has_new_overrides: false,
        };

        let spec = specular(&diffuse, config);
        let smooth = spec.get_pixel(0, 0)[0];
        let dark_edge = spec.get_pixel(2, 2)[0];

        assert!(dark_edge < smooth);
    }

    #[test]
    fn parse_size_accepts_square_legacy_size() {
        assert_eq!(
            parse_size("64").unwrap(),
            TextureDimensions {
                width: 64,
                height: 64
            }
        );
    }

    #[test]
    fn parse_size_accepts_wide_dimensions() {
        assert_eq!(
            parse_size("128x64").unwrap(),
            TextureDimensions {
                width: 128,
                height: 64
            }
        );
    }

    #[test]
    fn parse_size_accepts_tall_dimensions() {
        assert_eq!(
            parse_size("64x128").unwrap(),
            TextureDimensions {
                width: 64,
                height: 128
            }
        );
    }

    #[test]
    fn parse_size_rejects_zero_dimensions() {
        assert!(parse_size("0").is_err());
        assert!(parse_size("128x0").is_err());
        assert!(parse_size("0x128").is_err());
    }

    #[test]
    fn parse_size_rejects_malformed_strings() {
        assert!(parse_size("128x").is_err());
        assert!(parse_size("x128").is_err());
        assert!(parse_size("128x64x32").is_err());
        assert!(parse_size("big").is_err());
    }

    fn base_job(src: PathBuf, stem: &str) -> TextureJob {
        TextureJob {
            src,
            stem: stem.to_string(),
            width: 32,
            height: 32,
            tileable: false,
            spec_scale: DEFAULT_SPEC_SCALE,
            spec_profile: SpecProfile::Luminance,
            spec_base: None,
            spec_gamma: None,
            spec_edge_damping: None,
            normal_strength: DEFAULT_NORMAL_STRENGTH,
            quantize_levels: Some(24),
            height_strength: None,
            height_quantize_levels: Some(8),
        }
    }

    fn height_test_temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("imgproc-height-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_test_diffuse_source(path: &Path, w: u32, h: u32) {
        // Deterministic pattern with varying luminance so height derivation
        // has real signal to work with.
        let img = ImageBuffer::from_fn(w, h, |x, y| {
            let v = ((x * 37 + y * 91) % 256) as u8;
            Rgba([v, v, v, 255])
        });
        img.save(path).unwrap();
    }

    /// Reads back a PNG's color-space metadata the same way
    /// `crates/level-compiler/src/texture_validation.rs` does, so this test
    /// exercises the exact guarantee `prl-build` will enforce.
    fn png_color_chunks(path: &Path) -> (bool, bool, bool) {
        let file = fs::File::open(path).unwrap();
        let decoder = png::Decoder::new(io::BufReader::new(file));
        let reader = decoder.read_info().unwrap();
        let info = reader.info();
        let has_srgb = info.srgb.is_some();
        let has_iccp = info.icc_profile.is_some();
        let has_non_linear_gama = info
            .gama_chunk
            .map(|gama| {
                let gamma: f32 = gama.into_value();
                (gamma - 1.0).abs() > 0.01
            })
            .unwrap_or(false);
        (has_srgb, has_non_linear_gama, has_iccp)
    }

    #[test]
    fn height_map_output_has_no_color_management_chunks() {
        let dir = height_test_temp_dir("linear");
        let src = dir.join("source.png");
        write_test_diffuse_source(&src, 32, 32);
        let out_dir = dir.join("out");
        let job = base_job(src, "stone_test");
        process_texture(&job, &out_dir).expect("process_texture should succeed");

        let h_path = out_dir.join("stone_test_h.png");
        let (has_srgb, has_non_linear_gama, has_iccp) = png_color_chunks(&h_path);
        assert!(!has_srgb, "_h.png must not carry an sRGB chunk");
        assert!(
            !has_non_linear_gama,
            "_h.png must not carry a non-linear gAMA chunk"
        );
        assert!(!has_iccp, "_h.png must not carry an iCCP chunk");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn height_map_dimensions_match_diffuse() {
        let dir = height_test_temp_dir("dims");
        let src = dir.join("source.png");
        write_test_diffuse_source(&src, 40, 24);
        let out_dir = dir.join("out");
        let mut job = base_job(src, "dims_test");
        job.width = 40;
        job.height = 24;
        process_texture(&job, &out_dir).expect("process_texture should succeed");

        let diffuse = image::open(out_dir.join("dims_test.png")).unwrap();
        let height = image::open(out_dir.join("dims_test_h.png")).unwrap();
        assert_eq!(diffuse.dimensions(), height.dimensions());
        assert_eq!(height.dimensions(), (40, 24));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn height_quantization_reduces_distinct_values() {
        let dir = height_test_temp_dir("quant");
        let src = dir.join("source.png");
        write_test_diffuse_source(&src, 48, 48);
        let out_dir = dir.join("out");

        let mut low_job = base_job(src.clone(), "quant_low");
        low_job.width = 48;
        low_job.height = 48;
        low_job.height_quantize_levels = Some(2);
        process_texture(&low_job, &out_dir).expect("process_texture should succeed");

        let mut high_job = base_job(src, "quant_high");
        high_job.width = 48;
        high_job.height = 48;
        high_job.height_quantize_levels = Some(24);
        process_texture(&high_job, &out_dir).expect("process_texture should succeed");

        let low = image::open(out_dir.join("quant_low_h.png"))
            .unwrap()
            .to_rgba8();
        let high = image::open(out_dir.join("quant_high_h.png"))
            .unwrap()
            .to_rgba8();

        fn distinct_values(img: &ImageBuffer<Rgba<u8>, Vec<u8>>) -> usize {
            let mut values: Vec<u8> = img.pixels().map(|p| p[0]).collect();
            values.sort_unstable();
            values.dedup();
            values.len()
        }

        let low_distinct = distinct_values(&low);
        let high_distinct = distinct_values(&high);
        assert!(
            low_distinct <= 2,
            "2-level quantization should produce at most 2 plateaus, got {low_distinct}"
        );
        assert!(
            low_distinct < high_distinct,
            "low quantize levels ({low_distinct}) should produce fewer distinct height \
             values than high quantize levels ({high_distinct})"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn height_strength_differs_by_profile() {
        // polished-stone (pronounced relief) should produce a wider spread of
        // height values than painted-metal (almost none) for the same source.
        let diffuse = ImageBuffer::from_fn(16, 16, |x, y| {
            let v = ((x * 53 + y * 17) % 256) as u8;
            Rgba([v, v, v, 255])
        });

        let stone_config = HeightConfig {
            strength: SpecProfile::PolishedStone.height_defaults().strength,
            quantize_levels: 24,
        };
        let metal_config = HeightConfig {
            strength: SpecProfile::PaintedMetal.height_defaults().strength,
            quantize_levels: 24,
        };

        let stone = height_map(&diffuse, stone_config);
        let metal = height_map(&diffuse, metal_config);

        fn spread(img: &ImageBuffer<Rgba<u8>, Vec<u8>>) -> i32 {
            let min = img.pixels().map(|p| p[0]).min().unwrap() as i32;
            let max = img.pixels().map(|p| p[0]).max().unwrap() as i32;
            max - min
        }

        let stone_spread = spread(&stone);
        let metal_spread = spread(&metal);
        assert!(
            stone_spread > metal_spread,
            "polished-stone height strength should read more pronounced than \
             painted-metal: stone={stone_spread} metal={metal_spread}"
        );
    }

    #[test]
    fn height_strength_override_takes_precedence_over_profile_default() {
        let mut job = base_job(PathBuf::from("unused.png"), "override_test");
        job.spec_profile = SpecProfile::PolishedStone;
        job.height_strength = Some(0.05);

        let config = HeightConfig::from_job(&job).unwrap();
        assert_eq!(config.strength, 0.05);
    }

    #[test]
    fn resolved_height_quantize_levels_uses_size_based_default_when_unset() {
        let mut job = base_job(PathBuf::from("unused.png"), "default_levels_test");
        job.height_quantize_levels = None;
        job.width = 64;
        job.height = 64;
        assert_eq!(job.resolved_height_quantize_levels().unwrap(), 18);

        job.width = 128;
        job.height = 128;
        assert_eq!(job.resolved_height_quantize_levels().unwrap(), 24);
    }

    #[test]
    fn validate_rejects_height_quantize_levels_of_one() {
        let mut job = base_job(PathBuf::from("unused.png"), "bad_levels_test");
        job.height_quantize_levels = Some(1);
        assert!(job.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_finite_height_strength() {
        let mut job = base_job(PathBuf::from("unused.png"), "bad_strength_test");
        job.height_strength = Some(f32::NAN);
        assert!(job.validate().is_err());
    }
}
