// `ghostStyles` manifest drain: both runtimes convert each entry to JSON and
// parse it here, so they judge each entry alike. Range policy and the floor
// values live on `GhostStyleRegistry` / `GhostStyle::FLOOR` in foundation.
// See: context/lib/scripting.md §2 (Ghost style declarations)

use super::declaration_list::{
    ConvertedEntry, DeclarationList, EntryPath, convert_declaration_list_js,
    convert_declaration_list_lua, shape,
};
use super::*;
use postretro_foundation::data_descriptors::{
    GhostStyle, GhostStyleDescriptor, GhostStyleRegistry,
};

/// Keys a ghost style entry may carry. Anything else rejects the entry, so a
/// misspelled field is loud rather than silently the floor value.
const GHOST_STYLE_KEYS: [&str; 6] = [
    "name",
    "tint",
    "opacity",
    "saturation",
    "audio",
    "impactAudio",
];

const GHOST_STYLES: DeclarationList = DeclarationList {
    field: "ghostStyles",
    entries: "ghost styles",
};

/// Parse one converted entry's shape; ranges are checked by the registry.
/// `name` is required and explicit (maps reference it as a string). Every
/// other field is optional and an omitted (or `null`) one takes the floor
/// style's value, so the descriptor carries a whole resolved style.
/// Diagnostics name the field as `ghostStyles[index].<field>`.
fn ghost_style_from_json(
    index: usize,
    entry: serde_json::Value,
) -> Result<GhostStyleDescriptor, DescriptorError> {
    let path = EntryPath::new(GHOST_STYLES.field, index);
    let serde_json::Value::Object(entry) = entry else {
        return Err(shape(format!(
            "`ghostStyles[{index}]` must be a ghost style object"
        )));
    };
    if let Some(unknown) = entry
        .keys()
        .find(|key| !GHOST_STYLE_KEYS.contains(&key.as_str()))
    {
        return Err(shape(format!(
            "unknown ghost style field `{}`",
            path.field(unknown)
        )));
    }
    let name = match path.required(&entry, "name")? {
        serde_json::Value::String(name) => name.clone(),
        _ => return Err(shape(format!("`{}` must be a string", path.field("name")))),
    };
    let floor = GhostStyle::FLOOR;
    Ok(GhostStyleDescriptor {
        name,
        style: GhostStyle {
            tint: path.optional_rgb(&entry, "tint")?.unwrap_or(floor.tint),
            opacity: path
                .optional_number(&entry, "opacity")?
                .unwrap_or(floor.opacity),
            saturation: path
                .optional_number(&entry, "saturation")?
                .unwrap_or(floor.saturation),
            audio: path.optional_bool(&entry, "audio")?.unwrap_or(floor.audio),
            impact_audio: path
                .optional_bool(&entry, "impactAudio")?
                .unwrap_or(floor.impact_audio),
        },
    })
}

/// Drain `ghostStyles` from a QuickJS manifest object. A malformed entry or
/// a later duplicate name drops with a warning; a non-array field warns once
/// and declares none. Never rejects the manifest.
pub fn drain_ghost_styles_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    scope: &str,
) -> Result<GhostStyleRegistry, DescriptorError> {
    let converted = convert_declaration_list_js(ctx, obj, GHOST_STYLES, scope)?;
    Ok(registry_from(scope, converted))
}

/// Drain `ghostStyles` from a Luau manifest table: the twin of
/// [`drain_ghost_styles_js`].
pub fn drain_ghost_styles_lua(
    table: &Table,
    scope: &str,
) -> Result<GhostStyleRegistry, DescriptorError> {
    let converted = convert_declaration_list_lua(table, GHOST_STYLES, scope)?;
    Ok(registry_from(scope, converted))
}

fn registry_from(scope: &str, converted: Vec<ConvertedEntry>) -> GhostStyleRegistry {
    GhostStyleRegistry::from_declarations(
        scope,
        converted
            .into_iter()
            .enumerate()
            .map(|(index, entry)| entry.and_then(|entry| ghost_style_from_json(index, entry))),
    )
}
