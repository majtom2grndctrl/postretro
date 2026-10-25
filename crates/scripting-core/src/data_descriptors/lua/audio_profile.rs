// Luau drain for the manifest's `audio` block: attenuation and the Ghost bus.
// See: context/lib/scripting.md §1 · context/lib/audio.md §1, §5

use super::super::*;

/// Luau twin of [`drain_audio_profile_js`]: same validation and degradation,
/// through the same shared attenuation and ghost bus resolvers.
pub fn drain_audio_profile_lua(
    table: &Table,
    scope: &str,
) -> Result<ModAudioProfile, DescriptorError> {
    let raw_audio: LuaValue = table.get("audio").map_err(lua_err)?;
    let audio = match raw_audio {
        LuaValue::Nil => return Ok(ModAudioProfile::default()),
        // Luau has no array/object distinction at the type level, so a
        // sequence-style table (`audio = {}` with integer keys, e.g. `{1, 2}`)
        // must be caught explicitly here to match QuickJS's `Array.isArray`
        // rejection — otherwise it silently passes as a valid, field-less
        // table and the seeded default applies with no warning.
        LuaValue::Table(audio) if audio.raw_len() == 0 => audio,
        _ => {
            log::warn!(
                "[Scripting] {scope}: `audio` must be a table; using the default audio profile"
            );
            return Ok(ModAudioProfile::default());
        }
    };

    let attenuation =
        match audio_block_lua(&audio, "attenuation", "the default attenuation", scope)? {
            Some(attenuation) => resolve_authored_attenuation(
                scope,
                audio_field_lua(&attenuation, "minDistance")?,
                audio_field_lua(&attenuation, "maxDistance")?,
                audio_field_lua(&attenuation, "curve")?,
            ),
            None => ModAttenuation::DEFAULT,
        };
    let ghost = match audio_block_lua(&audio, "ghost", "the floor ghost bus profile", scope)? {
        Some(ghost) => resolve_authored_ghost_audio(
            scope,
            audio_field_lua(&ghost, "volume")?,
            audio_field_lua(&ghost, "highPassHz")?,
            audio_field_lua(&ghost, "lowPassHz")?,
            audio_field_lua(&ghost, "drive")?,
        ),
        None => ModGhostAudio::FLOOR,
    };
    Ok(ModAudioProfile { attenuation, ghost })
}

/// One optional table under `audio`. `None` when it is `nil`, or malformed
/// (which warns, naming `fallback`).
fn audio_block_lua(
    audio: &Table,
    key: &str,
    fallback: &str,
    scope: &str,
) -> Result<Option<Table>, DescriptorError> {
    match audio.get::<LuaValue>(key).map_err(lua_err)? {
        LuaValue::Nil => Ok(None),
        // Same sequence-style rejection as `audio` itself, for its blocks.
        LuaValue::Table(block) if block.raw_len() == 0 => Ok(Some(block)),
        _ => {
            log::warn!("[Scripting] {scope}: `audio.{key}` must be a table; using {fallback}");
            Ok(None)
        }
    }
}

fn audio_field_lua(block: &Table, key: &str) -> Result<AuthoredAudioField, DescriptorError> {
    Ok(match block.get::<LuaValue>(key).map_err(lua_err)? {
        LuaValue::Nil => AuthoredAudioField::Absent,
        LuaValue::Integer(value) => AuthoredAudioField::Number(value as f64),
        LuaValue::Number(value) => AuthoredAudioField::Number(value),
        LuaValue::String(value) => match value.to_str() {
            Ok(value) => AuthoredAudioField::String(value.to_string()),
            Err(_) => AuthoredAudioField::Other,
        },
        _ => AuthoredAudioField::Other,
    })
}
