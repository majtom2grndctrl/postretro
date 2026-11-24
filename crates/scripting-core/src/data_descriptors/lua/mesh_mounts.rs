// Data-context descriptors: Luau readers for the mesh key set and the
// `components.mesh.mounts` table. Mirrors `js/mesh_mounts.rs`.
// See: context/lib/scripting.md

use super::super::*;
use postretro_entities::data_descriptors::{
    MountDescriptor, check_mesh_key, check_mount_key, mount_shape_error,
};

/// A table's keys in display form, sorted so the first rejected key matches
/// the QuickJS reader.
fn sorted_keys(table: &Table) -> Result<Vec<String>, DescriptorError> {
    let mut keys = Vec::new();
    for pair in table.pairs::<LuaValue, LuaValue>() {
        let (key, _) = pair.map_err(lua_err)?;
        keys.push(match key {
            LuaValue::String(s) => s.to_str().map_err(lua_err)?.to_string(),
            LuaValue::Integer(i) => i.to_string(),
            LuaValue::Number(n) => n.to_string(),
            other => format!("<{}>", other.type_name()),
        });
    }
    keys.sort();
    Ok(keys)
}

/// Reject any key of `components.mesh` outside the mesh vocabulary.
pub(super) fn reject_unknown_mesh_keys(owner: &str, mesh: &Table) -> Result<(), DescriptorError> {
    for key in sorted_keys(mesh)? {
        check_mesh_key(owner, &key)?;
    }
    Ok(())
}

/// Read a `socket → model path` table at `path`. The error is the bare
/// reason; callers decide whether it names the character.
pub(super) fn string_map_from_lua(
    raw: LuaValue,
    path: &str,
) -> Result<HashMap<String, String>, String> {
    let LuaValue::Table(map) = raw else {
        return Err(format!("`{path}` must be a table, got {}", raw.type_name()));
    };
    let mut out = HashMap::new();
    for pair in map.pairs::<LuaValue, LuaValue>() {
        let (key, value) = pair.map_err(|error| error.to_string())?;
        let socket = match key {
            LuaValue::String(value) => value
                .to_str()
                .map_err(|error| error.to_string())?
                .to_string(),
            other => {
                return Err(format!(
                    "`{path}` must be a socket-name map, got {} key",
                    other.type_name()
                ));
            }
        };
        let model = match value {
            LuaValue::String(value) => value
                .to_str()
                .map_err(|error| error.to_string())?
                .to_string(),
            other => {
                return Err(format!(
                    "`{path}.{socket}` must be a string, got {}",
                    other.type_name()
                ));
            }
        };
        out.insert(socket, model);
    }
    Ok(out)
}

/// Read `components.mesh.mounts`: a sequence of `{ item, attachments }`
/// entries whose `item` the SDK has lowered to a canonical name. Shape
/// errors name the character; the shared rules run in `MeshDescriptor::build`.
pub(super) fn mounts_from_lua(
    owner: &str,
    raw: LuaValue,
) -> Result<Vec<MountDescriptor>, DescriptorError> {
    let not_a_list = || {
        mount_shape_error(
            owner,
            "`components.mesh.mounts` must be an array of `{ item, attachments }` entries",
        )
    };
    let LuaValue::Table(list) = raw else {
        return Err(not_a_list());
    };
    // A sequence: exactly the integer keys 1..=n, nothing else.
    let len = list.raw_len();
    let mut count = 0usize;
    for pair in list.pairs::<LuaValue, LuaValue>() {
        let (key, _) = pair.map_err(lua_err)?;
        match key {
            LuaValue::Integer(i) if i >= 1 && (i as usize) <= len => count += 1,
            _ => return Err(not_a_list()),
        }
    }
    if count != len {
        return Err(not_a_list());
    }

    let mut entries = Vec::with_capacity(len);
    for index in 0..len {
        let path = format!("components.mesh.mounts[{index}]");
        let value: LuaValue = list.raw_get(index + 1).map_err(lua_err)?;
        let LuaValue::Table(entry) = value else {
            return Err(mount_shape_error(
                owner,
                format_args!("`{path}` must be an object `{{ item, attachments }}`"),
            ));
        };
        for key in sorted_keys(&entry)? {
            check_mount_key(owner, index, &key)?;
        }
        let item = match entry.raw_get::<LuaValue>("item").map_err(lua_err)? {
            LuaValue::String(name) => name.to_str().map_err(lua_err)?.to_string(),
            _ => {
                return Err(mount_shape_error(
                    owner,
                    format_args!(
                        "`{path}.item` must be a weapon name; `defineEntity` lowers a weapon descriptor to its canonical name"
                    ),
                ));
            }
        };
        let attachments = match entry.raw_get::<LuaValue>("attachments").map_err(lua_err)? {
            LuaValue::Nil => None,
            attachments => Some(
                string_map_from_lua(attachments, &format!("{path}.attachments"))
                    .map_err(|reason| mount_shape_error(owner, reason))?,
            ),
        };
        let muzzle_socket = match entry.raw_get::<LuaValue>("muzzleSocket").map_err(lua_err)? {
            LuaValue::Nil => None,
            LuaValue::String(socket) => Some(socket.to_str().map_err(lua_err)?.to_string()),
            _ => {
                return Err(mount_shape_error(
                    owner,
                    format_args!("`{path}.muzzleSocket` must be a socket name string"),
                ));
            }
        };
        entries.push(MountDescriptor {
            item,
            attachments,
            muzzle_socket,
        });
    }
    Ok(entries)
}
