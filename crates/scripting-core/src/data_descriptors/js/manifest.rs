// Data-context descriptors: JS manifest drains (uiTrees/theme/fonts/maps).
// See: context/lib/scripting.md

use super::super::*;

impl LevelManifest {
    /// Deserialize a top-level `{ reactions, events, crossings, triggerEvents, triggerPools, uiTrees }`
    /// object returned from a QuickJS `setupLevel()` call. Each array field is optional.
    pub fn from_js_value<'js>(
        ctx: &Ctx<'js>,
        value: JsValue<'js>,
    ) -> Result<Self, DescriptorError> {
        let obj = Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
            reason: "setupLevel must return an object".to_string(),
        })?;

        let reactions = if obj.contains_key("reactions").map_err(js_err)? {
            let arr: Array = obj.get("reactions").map_err(js_err)?;
            let mut out = Vec::with_capacity(arr.len());
            for i in 0..arr.len() {
                let item: JsValue = arr.get(i).map_err(js_err)?;
                let is_resource_grant = is_resource_grant_reaction_js(&item);
                match named_reaction_from_js(ctx, item) {
                    Ok(reaction) => out.push(reaction),
                    Err(error) if is_resource_grant => return Err(error),
                    Err(error) => log::warn!(
                        "[Scripting] setupLevel: reactions[{i}] is malformed and was skipped: {error}"
                    ),
                }
            }
            out
        } else {
            Vec::new()
        };

        let crossings = if obj.contains_key("crossings").map_err(js_err)? {
            let arr: Array = obj.get("crossings").map_err(js_err)?;
            let mut out = Vec::with_capacity(arr.len());
            for i in 0..arr.len() {
                let item: JsValue = arr.get(i).map_err(js_err)?;
                out.push(crossing_descriptor_from_js(ctx, &item)?);
            }
            out
        } else {
            Vec::new()
        };
        let events = drain_impact_events_js(ctx, &obj, "setupLevel")?;
        let trigger_events = drain_trigger_events_js(&obj, "setupLevel")?;
        let trigger_pools = drain_trigger_pools_js(&obj, "setupLevel")?;

        let ui_trees = drain_ui_trees_js(ctx, &obj, "setupLevel")?;

        Ok(Self {
            reactions,
            events,
            crossings,
            trigger_events,
            trigger_pools,
            ui_trees,
        })
    }
}

/// Resource grants alter engine-owned state. Their descriptor contract is
/// intentionally strict: unlike ordinary malformed reaction entries, a bad
/// grant rejects the setup manifest rather than silently omitting a pickup or
/// reward path.
fn is_resource_grant_reaction_js<'js>(value: &JsValue<'js>) -> bool {
    let Ok(object) = Object::from_value(value.clone()) else {
        return false;
    };
    let primitive: JsValue = match object.get("primitive") {
        Ok(primitive) => primitive,
        Err(_) => return false,
    };
    let Some(primitive) = primitive.as_string() else {
        return false;
    };
    matches!(
        primitive.to_string().ok().as_deref(),
        Some("grantHealth" | "grantAmmo")
    )
}

/// Drain the strict mod-global switching declaration. Unlike presentation
/// preferences, an invalid switching policy changes simulation authorization,
/// so the complete mod-init attempt must fail rather than degrade a field.
pub fn drain_switching_js<'js>(
    obj: &Object<'js>,
    scope: &str,
) -> Result<SwitchingDescriptor, DescriptorError> {
    if !obj.contains_key("switching").map_err(js_err)? {
        return Ok(SwitchingDescriptor::default());
    }
    let raw: JsValue = obj.get("switching").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(SwitchingDescriptor::default());
    }
    if !raw.is_object() || raw.is_array() {
        return Err(DescriptorError::InvalidShape {
            reason: format!("{scope}: `switching` must be an object"),
        });
    }
    let switching = raw
        .as_object()
        .expect("object type was checked before borrowing");
    let commit_on_direct_select = get_required_bool_js(&switching, "commitOnDirectSelect")
        .map_err(|error| switching_field_error(scope, "commitOnDirectSelect", error))?;
    let cycle_commit_dwell_ms = get_required_f32_js(&switching, "cycleCommitDwellMs")
        .map_err(|error| switching_field_error(scope, "cycleCommitDwellMs", error))?;
    let block_during_reload = get_required_bool_js(&switching, "blockDuringReload")
        .map_err(|error| switching_field_error(scope, "blockDuringReload", error))?;

    SwitchingDescriptor {
        commit_on_direct_select,
        cycle_commit_dwell_ms,
        block_during_reload,
    }
    .validate()
    .map_err(|error| DescriptorError::InvalidShape {
        reason: format!("{scope}: {error}"),
    })
}

fn switching_field_error(scope: &str, field: &str, error: DescriptorError) -> DescriptorError {
    DescriptorError::InvalidShape {
        reason: format!("{scope}: `switching.{field}` invalid: {error}"),
    }
}

/// Drain the optional static renderer profile from a QuickJS mod manifest.
/// Every malformed field degrades independently so presentation preferences
/// never reject an otherwise valid manifest.
pub fn drain_render_profile_js<'js>(
    obj: &Object<'js>,
    scope: &str,
) -> Result<ModRenderProfile, DescriptorError> {
    if !obj.contains_key("render").map_err(js_err)? {
        return Ok(ModRenderProfile::default());
    }
    let raw_render: JsValue = obj.get("render").map_err(js_err)?;
    // `render: undefined`/`null` is the idiomatic TS spelling of an absent
    // optional; treat it as omission (silent default) so QuickJS matches Luau's
    // `nil` path, like every other manifest drain in this file.
    if raw_render.is_null() || raw_render.is_undefined() {
        return Ok(ModRenderProfile::default());
    }
    if !raw_render.is_object() || raw_render.is_array() {
        log::warn!(
            "[Scripting] {scope}: `render` must be an object; using the default render profile"
        );
        return Ok(ModRenderProfile::default());
    }
    let render = raw_render
        .as_object()
        .expect("object type was checked before borrowing");
    if !render.contains_key("bloom").map_err(js_err)? {
        return Ok(ModRenderProfile::default());
    }
    let raw_bloom: JsValue = render.get("bloom").map_err(js_err)?;
    if raw_bloom.is_null() || raw_bloom.is_undefined() {
        return Ok(ModRenderProfile::default());
    }
    if !raw_bloom.is_object() || raw_bloom.is_array() {
        log::warn!(
            "[Scripting] {scope}: `render.bloom` must be an object; using the default bloom profile"
        );
        return Ok(ModRenderProfile::default());
    }
    let bloom = raw_bloom
        .as_object()
        .expect("object type was checked before borrowing");

    let resolution = if bloom.contains_key("resolution").map_err(js_err)? {
        let raw: JsValue = bloom.get("resolution").map_err(js_err)?;
        let authored = raw.as_string().and_then(|value| value.to_string().ok());
        match authored.as_deref() {
            Some("half") => ModBloomResolution::Half,
            Some("quarter") => ModBloomResolution::Quarter,
            Some("eighth") => ModBloomResolution::Eighth,
            _ => {
                log::warn!(
                    "[Scripting] {scope}: `render.bloom.resolution` must be `half`, `quarter`, or `eighth`; using `half`"
                );
                ModBloomResolution::default()
            }
        }
    } else {
        ModBloomResolution::default()
    };

    let pixelated = if bloom.contains_key("pixelated").map_err(js_err)? {
        let raw: JsValue = bloom.get("pixelated").map_err(js_err)?;
        match raw.as_bool() {
            Some(value) => value,
            None => {
                log::warn!(
                    "[Scripting] {scope}: `render.bloom.pixelated` must be a boolean; using `false`"
                );
                false
            }
        }
    } else {
        false
    };

    Ok(ModRenderProfile {
        bloom: ModBloomProfile {
            resolution,
            pixelated,
        },
    })
}

/// Drain pure SDK `defineImpactEvent` handles from a manifest. Parsing stops at
/// the descriptor boundary: Task 5 owns policy validation, author-id merging,
/// and effect evaluation.
pub fn drain_impact_events_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    scope: &str,
) -> Result<Vec<ImpactEventDescriptor>, DescriptorError> {
    let Some(arr) = optional_manifest_array_js(obj, "events", scope)? else {
        return Ok(Vec::new());
    };
    if arr.len() > MAX_IMPACT_EVENT_CONTAINER_ENTRIES {
        log::warn!(
            "[Scripting] {scope}: `events` exceeds {MAX_IMPACT_EVENT_CONTAINER_ENTRIES} array slots; ignoring the field"
        );
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(arr.len());
    for i in 0..arr.len() {
        let value: JsValue = match arr.get(i) {
            Ok(value) => value,
            Err(error) => {
                log::warn!(
                    "[Scripting] {scope}: events[{i}] could not be read and was skipped: {}",
                    js_err(error)
                );
                continue;
            }
        };
        match impact_event_from_js(ctx, value) {
            Ok(descriptor) => out.push(descriptor),
            Err(error) => {
                log::warn!("[Scripting] {scope}: events[{i}] is malformed and was skipped: {error}")
            }
        }
    }
    Ok(out)
}

fn impact_event_from_js<'js>(
    ctx: &Ctx<'js>,
    value: JsValue<'js>,
) -> Result<ImpactEventDescriptor, DescriptorError> {
    let item = Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
        reason: "impact-event entry must be an object".into(),
    })?;
    let kind = get_required_string_js(&item, "kind")?;
    if kind != "impact" {
        return Err(DescriptorError::InvalidShape {
            reason: "impact-event entry `kind` must be `impact`".into(),
        });
    }

    let filter: Object = item
        .get("filter")
        .map_err(|_| DescriptorError::MissingField { field: "filter" })?;
    let filter_tag = if filter.contains_key("tag").map_err(js_err)? {
        let raw: JsValue = filter.get("tag").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            Some(String::from_js_value_required(raw, "tag")?)
        }
    } else {
        None
    };

    let policy: Array = item
        .get("policy")
        .map_err(|_| DescriptorError::InvalidShape {
            reason: "impact-event entry `policy` must be an array".into(),
        })?;
    let mut policy_json = Vec::with_capacity(policy.len());
    for i in 0..policy.len() {
        let raw: JsValue = policy.get(i).map_err(js_err)?;
        if raw.is_undefined() {
            return Err(DescriptorError::InvalidShape {
                reason: "impact-event entry `policy` must be a dense array; holes are not allowed"
                    .into(),
            });
        }
        policy_json.push(impact_policy_entry_from_js(ctx, raw)?);
    }

    let is_override = get_optional_bool_js(&item, "isOverride")?.unwrap_or(false);
    if is_override && filter_tag.is_none() {
        return Err(DescriptorError::InvalidShape {
            reason: "impact-event override `filter.tag` is required".into(),
        });
    }

    Ok(ImpactEventDescriptor {
        id: validate_impact_event_id(get_required_string_js(&item, "id")?)?,
        is_override,
        levels: string_array_from_js(&item, "levels")?,
        filter_tag,
        policy: policy_json,
    })
}

fn impact_policy_entry_from_js<'js>(
    ctx: &Ctx<'js>,
    raw: JsValue<'js>,
) -> Result<serde_json::Value, DescriptorError> {
    let Some(object) = raw.as_object() else {
        return conv::js_to_json(ctx, raw).map_err(js_err);
    };
    if !object.contains_key("do").map_err(js_err)? {
        return conv::js_to_json(ctx, raw).map_err(js_err);
    }

    let effects: Array = object
        .get("do")
        .map_err(|_| DescriptorError::InvalidShape {
            reason: "impact policy group `do` must be an array".into(),
        })?;
    let mut json = conv::js_to_json(ctx, raw).map_err(js_err)?;
    let json_object = json
        .as_object_mut()
        .ok_or_else(|| DescriptorError::InvalidShape {
            reason: "impact policy group must be an object".into(),
        })?;
    let mut lowered = Vec::with_capacity(effects.len());
    for i in 0..effects.len() {
        let effect: JsValue = effects.get(i).map_err(js_err)?;
        if effect.is_undefined() {
            return Err(DescriptorError::InvalidShape {
                reason: "impact policy group `do` must be a dense array; holes are not allowed"
                    .into(),
            });
        }
        lowered.push(conv::js_to_json(ctx, effect).map_err(js_err)?);
    }
    json_object.insert("do".into(), serde_json::Value::Array(lowered));
    Ok(json)
}

/// Drain the `triggerEvents` array from a QuickJS manifest object. Mirrors
/// [`drain_ui_trees_js`]: a malformed entry (non-object, or missing/invalid
/// `tag`/`fire`/`levels`) is logged and skipped rather than aborting the whole
/// manifest; an unknown `event` value is likewise logged and skipped.
pub fn drain_trigger_events_js<'js>(
    obj: &Object<'js>,
    scope: &str,
) -> Result<Vec<TriggerEventDescriptor>, DescriptorError> {
    let Some(arr) = optional_manifest_array_js(obj, "triggerEvents", scope)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(arr.len());
    for i in 0..arr.len() {
        let value: JsValue = arr.get(i).map_err(js_err)?;
        match trigger_event_from_js(value, i, scope) {
            Ok(Some(descriptor)) => out.push(descriptor),
            Ok(None) => {}
            Err(e) => log::warn!(
                "[Scripting] {scope}: triggerEvents[{i}] is malformed and was skipped: {e}"
            ),
        }
    }
    Ok(out)
}

/// Parse a single `triggerEvents` entry (`{ event, tag, fire, levels? }`) from
/// JS. `Ok(None)` means the entry parsed but its `event` is unrecognized (the
/// caller has already logged the reason); a genuinely malformed entry returns
/// `Err` for the caller to log and skip.
fn trigger_event_from_js<'js>(
    value: JsValue<'js>,
    i: usize,
    scope: &str,
) -> Result<Option<TriggerEventDescriptor>, DescriptorError> {
    let item = Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
        reason: "trigger-event entry must be an object".into(),
    })?;
    let event = get_required_string_js(&item, "event")?;
    if !matches!(event.as_str(), "enter" | "exit") {
        log::warn!(
            "[Scripting] {scope}: triggerEvents[{i}] has unknown event `{event}` and was skipped"
        );
        return Ok(None);
    }
    Ok(Some(TriggerEventDescriptor {
        tag: get_required_string_js(&item, "tag")?,
        event,
        fire: string_array_from_js(&item, "fire")?,
        levels: string_array_from_js(&item, "levels")?,
    }))
}

/// Drain the `triggerPools` array from a QuickJS manifest object. A malformed
/// entry is logged and skipped so one bad pool does not abort the manifest.
/// Pool tags are unique within this drain; a later duplicate is skipped.
pub fn drain_trigger_pools_js<'js>(
    obj: &Object<'js>,
    scope: &str,
) -> Result<Vec<TriggerPoolDescriptor>, DescriptorError> {
    let Some(arr) = optional_trigger_pool_array_js(obj, scope)? else {
        return Ok(Vec::new());
    };
    let entries = trigger_pool_entries_js(&arr, scope)?;
    let mut out = Vec::with_capacity(entries.len());
    let mut seen_tags = BTreeSet::new();
    for (i, value) in entries {
        match trigger_pool_from_js(value) {
            Ok(descriptor) if seen_tags.insert(descriptor.tag.clone()) => out.push(descriptor),
            Ok(descriptor) => log::warn!(
                "[Scripting] {scope}: triggerPools[{i}] duplicates pool tag `{}` and was skipped",
                descriptor.tag,
            ),
            Err(e) => log::warn!(
                "[Scripting] {scope}: triggerPools[{i}] is malformed and was skipped: {e}"
            ),
        }
    }
    Ok(out)
}

fn optional_trigger_pool_array_js<'js>(
    obj: &Object<'js>,
    scope: &str,
) -> Result<Option<Array<'js>>, DescriptorError> {
    let has_trigger_pools = match obj.contains_key("triggerPools") {
        Ok(has_trigger_pools) => has_trigger_pools,
        Err(error) => {
            let error = caught_js_err(obj.ctx(), error);
            log::warn!(
                "[Scripting] {scope}: could not access `triggerPools`; ignoring the field: {error}"
            );
            return Ok(None);
        }
    };
    if !has_trigger_pools {
        return Ok(None);
    }
    let raw: JsValue = match obj.get("triggerPools") {
        Ok(raw) => raw,
        Err(error) => {
            let error = caught_js_err(obj.ctx(), error);
            log::warn!(
                "[Scripting] {scope}: could not access `triggerPools`; ignoring the field: {error}"
            );
            return Ok(None);
        }
    };
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    let Some(arr) = raw.as_array() else {
        log::warn!("[Scripting] {scope}: `triggerPools` must be an array; ignoring the field");
        return Ok(None);
    };
    Ok(Some(arr.clone()))
}

/// Walk the bounded slot range directly. Holes are valid, but arrays whose
/// declared length exceeds the authoring limit degrade before any slot reads.
fn trigger_pool_entries_js<'js>(
    arr: &Array<'js>,
    scope: &str,
) -> Result<Vec<(u64, JsValue<'js>)>, DescriptorError> {
    // `Array::len()` asserts that QuickJS stores `length` as a 32-bit integer.
    // Read the standard array length as a JS number instead so a hostile sparse
    // index still degrades through this parser rather than panicking.
    let len = match safe_js_array_len(arr, "`triggerPools`") {
        Ok(len) => len,
        Err(error) => {
            log::warn!(
                "[Scripting] {scope}: could not read `triggerPools.length`; ignoring the field: {error}"
            );
            return Ok(Vec::new());
        }
    };
    if len > MAX_TRIGGER_POOL_CONTAINER_ENTRIES {
        log::warn!(
            "[Scripting] {scope}: `triggerPools` exceeds the {}-slot limit; ignoring the field",
            MAX_TRIGGER_POOL_CONTAINER_ENTRIES,
        );
        return Ok(Vec::new());
    }

    let mut entries = Vec::with_capacity(len);
    for index in 0..len {
        let value: JsValue = match arr.get(index) {
            Ok(value) => value,
            Err(error) => {
                let error = caught_js_err(arr.ctx(), error);
                log::warn!(
                    "[Scripting] {scope}: triggerPools[{index}] accessor failed and was skipped: {error}"
                );
                continue;
            }
        };
        if !value.is_undefined() {
            entries.push((index as u64, value));
        }
    }
    Ok(entries)
}

fn safe_js_array_len<'js>(arr: &Array<'js>, field: &str) -> Result<usize, DescriptorError> {
    let raw_len: JsValue = arr
        .as_object()
        .get("length")
        .map_err(|error| caught_js_err(arr.ctx(), error))?;
    raw_len
        .as_int()
        .and_then(|value| usize::try_from(value).ok())
        .or_else(|| {
            raw_len.as_float().and_then(|value| {
                (value.is_finite()
                    && value >= 0.0
                    && value.fract() == 0.0
                    && value <= usize::MAX as f64)
                    .then_some(value as usize)
            })
        })
        .ok_or_else(|| DescriptorError::InvalidShape {
            reason: format!("{field} has an invalid array length"),
        })
}

fn caught_js_err<'js>(ctx: &Ctx<'js>, error: rquickjs::Error) -> DescriptorError {
    if error.is_exception() {
        let _ = ctx.catch();
    }
    js_err(error)
}

fn trigger_pool_from_js<'js>(
    value: JsValue<'js>,
) -> Result<TriggerPoolDescriptor, DescriptorError> {
    let item = Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
        reason: "trigger-pool entry must be an object".into(),
    })?;
    let tag = get_required_string_js(&item, "tag")?;
    if tag.is_empty() {
        return Err(DescriptorError::InvalidShape {
            reason: "trigger-pool `tag` must not be empty".into(),
        });
    }

    let arm_value: JsValue = item.get("arm").map_err(js_err)?;
    let percentage_value: JsValue = item.get("armPercentage").map_err(js_err)?;
    // Treat null/undefined like an omitted optional property, matching Luau's
    // `nil` semantics and keeping the TS/Luau descriptor drains equivalent.
    let has_arm = !arm_value.is_null() && !arm_value.is_undefined();
    let has_percentage = !percentage_value.is_null() && !percentage_value.is_undefined();
    if has_arm == has_percentage {
        return Err(DescriptorError::InvalidShape {
            reason: "trigger-pool must define exactly one of `arm` or `armPercentage`".into(),
        });
    }

    let arm = if has_arm {
        TriggerPoolArm::Count(get_required_u32_js(&item, "arm")?)
    } else {
        let raw = percentage_value;
        let percentage = if let Some(value) = raw.as_int() {
            value as f64
        } else if let Some(value) = raw.as_float() {
            value
        } else {
            return Err(DescriptorError::InvalidShape {
                reason: "'armPercentage' must be a number".into(),
            });
        };
        if !percentage.is_finite() || !(0.0..=100.0).contains(&percentage) {
            return Err(DescriptorError::InvalidShape {
                reason: "'armPercentage' must be finite and in [0, 100]".into(),
            });
        }
        TriggerPoolArm::Percentage(percentage)
    };

    Ok(TriggerPoolDescriptor {
        tag,
        arm,
        levels: trigger_pool_levels_from_js(&item)?,
    })
}

fn trigger_pool_levels_from_js<'js>(item: &Object<'js>) -> Result<Vec<String>, DescriptorError> {
    let raw: JsValue = item
        .get("levels")
        .map_err(|error| caught_js_err(item.ctx(), error))?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(Vec::new());
    }
    let Some(arr) = raw.as_array() else {
        return Err(DescriptorError::InvalidShape {
            reason: "`levels` must be a string array".into(),
        });
    };
    let len = safe_js_array_len(arr, "`levels`")?;
    if len > MAX_TRIGGER_POOL_CONTAINER_ENTRIES {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`levels` exceeds the {}-slot limit",
                MAX_TRIGGER_POOL_CONTAINER_ENTRIES
            ),
        });
    }

    let mut levels = Vec::with_capacity(len);
    for index in 0..len {
        let value: JsValue = arr
            .get(index)
            .map_err(|error| caught_js_err(arr.ctx(), error))?;
        levels.push(String::from_js_value_required(value, "levels")?);
    }
    Ok(levels)
}

// ===========================================================================
// Manifest-level UI field drains for ModManifest/setupLevel().
//
// `uiTrees` / `theme` / `fonts` are optional fields on the mod manifest
// (mod scope); `uiTrees` is also optional on `setupLevel()` (level scope). Each
// drain reads the field straight off the returned object/table via the per-
// runtime field readers, building typed values held on the manifest result.
//
// Degradation contract (ui.md §1.1): a malformed UI *registration* (a tree entry
// that fails its own parse, including the `anchored_tree_from_*` bridge)
// produces a named load-time diagnostic and is SKIPPED — it never aborts the
// boot / level-load pass and never panics. A malformed *container* (the
// `uiTrees`/`theme`/`fonts` field itself not being the expected shape) is also
// logged and degraded to "no UI from this field" rather than failing the parse,
// for the same reason: a bad UI field must not take down mod-init.
// ===========================================================================

/// Drain the `uiTrees` array from a QuickJS manifest object. `scope` is a short
/// label ("ModManifest" / "setupLevel") used in diagnostics. Malformed entries are
/// logged and skipped; a non-array `uiTrees` field is logged and yields empty.
pub fn drain_ui_trees_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    scope: &str,
) -> Result<Vec<RegisteredUiTree>, DescriptorError> {
    if !obj.contains_key("uiTrees").map_err(js_err)? {
        return Ok(Vec::new());
    }
    let raw: JsValue = obj.get("uiTrees").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(Vec::new());
    }
    let Some(arr) = raw.as_array() else {
        log::warn!(
            "[Scripting] {scope}: `uiTrees` must be an array of registered trees; ignoring the field"
        );
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(arr.len());
    for i in 0..arr.len() {
        let item: JsValue = arr.get(i).map_err(js_err)?;
        match registered_ui_tree_from_js(ctx, item) {
            Ok(tree) => out.push(tree),
            Err(e) => {
                log::warn!("[Scripting] {scope}: `uiTrees[{i}]` is malformed and was skipped: {e}")
            }
        }
    }
    Ok(out)
}

/// Parse a single registered-tree entry (`{ name, tree, alwaysOn? }`) from JS.
/// The `tree` field is converted via the G1a `anchored_tree_from_js_value`
/// bridge. Returns a named [`DescriptorError`] (never panics) on malformed input.
pub fn registered_ui_tree_from_js<'js>(
    ctx: &Ctx<'js>,
    value: JsValue<'js>,
) -> Result<RegisteredUiTree, DescriptorError> {
    let obj = Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
        reason: "registered UI tree must be an object".to_string(),
    })?;
    let name = get_required_string_js(&obj, "name")?;
    if !obj.contains_key("tree").map_err(js_err)? {
        return Err(DescriptorError::MissingField { field: "tree" });
    }
    let tree_val: JsValue = obj.get("tree").map_err(js_err)?;
    let tree = anchored_tree_from_js_value(ctx, tree_val)?;
    let always_on = get_optional_bool_js(&obj, "alwaysOn")?.unwrap_or(false);
    Ok(RegisteredUiTree {
        name,
        tree,
        always_on,
    })
}

/// Drain the optional `theme` token maps from a QuickJS manifest object. A
/// malformed `theme` field is logged and degraded to default (empty) tokens.
pub fn drain_theme_js<'js>(
    obj: &Object<'js>,
    scope: &str,
) -> Result<ModThemeTokens, DescriptorError> {
    if !obj.contains_key("theme").map_err(js_err)? {
        return Ok(ModThemeTokens::default());
    }
    let raw: JsValue = obj.get("theme").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(ModThemeTokens::default());
    }
    let Ok(theme_obj) = Object::from_value(raw) else {
        log::warn!("[Scripting] {scope}: `theme` must be an object; ignoring the field");
        return Ok(ModThemeTokens::default());
    };
    let colors = match theme_obj.contains_key("colors").map_err(js_err)? {
        true => f32_array4_map_from_js(&theme_obj, "colors")?,
        false => HashMap::new(),
    };
    let fonts = match theme_obj.contains_key("fonts").map_err(js_err)? {
        true => string_map_from_js(&theme_obj, "fonts")?,
        false => HashMap::new(),
    };
    let spacing = match theme_obj.contains_key("spacing").map_err(js_err)? {
        true => f32_map_from_js(&theme_obj, "spacing")?,
        false => HashMap::new(),
    };
    Ok(ModThemeTokens {
        colors,
        fonts,
        spacing,
    })
}

/// Drain the optional mod frontend declaration from a QuickJS manifest object.
/// Missing/null normalizes to `None`; a present malformed object is fatal so a
/// bad frontend cannot partially replace the committed app-side snapshot.
pub fn drain_frontend_js<'js>(
    obj: &Object<'js>,
    _scope: &str,
) -> Result<Option<Frontend>, DescriptorError> {
    if !obj.contains_key("frontend").map_err(js_err)? {
        return Ok(None);
    }
    let raw: JsValue = obj.get("frontend").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    let frontend_obj = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
        reason: "`frontend` must be an object".to_string(),
    })?;
    Ok(Some(frontend_from_js(&frontend_obj)?))
}

pub fn frontend_from_js<'js>(obj: &Object<'js>) -> Result<Frontend, DescriptorError> {
    let menu_tree = get_required_string_js(obj, "menuTree")?;
    let background_level = get_optional_string_js(obj, "backgroundLevel")?;
    let camera = menu_camera_from_js(obj)?;
    Ok(Frontend {
        menu_tree,
        background_level,
        camera,
    })
}

pub fn menu_camera_from_js<'js>(obj: &Object<'js>) -> Result<MenuCamera, DescriptorError> {
    if !obj.contains_key("camera").map_err(js_err)? {
        return Err(DescriptorError::MissingField { field: "camera" });
    }
    let raw: JsValue = obj.get("camera").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Err(DescriptorError::MissingField { field: "camera" });
    }
    let camera_obj = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
        reason: "`frontend.camera` must be an object".to_string(),
    })?;
    if !camera_obj.contains_key("position").map_err(js_err)? {
        return Err(DescriptorError::MissingField { field: "position" });
    }
    let raw_position: JsValue = camera_obj.get("position").map_err(js_err)?;
    let position = validate_finite_array3(
        read_f32_array_n_js::<3>(&raw_position, "frontend.camera.position")?,
        "frontend.camera.position",
    )?;
    Ok(MenuCamera {
        position,
        yaw: validate_finite_f32(
            get_required_f32_js(&camera_obj, "yaw")?,
            "frontend.camera.yaw",
        )?,
        pitch: validate_finite_f32(
            get_required_f32_js(&camera_obj, "pitch")?,
            "frontend.camera.pitch",
        )?,
    })
}

/// Drain the optional `fonts` (family → TTF path) map from a QuickJS manifest
/// object. A malformed `fonts` field is logged and degraded to empty.
pub fn drain_fonts_js<'js>(
    obj: &Object<'js>,
    scope: &str,
) -> Result<ModFontAssets, DescriptorError> {
    if !obj.contains_key("fonts").map_err(js_err)? {
        return Ok(ModFontAssets::default());
    }
    let raw: JsValue = obj.get("fonts").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(ModFontAssets::default());
    }
    if raw.as_object().is_none() {
        log::warn!("[Scripting] {scope}: `fonts` must be a family→path object; ignoring the field");
        return Ok(ModFontAssets::default());
    }
    Ok(ModFontAssets {
        families: string_map_from_js(obj, "fonts")?,
    })
}

/// Drain the optional mod map catalog from a QuickJS manifest object. Malformed
/// entries, duplicate ids, entries with empty ids, and entries with invalid
/// paths are logged and skipped.
pub fn drain_maps_js<'js>(
    obj: &Object<'js>,
    scope: &str,
) -> Result<Vec<ModMapEntry>, DescriptorError> {
    if !obj.contains_key("maps").map_err(js_err)? {
        return Ok(Vec::new());
    }
    let raw: JsValue = obj.get("maps").map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(Vec::new());
    }
    let Some(arr) = raw.as_array() else {
        log::warn!(
            "[Scripting] {scope}: `maps` must be an array of map catalog entries; ignoring the field"
        );
        return Ok(Vec::new());
    };

    let mut out = Vec::with_capacity(arr.len());
    let mut seen_ids = BTreeSet::new();
    for i in 0..arr.len() {
        let item: JsValue = arr.get(i).map_err(js_err)?;
        match mod_map_entry_from_js(item) {
            Ok(entry) => push_valid_map_entry(entry, &mut seen_ids, &mut out, scope, i),
            Err(e) => {
                log::warn!("[Scripting] {scope}: `maps[{i}]` is malformed and was skipped: {e}")
            }
        }
    }
    Ok(out)
}

/// Drain mod-global reaction definitions from a QuickJS manifest object.
/// Missing/null `reactions` normalizes to empty; present entries use the same
/// descriptor parser as level-local reactions plus an optional `levels` scope.
pub fn drain_global_reactions_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    scope: &str,
) -> Result<Vec<ScopedReaction>, DescriptorError> {
    let Some(arr) = optional_manifest_array_js(obj, "reactions", scope)? else {
        return Ok(Vec::new());
    };

    let mut out = Vec::with_capacity(arr.len());
    for i in 0..arr.len() {
        let item: JsValue = arr.get(i).map_err(js_err)?;
        let item_obj =
            Object::from_value(item.clone()).map_err(|_| DescriptorError::InvalidShape {
                reason: "reaction entry must be an object".to_string(),
            })?;
        out.push(ScopedReaction {
            reaction: named_reaction_from_js(ctx, item)?,
            levels: string_array_from_js(&item_obj, "levels")?,
        });
    }
    Ok(out)
}

/// Drain mod-global crossing definitions from a QuickJS manifest object.
/// Missing/null `crossings` normalizes to empty; present entries use the same
/// descriptor parser as level-local crossings plus an optional `levels` scope.
pub fn drain_global_crossings_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    scope: &str,
) -> Result<Vec<ScopedCrossing>, DescriptorError> {
    let Some(arr) = optional_manifest_array_js(obj, "crossings", scope)? else {
        return Ok(Vec::new());
    };

    let mut out = Vec::with_capacity(arr.len());
    for i in 0..arr.len() {
        let item: JsValue = arr.get(i).map_err(js_err)?;
        let item_obj =
            Object::from_value(item.clone()).map_err(|_| DescriptorError::InvalidShape {
                reason: "crossing entry must be an object".to_string(),
            })?;
        out.push(ScopedCrossing {
            crossing: crossing_descriptor_from_js(ctx, &item)?,
            levels: string_array_from_js(&item_obj, "levels")?,
        });
    }
    Ok(out)
}

pub fn optional_manifest_array_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
    scope: &str,
) -> Result<Option<Array<'js>>, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Ok(None);
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    let Some(arr) = raw.as_array() else {
        return Err(DescriptorError::InvalidShape {
            reason: format!("{scope}: `{field}` must be an array"),
        });
    };
    Ok(Some(arr.clone()))
}

pub fn mod_map_entry_from_js<'js>(value: JsValue<'js>) -> Result<ModMapEntry, DescriptorError> {
    let obj = Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
        reason: "map catalog entry must be an object".to_string(),
    })?;
    Ok(ModMapEntry {
        id: get_required_string_js(&obj, "id")?,
        path: get_required_string_js(&obj, "path")?,
        name: get_required_string_js(&obj, "name")?,
        tags: string_array_from_js(&obj, "tags")?,
    })
}

pub fn push_valid_map_entry(
    entry: ModMapEntry,
    seen_ids: &mut BTreeSet<String>,
    out: &mut Vec<ModMapEntry>,
    scope: &str,
    index: usize,
) {
    if entry.id.is_empty() {
        log::warn!("[Scripting] {scope}: `maps[{index}]` has an empty `id` and was skipped");
        return;
    }
    if entry.path.is_empty() {
        log::warn!("[Scripting] {scope}: `maps[{index}]` has an empty `path` and was skipped");
        return;
    }
    if !is_catalog_path_relative_to_content_root(&entry.path) {
        log::warn!(
            "[Scripting] {scope}: `maps[{index}]` path `{}` escapes the content root and was skipped",
            entry.path,
        );
        return;
    }
    if !seen_ids.insert(entry.id.clone()) {
        log::warn!(
            "[Scripting] {scope}: duplicate map catalog id `{}` at `maps[{index}]`; keeping the first entry",
            entry.id,
        );
        return;
    }
    out.push(entry);
}

/// Read an object-valued field as a `String → String` map. Absent/non-object →
/// empty (with a `log::warn!` when present but not an object). Malformed tokens
/// are logged and skipped (per-token degraded) so a single bad entry does not
/// abort the whole theme drain — mirrors the Luau twin.
pub fn string_map_from_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<HashMap<String, String>, DescriptorError> {
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    let map = match Object::from_value(raw) {
        Ok(o) => o,
        Err(_) => {
            log::warn!("[Scripting] theme `{field}` must be an object; skipping field");
            return Ok(HashMap::new());
        }
    };
    let mut out = HashMap::new();
    for entry in map.props::<String, JsValue>() {
        let (key, value) = entry.map_err(js_err)?;
        match String::from_js_value_required(value, field) {
            Ok(s) => {
                out.insert(key, s);
            }
            Err(e) => {
                log::warn!("[Scripting] theme `{field}.{key}` is malformed and was skipped: {e}");
            }
        }
    }
    Ok(out)
}

/// Read an object-valued field as a `String → f32` map. Absent/non-object →
/// empty (with a `log::warn!` when present but not an object). Malformed tokens
/// are logged and skipped (per-token degraded) so a single bad entry does not
/// abort the whole theme drain — mirrors the Luau twin.
pub fn f32_map_from_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<HashMap<String, f32>, DescriptorError> {
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    let map = match Object::from_value(raw) {
        Ok(o) => o,
        Err(_) => {
            log::warn!("[Scripting] theme `{field}` must be an object; skipping field");
            return Ok(HashMap::new());
        }
    };
    let mut out = HashMap::new();
    for entry in map.props::<String, JsValue>() {
        let (key, value) = entry.map_err(js_err)?;
        match js_value_as_f32(&value, field) {
            Ok(f) => {
                out.insert(key, f);
            }
            Err(e) => {
                log::warn!("[Scripting] theme `{field}.{key}` is malformed and was skipped: {e}");
            }
        }
    }
    Ok(out)
}

/// Read an object-valued field as a `String → [f32; 4]` map (linear-RGBA color
/// tokens). Absent/non-object → empty (with a `log::warn!` when present but not
/// an object). Malformed tokens are logged and skipped (per-token degraded) so
/// a single bad entry does not abort the whole theme drain — mirrors the Luau twin.
pub fn f32_array4_map_from_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<HashMap<String, [f32; 4]>, DescriptorError> {
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    let map = match Object::from_value(raw) {
        Ok(o) => o,
        Err(_) => {
            log::warn!("[Scripting] theme `{field}` must be an object; skipping field");
            return Ok(HashMap::new());
        }
    };
    let mut out = HashMap::new();
    for entry in map.props::<String, JsValue>() {
        let (key, value) = entry.map_err(js_err)?;
        match read_f32_array_n_js::<4>(&value, field) {
            Ok(arr) => {
                out.insert(key, arr);
            }
            Err(e) => {
                log::warn!("[Scripting] theme `{field}.{key}` is malformed and was skipped: {e}");
            }
        }
    }
    Ok(out)
}
