// QuickJS drain for the manifest's `audio` block: attenuation and the Ghost bus.
// See: context/lib/scripting.md §1 · context/lib/audio.md §1, §5

use super::super::*;

/// Drain the optional mod-wide audio profile. Like the render profile, a
/// malformed value warns naming its path and falls back to the engine seed;
/// it never rejects an otherwise valid manifest. `audio.attenuation` and
/// `audio.ghost` resolve independently: either may be absent or malformed
/// without touching the other.
pub fn drain_audio_profile_js<'js>(
    obj: &Object<'js>,
    scope: &str,
) -> Result<ModAudioProfile, DescriptorError> {
    if !obj.contains_key("audio").map_err(js_err)? {
        return Ok(ModAudioProfile::default());
    }
    let raw_audio: JsValue = obj.get("audio").map_err(js_err)?;
    if raw_audio.is_null() || raw_audio.is_undefined() {
        return Ok(ModAudioProfile::default());
    }
    if !raw_audio.is_object() || raw_audio.is_array() {
        log::warn!(
            "[Scripting] {scope}: `audio` must be an object; using the default audio profile"
        );
        return Ok(ModAudioProfile::default());
    }
    let audio = raw_audio
        .as_object()
        .expect("object type was checked before borrowing");

    let attenuation = match audio_block_js(audio, "attenuation", "the default attenuation", scope)?
    {
        Some(attenuation) => resolve_authored_attenuation(
            scope,
            audio_field_js(&attenuation, "minDistance")?,
            audio_field_js(&attenuation, "maxDistance")?,
            audio_field_js(&attenuation, "curve")?,
        ),
        None => ModAttenuation::DEFAULT,
    };
    let ghost = match audio_block_js(audio, "ghost", "the floor ghost bus profile", scope)? {
        Some(ghost) => resolve_authored_ghost_audio(
            scope,
            audio_field_js(&ghost, "volume")?,
            audio_field_js(&ghost, "highPassHz")?,
            audio_field_js(&ghost, "lowPassHz")?,
            audio_field_js(&ghost, "drive")?,
        ),
        None => ModGhostAudio::FLOOR,
    };
    Ok(ModAudioProfile { attenuation, ghost })
}

/// One optional object under `audio`. `None` when it is absent, `null` or
/// `undefined`, or malformed (which warns, naming `fallback`).
fn audio_block_js<'js>(
    audio: &Object<'js>,
    key: &str,
    fallback: &str,
    scope: &str,
) -> Result<Option<Object<'js>>, DescriptorError> {
    if !audio.contains_key(key).map_err(js_err)? {
        return Ok(None);
    }
    let raw: JsValue = audio.get(key).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    if !raw.is_object() || raw.is_array() {
        log::warn!("[Scripting] {scope}: `audio.{key}` must be an object; using {fallback}");
        return Ok(None);
    }
    Ok(raw.into_object())
}

fn audio_field_js<'js>(
    block: &Object<'js>,
    key: &str,
) -> Result<AuthoredAudioField, DescriptorError> {
    if !block.contains_key(key).map_err(js_err)? {
        return Ok(AuthoredAudioField::Absent);
    }
    let raw: JsValue = block.get(key).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(AuthoredAudioField::Absent);
    }
    if let Some(value) = raw.as_int().map(f64::from).or_else(|| raw.as_float()) {
        return Ok(AuthoredAudioField::Number(value));
    }
    Ok(
        match raw.as_string().and_then(|value| value.to_string().ok()) {
            Some(value) => AuthoredAudioField::String(value),
            None => AuthoredAudioField::Other,
        },
    )
}
