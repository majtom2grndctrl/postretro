// Data-context descriptors: JS readers for the mesh key set and the
// `components.mesh.weaponMounts` table.
// See: context/lib/scripting.md

use super::super::*;
use postretro_entities::data_descriptors::{
    WeaponMountDescriptor, check_mesh_key, check_weapon_mount_key, weapon_mount_shape_error,
};
use rquickjs::object::Filter;

/// An object's own string keys, sorted so the first rejected key matches the
/// Luau reader, whose table order is unspecified.
fn sorted_own_keys(object: &Object<'_>) -> Result<Vec<String>, DescriptorError> {
    let mut keys = object
        .own_keys::<String>(Filter::new().string())
        .map(|key| key.map_err(js_err))
        .collect::<Result<Vec<_>, _>>()?;
    keys.sort();
    Ok(keys)
}

/// Reject any own key of `components.mesh` outside the mesh vocabulary.
pub(super) fn reject_unknown_mesh_keys(
    owner: &str,
    mesh: &Object<'_>,
) -> Result<(), DescriptorError> {
    for key in sorted_own_keys(mesh)? {
        check_mesh_key(owner, &key)?;
    }
    Ok(())
}

/// Read a plain-object `socket → model path` map at `path`. The error is the
/// bare reason; callers decide whether it names the character.
pub(super) fn string_map_from_js(
    raw: JsValue<'_>,
    path: &str,
) -> Result<HashMap<String, String>, String> {
    let not_a_map = || format!("`{path}` must be a plain object map");
    if raw.type_of() != rquickjs::Type::Object {
        return Err(not_a_map());
    }
    let map = Object::from_value(raw).map_err(|_| not_a_map())?;
    if let Some(prototype) = map.get_prototype() {
        let object_constructor: Object = map
            .ctx()
            .globals()
            .get("Object")
            .map_err(|error| error.to_string())?;
        let object_prototype: Object = object_constructor
            .get("prototype")
            .map_err(|error| error.to_string())?;
        if prototype.as_value() != object_prototype.as_value() {
            return Err(not_a_map());
        }
    }
    let mut out = HashMap::new();
    for entry in map.props::<String, JsValue>() {
        let (socket, value) = entry.map_err(|error| error.to_string())?;
        let model = value
            .as_string()
            .ok_or_else(|| format!("`{path}.{socket}` must be a string"))?
            .to_string()
            .map_err(|error| error.to_string())?;
        out.insert(socket, model);
    }
    Ok(out)
}

/// Read `components.mesh.weaponMounts`: an array of `{ weapon, attachments }`
/// entries whose `weapon` the SDK has lowered to a canonical name. Shape
/// errors name the character; the shared rules run in `MeshDescriptor::build`.
pub(super) fn weapon_mounts_from_js(
    owner: &str,
    raw: JsValue<'_>,
) -> Result<Vec<WeaponMountDescriptor>, DescriptorError> {
    let Some(array) = raw.as_array() else {
        return Err(weapon_mount_shape_error(
            owner,
            "`components.mesh.weaponMounts` must be an array of `{ weapon, attachments }` entries",
        ));
    };
    let mut entries = Vec::with_capacity(array.len());
    for index in 0..array.len() {
        let path = format!("components.mesh.weaponMounts[{index}]");
        let value: JsValue = array.get(index).map_err(js_err)?;
        if value.type_of() != rquickjs::Type::Object {
            return Err(weapon_mount_shape_error(
                owner,
                format_args!("`{path}` must be an object `{{ weapon, attachments }}`"),
            ));
        }
        let entry = Object::from_value(value).map_err(js_err)?;
        for key in sorted_own_keys(&entry)? {
            check_weapon_mount_key(owner, index, &key)?;
        }
        let weapon: JsValue = entry.get("weapon").map_err(js_err)?;
        let Some(weapon) = weapon.as_string() else {
            return Err(weapon_mount_shape_error(
                owner,
                format_args!(
                    "`{path}.weapon` must be a weapon name; `defineEntity` lowers a weapon descriptor to its canonical name"
                ),
            ));
        };
        let weapon = weapon.to_string().map_err(js_err)?;
        let attachments: JsValue = entry.get("attachments").map_err(js_err)?;
        let attachments = string_map_from_js(attachments, &format!("{path}.attachments"))
            .map_err(|reason| weapon_mount_shape_error(owner, reason))?;
        entries.push(WeaponMountDescriptor {
            weapon,
            attachments,
        });
    }
    Ok(entries)
}
