// Shared PNG-backed room for coloured-bounce estimator and pipeline tests.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::path::Path;

#[derive(Clone, Copy)]
pub enum Appearance {
    RedWall,
    Unresolved,
    MissingSources,
}

/// The closed room is 256 × 256 × 128 map units. Its west wall is red;
/// the other walls, ceiling and floor are white. The small central pool
/// makes reflection captures part of the same fixture's real bake.
pub fn room_map(appearance: Appearance, static_light: bool, animated_light: bool) -> String {
    let (wall, floor, ceiling) = match appearance {
        Appearance::RedWall => ("bounce/red", "bounce/white", "bounce/white"),
        // Three distinct unresolved paths: absent source, normal-only and specular-only.
        Appearance::Unresolved => (
            "bounce/missing",
            "bounce/normal_only",
            "bounce/specular_only",
        ),
        Appearance::MissingSources => ("bounce/missing", "bounce/missing", "bounce/missing"),
    };
    let world = [
        box_brush([-144, -144, -16], [144, 144, 0], floor),
        box_brush([-144, -144, 128], [144, 144, 144], ceiling),
        box_brush([-144, -144, 0], [-128, 144, 128], wall),
        box_brush([128, -144, 0], [144, 144, 128], floor),
        box_brush([-128, -144, 0], [128, -128, 128], floor),
        box_brush([-128, 128, 0], [128, 144, 128], floor),
    ]
    .join("\n");
    let mut map = format!(
        "{{\n\"classname\" \"worldspawn\"\n{world}\n}}\n\
         {{\n\"classname\" \"player_spawn\"\n\"origin\" \"64 64 32\"\n}}\n"
    );
    // Both rigs use precisely the same hard white light. Tests can select
    // either alone for pre-encoding animated-versus-static bounce parity.
    for animated in [false, true] {
        if (animated && !animated_light) || (!animated && !static_light) {
            continue;
        }
        let style = if animated { "\"style\" \"1\"\n" } else { "" };
        map.push_str(&format!(
            "{{\n\"classname\" \"light\"\n\"origin\" \"-64 0 96\"\n\
             \"light\" \"1200\"\n\"_falloff_range\" \"512\"\n\
             \"_light_size\" \"0\"\n\"_color\" \"255 255 255\"\n{style}}}\n"
        ));
    }
    let water_texture = match appearance {
        Appearance::RedWall => "bounce/water",
        Appearance::Unresolved | Appearance::MissingSources => "bounce/missing",
    };
    let water = box_brush([-32, -32, 0], [32, 32, 16], water_texture);
    map.push_str(&format!(
        "{{\n\"classname\" \"fluid_volume\"\n\"fluid\" \"water\"\n{water}\n}}\n"
    ));
    map
}

/// Insert a worldspawn key-value pair ahead of the brushes. `_sh_coarsen 0`
/// keeps id 41 in uniform storage, independent of the owner-approved
/// adaptive boundary that coloured indirect brightness may move.
pub fn with_worldspawn_key(map: &str, key: &str, value: &str) -> String {
    map.replacen(
        "\"classname\" \"worldspawn\"",
        &format!("\"classname\" \"worldspawn\"\n\"{key}\" \"{value}\""),
        1,
    )
}

/// The room without its trailing fluid entity: no fluid-only tail names.
pub fn without_fluid(map: String) -> String {
    let end = map
        .rfind("{\n\"classname\" \"fluid_volume\"")
        .expect("shared fixture ends with fluid entity");
    map[..end].to_owned()
}

/// Generate tiny opaque source PNGs, then let the production material baker
/// create their full mip chains and content-addressed sidecars.
pub fn write_textures(content_root: &Path) {
    for (name, rgb) in [
        ("red", [255, 0, 0]),
        ("white", [255, 255, 255]),
        ("water", [32, 64, 96]),
        ("normal_only_n", [128, 128, 255]),
        ("specular_only_s", [128, 128, 128]),
    ] {
        write_texture(content_root, name, rgb);
    }
}

/// One solid 16 × 16 opaque `textures/bounce/<name>.png`.
pub fn write_texture(content_root: &Path, name: &str, rgb: [u8; 3]) {
    let path = content_root
        .join("textures/bounce")
        .join(format!("{name}.png"));
    std::fs::create_dir_all(path.parent().expect("texture parent")).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), 16, 16);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer
        .write_image_data(&[rgb[0], rgb[1], rgb[2], 255].repeat(256))
        .unwrap();
}

/// One axis-aligned brush with every side painted `texture`.
pub fn box_brush(min: [i32; 3], max: [i32; 3], texture: &str) -> String {
    box_brush_sides(min, max, [texture; 6])
}

/// One axis-aligned brush; `sides` paints −X, +X, −Y, +Y, −Z, +Z in map axes.
pub fn box_brush_sides(min: [i32; 3], max: [i32; 3], sides: [&str; 6]) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    let [west, east, south, north, bottom, top] = sides;
    format!(
        "{{\n\
         ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {west} 0 0 0 1 1\n\
         ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {east} 0 0 0 1 1\n\
         ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {south} 0 0 0 1 1\n\
         ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {north} 0 0 0 1 1\n\
         ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {bottom} 0 0 0 1 1\n\
         ( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {top} 0 0 0 1 1\n\
         }}"
    )
}

/// A ping-pong mover painted `texture`, rising 32 units in the room's
/// south-east corner. No static-world face uses its name.
pub fn mover_entity(texture: &str) -> String {
    let brush = box_brush([56, -88, 0], [88, -56, 16], texture);
    format!(
        "{{\n\"classname\" \"kinematic_mover\"\n\"name\" \"lift\"\n\"path\" \"lift_a\"\n\
         \"speed\" \"1\"\n\"move_mode\" \"ping_pong\"\n{brush}\n}}\n\
         {{\n\"classname\" \"kinematic_waypoint\"\n\"name\" \"lift_a\"\n\"next\" \"lift_b\"\n\
         \"origin\" \"72 -72 0\"\n}}\n\
         {{\n\"classname\" \"kinematic_waypoint\"\n\"name\" \"lift_b\"\n\"origin\" \"72 -72 32\"\n}}\n"
    )
}

/// The red-wall room with a static light, a `bounce/lift` mover, and a pool
/// whose top shares the world floor's `bounce/white` while its sides use the
/// fluid-only `bounce/water`: one name per material-stage/fluid-tail split case.
pub fn shared_name_room_map() -> String {
    let pool = box_brush_sides(
        [-32, -32, 0],
        [32, 32, 16],
        [
            "bounce/water",
            "bounce/water",
            "bounce/water",
            "bounce/water",
            "bounce/water",
            "bounce/white",
        ],
    );
    format!(
        "{}{}{{\n\"classname\" \"fluid_volume\"\n\"fluid\" \"water\"\n{pool}\n}}\n",
        without_fluid(room_map(Appearance::RedWall, true, false)),
        mover_entity("bounce/lift"),
    )
}
