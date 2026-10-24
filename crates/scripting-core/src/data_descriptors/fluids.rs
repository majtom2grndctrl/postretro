// `fluids` manifest drain: each VM converts one entry at a time to JSON and
// both parse it here, so the two runtimes judge each entry with the same
// diagnostics. Which keys count as entries differs at the container (see
// `declaration_list`). Range policy lives on `FluidRegistry`.
// See: context/lib/scripting.md §2 (Mod-init context lifecycle)

use super::declaration_list::{
    ConvertedEntry, DeclarationList, EntryPath, convert_declaration_list_js,
    convert_declaration_list_lua, shape,
};
use super::*;
use postretro_foundation::data_descriptors::{FluidDescriptor, FluidRegistry};

/// Keys a fluid entry may carry. Anything else rejects the entry, so a
/// misspelled optional sound is loud.
const FLUID_KEYS: [&str; 13] = [
    "name",
    "viscosity",
    "viewTint",
    "muffle",
    "surfaceOpacity",
    "surfaceReflectivity",
    "enterSound",
    "exitSound",
    "fogDistance",
    "fogColor",
    "wavePeriod",
    "waveAmplitude",
    "waveLength",
];

/// An optional range: one number `L`, meaning `[L, L]`, or a `[min, max]`
/// pair. Order is judged by validation, not here.
fn optional_number_or_pair(
    path: &EntryPath,
    entry: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<Option<[f32; 2]>, DescriptorError> {
    match entry.get(field) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Array(pair)) if pair.len() == 2 => Ok(Some([
            path.number(&pair[0], field)?,
            path.number(&pair[1], field)?,
        ])),
        Some(value @ serde_json::Value::Number(_)) => {
            let length = path.number(value, field)?;
            Ok(Some([length, length]))
        }
        Some(_) => Err(shape(format!(
            "`{}` must be a number or an array of 2 numbers [min, max]",
            path.field(field)
        ))),
    }
}

/// Parse one converted entry's shape. Ranges are checked by the registry.
/// Diagnostics name the field as `fluids[index].<field>`; `index` counts
/// entries from 0 in both runtimes. A hole (`null`, `undefined` or `nil`) is a
/// malformed entry like any other non-object. An omitted `surfaceReflectivity`,
/// `waveAmplitude` and `waveLength` resolve to their defaults here, so
/// validation judges the resolved values; an omitted `fogColor` stays `None`
/// and resolves against `viewTint` where it is read
/// (`FluidDescriptor::resolved_fog_color`).
fn fluid_from_json(
    index: usize,
    entry: serde_json::Value,
) -> Result<FluidDescriptor, DescriptorError> {
    let path = EntryPath::new(FLUIDS.field, index);
    let serde_json::Value::Object(entry) = entry else {
        return Err(shape(format!("`fluids[{index}]` must be a fluid object")));
    };
    if let Some(unknown) = entry.keys().find(|key| !FLUID_KEYS.contains(&key.as_str())) {
        return Err(shape(format!(
            "unknown fluid field `{}`",
            path.field(unknown)
        )));
    }
    let name = match path.required(&entry, "name")? {
        serde_json::Value::String(name) => name.clone(),
        _ => return Err(shape(format!("`{}` must be a string", path.field("name")))),
    };
    let tint = match path.required(&entry, "viewTint")? {
        serde_json::Value::Array(components) if components.len() == 4 => components,
        _ => {
            return Err(shape(format!(
                "`{}` must be an array of 4 numbers",
                path.field("viewTint")
            )));
        }
    };
    let mut view_tint = [0.0; 4];
    for (slot, component) in view_tint.iter_mut().zip(tint) {
        *slot = path.number(component, "viewTint")?;
    }
    let wave_amplitude = path
        .optional_number(&entry, "waveAmplitude")?
        .unwrap_or(FluidDescriptor::DEFAULT_WAVE_AMPLITUDE);
    let wave_length = optional_number_or_pair(&path, &entry, "waveLength")?
        .unwrap_or_else(|| FluidDescriptor::default_wave_length(wave_amplitude));
    Ok(FluidDescriptor {
        name,
        viscosity: path.number(path.required(&entry, "viscosity")?, "viscosity")?,
        view_tint,
        muffle: path.number(path.required(&entry, "muffle")?, "muffle")?,
        surface_opacity: path.number(path.required(&entry, "surfaceOpacity")?, "surfaceOpacity")?,
        surface_reflectivity: path
            .optional_number(&entry, "surfaceReflectivity")?
            .unwrap_or(FluidDescriptor::DEFAULT_SURFACE_REFLECTIVITY),
        enter_sound: path.optional_string(&entry, "enterSound")?,
        exit_sound: path.optional_string(&entry, "exitSound")?,
        fog_distance: path.optional_number(&entry, "fogDistance")?,
        fog_color: path.optional_rgb(&entry, "fogColor")?,
        wave_period: path.optional_number(&entry, "wavePeriod")?,
        wave_amplitude,
        wave_length,
    })
}

const FLUIDS: DeclarationList = DeclarationList {
    field: "fluids",
    entries: "fluids",
};

/// Drain `fluids` from a QuickJS manifest object. Entries convert one at a
/// time, so an unconvertible entry costs only itself; a non-array field, or
/// an array past the container bound, warns once and declares no fluids
/// (`convert_declaration_list_js`). Never rejects the manifest.
pub fn drain_fluids_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    scope: &str,
) -> Result<FluidRegistry, DescriptorError> {
    let converted = convert_declaration_list_js(ctx, obj, FLUIDS, scope)?;
    Ok(registry_from(scope, converted))
}

/// Drain `fluids` from a Luau manifest table: the twin of
/// [`drain_fluids_js`], including where the runtimes diverge on keys that are
/// not slots (`convert_declaration_list_lua`).
pub fn drain_fluids_lua(table: &Table, scope: &str) -> Result<FluidRegistry, DescriptorError> {
    let converted = convert_declaration_list_lua(table, FLUIDS, scope)?;
    Ok(registry_from(scope, converted))
}

fn registry_from(scope: &str, converted: Vec<ConvertedEntry>) -> FluidRegistry {
    FluidRegistry::from_declarations(
        scope,
        converted
            .into_iter()
            .enumerate()
            .map(|(index, entry)| entry.and_then(|entry| fluid_from_json(index, entry))),
    )
}
