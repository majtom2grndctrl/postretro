// Data-context descriptors: JS readers for the mesh key set and the
// `components.mesh.mounts` table.
// See: context/lib/scripting.md

use super::super::*;
use postretro_entities::data_descriptors::{
    MountDescriptor, check_mesh_key, check_mount_key, mount_shape_error,
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

/// Read `components.mesh.mounts`: an array of `{ item, attachments }` or `{ item, muzzleSocket }`
/// entries whose `item` the SDK has lowered to a canonical name. Shape
/// errors name the character; the shared rules run in `MeshDescriptor::build`.
pub(super) fn mounts_from_js(
    owner: &str,
    raw: JsValue<'_>,
) -> Result<Vec<MountDescriptor>, DescriptorError> {
    let Some(array) = raw.as_array() else {
        return Err(mount_shape_error(
            owner,
            "`components.mesh.mounts` must be an array of `{ item, attachments }` or `{ item, muzzleSocket }` entries",
        ));
    };
    let mut entries = Vec::with_capacity(array.len());
    for index in 0..array.len() {
        let path = format!("components.mesh.mounts[{index}]");
        let value: JsValue = array.get(index).map_err(js_err)?;
        if value.type_of() != rquickjs::Type::Object {
            return Err(mount_shape_error(
                owner,
                format_args!(
                    "`{path}` must be an object `{{ item, attachments }}` or `{{ item, muzzleSocket }}`"
                ),
            ));
        }
        let entry = Object::from_value(value).map_err(js_err)?;
        for key in sorted_own_keys(&entry)? {
            check_mount_key(owner, index, &key)?;
        }
        let item: JsValue = entry.get("item").map_err(js_err)?;
        let Some(item) = item.as_string() else {
            return Err(mount_shape_error(
                owner,
                format_args!(
                    "`{path}.item` must be a weapon name; `defineEntity` lowers a weapon descriptor to its canonical name"
                ),
            ));
        };
        let item = item.to_string().map_err(js_err)?;
        let attachments: JsValue = entry.get("attachments").map_err(js_err)?;
        let attachments = if attachments.is_undefined() {
            None
        } else {
            Some(
                string_map_from_js(attachments, &format!("{path}.attachments"))
                    .map_err(|reason| mount_shape_error(owner, reason))?,
            )
        };
        let muzzle_socket: JsValue = entry.get("muzzleSocket").map_err(js_err)?;
        let muzzle_socket = if muzzle_socket.is_undefined() {
            None
        } else {
            let Some(socket) = muzzle_socket.as_string() else {
                return Err(mount_shape_error(
                    owner,
                    format_args!("`{path}.muzzleSocket` must be a socket name string"),
                ));
            };
            Some(socket.to_string().map_err(js_err)?)
        };
        entries.push(MountDescriptor {
            item,
            attachments,
            muzzle_socket,
        });
    }
    Ok(entries)
}
