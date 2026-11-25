// Colour mip source gates: the exponent -> mip WGSL pinned to its level-format
// reference, water's mip-0 read, the overlay's per-mip read,
// and no pass writing the probe arrays.
// See: context/lib/testing_guide.md · context/lib/rendering_pipeline.md §7.3a

use postretro_level_format::reflection_probes::REFLECTION_PROBE_MIN_BLUR_MIP;

use super::GLOSSY_OVERLAY_SHADER_SOURCE;
use crate::render::gpu_test_harness::{wgsl_const_line, wgsl_function};

const BLUR_WGSL: &str = include_str!("../shaders/reflection_probe_blur.wgsl");
const OVERLAY_WGSL: &str = include_str!("../shaders/glossy_overlay.wgsl");
const FLUID_REFLECTION_WGSL: &str = include_str!("../shaders/fluid_reflection.wgsl");

/// `src` with every `//` comment removed, then every whitespace character.
fn squashed(src: &str) -> String {
    src.lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<String>()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// The f32 a `const NAME: f32 = literal;` line declares.
fn wgsl_f32(source: &str, name: &str) -> f32 {
    let line = wgsl_const_line(source, name);
    line.split('=')
        .nth(1)
        .and_then(|value| value.trim().trim_end_matches(';').parse().ok())
        .unwrap_or_else(|| panic!("{name}: {line}"))
}

/// Every `.wgsl` under `shaders/`, by file name.
fn shaders() -> Vec<(String, String)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/shaders");
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .expect("shader directory")
        .map(|entry| entry.expect("shader entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "wgsl"))
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read_to_string(&path).expect("shader text"),
            )
        })
        .collect();
    out.sort();
    out
}

// The shader's exponent -> mip function is level-format's, constant
// for constant and operation for operation, and composed into the overlay.
// α₀ derives from the section's tile dimension as level-format's
// `reflection_probe_mip0_texel_angle` derives it, so no engine tile size is
// baked into the shader.
#[test]
fn the_shaders_exponent_mip_function_is_the_pinned_reference() {
    assert_eq!(
        wgsl_f32(BLUR_WGSL, "REFLECTION_PROBE_PI"),
        std::f32::consts::PI
    );
    // level-format `reflection_probe_mip0_texel_angle`: π / (tile / 2).
    assert_eq!(
        squashed(wgsl_function(
            BLUR_WGSL,
            "reflection_probe_mip0_texel_angle"
        )),
        "fnreflection_probe_mip0_texel_angle(tile_dimension:u32)->f32{\
         returnREFLECTION_PROBE_PI/f32(tile_dimension/2u);}"
    );
    assert_eq!(
        wgsl_f32(BLUR_WGSL, "REFLECTION_PROBE_MIN_BLUR_MIP"),
        REFLECTION_PROBE_MIN_BLUR_MIP
    );
    // level-format `reflection_lobe_half_width`: 2·acos(0.5^(1/n)).
    assert_eq!(
        squashed(wgsl_function(BLUR_WGSL, "reflection_lobe_half_width")),
        "fnreflection_lobe_half_width(exponent:f32)->f32{\
         return2.0*acos(pow(0.5,1.0/exponent));}"
    );
    // level-format `reflection_probe_exponent_mip`: the last mip wins over
    // the minimum blur (`max` then `min`).
    assert_eq!(
        squashed(wgsl_function(BLUR_WGSL, "reflection_probe_exponent_mip")),
        "fnreflection_probe_exponent_mip(exponent:f32,mip0_texel_angle:f32,\
         colour_mip_count:u32)->f32{\
         letlast=f32(colour_mip_count-1u);\
         letmip=log2(reflection_lobe_half_width(exponent)/mip0_texel_angle);\
         returnmin(max(mip,REFLECTION_PROBE_MIN_BLUR_MIP),last);}"
    );
    assert!(GLOSSY_OVERLAY_SHADER_SOURCE.contains(BLUR_WGSL));
    // One definition: no other shader redeclares the function or constants.
    for (name, source) in shaders() {
        if name == "reflection_probe_blur.wgsl" {
            continue;
        }
        for item in [
            "fn reflection_probe_exponent_mip(",
            "fn reflection_probe_mip0_texel_angle(",
            "fn reflection_lobe_half_width(",
            "const REFLECTION_PROBE_PI",
            "const REFLECTION_PROBE_MIN_BLUR_MIP",
        ] {
            assert!(!source.contains(item), "{name} redeclares {item}");
        }
    }
}

// Shader half: the overlay reads the two mips the function selects,
// each at its own tile's UV and level, blended as render-cpu
// `reflection_probe_mip_colour` does; the probe's layer and the colour mip
// count (the installed array's own) and the tile dimension (the probe
// table's, the section's own) are its only other inputs.
#[test]
fn the_overlay_reads_each_selected_mip_at_its_own_uv() {
    assert_eq!(
        squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_probe_colour")),
        "fnglossy_overlay_probe_colour(direction:vec3<f32>,layer:u32,exponent:f32)->vec3<f32>{\
         letcount=textureNumLevels(fluid_probe_array);\
         letmip=reflection_probe_exponent_mip(exponent,\
         reflection_probe_mip0_texel_angle(fluid_probes.tile.x),count);\
         letlower=u32(mip);\
         letupper=min(lower+1u,count-1u);\
         returnmix(glossy_overlay_probe_mip(direction,layer,lower),\
         glossy_overlay_probe_mip(direction,layer,upper),mip-f32(lower));}"
    );
    assert_eq!(
        squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_probe_mip")),
        "fnglossy_overlay_probe_mip(direction:vec3<f32>,layer:u32,mip:u32)->vec3<f32>{\
         letuv=fluid_reflection_tile_uv(direction,fluid_probes.tile.x>>mip,fluid_probes.tile.y);\
         returntextureSampleLevel(fluid_probe_array,sh_atlas_sampler,uv,layer,f32(mip)).rgb;}"
    );
    // The overlay's only colour-array reads are these.
    let code = squashed(OVERLAY_WGSL);
    assert_eq!(
        code.matches("textureSampleLevel(fluid_probe_array").count(),
        1
    );
    assert_eq!(code.matches("glossy_overlay_probe_colour(").count(), 2);
}

// Scan half: water samples colour mip 0 only. Every read of the
// colour array outside the overlay is at level 0.0, the fluid shader never
// composes the exponent -> mip function, and water's one colour tap is its
// sample's.
#[test]
fn water_samples_colour_mip_zero_only() {
    let water = squashed(wgsl_function(
        FLUID_REFLECTION_WGSL,
        "fluid_reflection_sample",
    ));
    assert!(water.contains(
        "letcapture=textureSampleLevel(fluid_probe_array,sh_atlas_sampler,uv,layer,0.0).rgb;"
    ));
    for (name, source) in shaders() {
        if name == "glossy_overlay.wgsl" {
            continue;
        }
        let code = squashed(&source);
        // The glossy floor layer's blur reads the probe term's blur width
        // at a texel's exponent, never the colour array (its pass binds no
        // probe array: `glossy_floor_march_tests`).
        assert!(
            !code.contains("reflection_probe_exponent_mip(")
                || name == "reflection_probe_blur.wgsl"
                || name == "glossy_floor_blur.wgsl",
            "{name} reaches the exponent -> mip function"
        );
        for (at, _) in code.match_indices("(fluid_probe_array,") {
            let call = &code[..at];
            assert!(
                call.ends_with("textureSampleLevel"),
                "{name}: the colour array is read by something other than an explicit level"
            );
            let args = &code[at..];
            let args = &args[..args.find(')').expect("call end") + 1];
            assert!(
                args.ends_with(",0.0)"),
                "{name}: colour array read at {args}"
            );
        }
    }
    let fluid = include_str!("fluid_surface.rs");
    assert!(!fluid.contains("reflection_probe_blur.wgsl"));
}

// Source half: every colour mip uploads once, at install, from id 54;
// the probe arrays are created in one place, sampled only, and never a
// render attachment, storage texture or copy destination after their upload.
#[test]
fn no_pass_writes_the_probe_arrays_after_install() {
    let resources = include_str!("reflection_probe_resources.rs");
    let code = squashed(resources);
    assert!(
        code.contains("usage:wgpu::TextureUsages::TEXTURE_BINDING|wgpu::TextureUsages::COPY_DST,"),
        "the arrays are sampled and filled once"
    );
    for forbidden in [
        "RENDER_ATTACHMENT",
        "STORAGE_BINDING",
        "write_texture",
        "copy_texture_to_texture",
        "copy_buffer_to_texture",
        "StorageTexture",
    ] {
        assert!(
            !code.contains(forbidden),
            "reflection_probe_resources.rs: {forbidden}"
        );
    }
    // Uploaded whole, every mip of every layer, from the section's tiles.
    assert!(code.contains("section.colour_mip_count,"));
    assert!(code.contains("wgpu::util::TextureDataOrder::LayerMajor,&section.tiles,"));
    // The arrays bind as sampled textures in every layout that reads them.
    for (file, source) in [
        (
            "fluid_reflection_probes.rs",
            include_str!("fluid_reflection_probes.rs"),
        ),
        ("glossy_overlay.rs", include_str!("glossy_overlay.rs")),
        (
            "glossy_overlay/level.rs",
            include_str!("glossy_overlay/level.rs"),
        ),
    ] {
        assert!(
            !source.contains("StorageTexture"),
            "{file} binds a storage texture"
        );
    }
    for (name, source) in shaders() {
        let code = squashed(&source);
        for array in ["fluid_probe_array", "fluid_probe_distance"] {
            if let Some(at) = code.find(&format!("var{array}:")) {
                assert!(
                    code[at..].starts_with(&format!("var{array}:texture_2d_array<f32>;")),
                    "{name} declares {array} as something other than a sampled array"
                );
            }
            assert!(
                !code.contains(&format!("textureStore({array}")),
                "{name} writes {array}"
            );
        }
    }
}
