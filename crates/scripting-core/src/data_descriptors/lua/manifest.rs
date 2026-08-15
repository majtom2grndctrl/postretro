// Data-context descriptors: Lua manifest drains (uiTrees/theme/fonts/maps).
// See: context/lib/scripting.md

use super::super::*;

impl LevelManifest {
    /// Deserialize a top-level table returned from a Luau `setupLevel()` call.
    /// Its `reactions`, `events`, `crossings`, `triggerEvents`, `triggerPools`, and `uiTrees` arrays are optional.
    pub fn from_lua_value(value: LuaValue) -> Result<Self, DescriptorError> {
        let table = match value {
            LuaValue::Table(t) => t,
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("setupLevel must return a table, got {}", other.type_name()),
                });
            }
        };

        let reactions = if table.contains_key("reactions").map_err(lua_err)? {
            let arr: Table = table.get("reactions").map_err(lua_err)?;
            let len = validate_dense_lua_array(&arr, "`reactions` field")?;
            let mut out = Vec::with_capacity(len);
            for i in 1..=(len as i64) {
                let item: LuaValue = arr.get(i).map_err(lua_err)?;
                let is_resource_grant = is_resource_grant_reaction_lua(&item);
                match named_reaction_from_lua(item) {
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

        let crossings = if table.contains_key("crossings").map_err(lua_err)? {
            let arr: Table = table.get("crossings").map_err(lua_err)?;
            let len = validate_dense_lua_array(&arr, "`crossings` field")?;
            let mut out = Vec::with_capacity(len);
            for i in 1..=(len as i64) {
                let item: LuaValue = arr.get(i).map_err(lua_err)?;
                out.push(crossing_descriptor_from_lua(item)?);
            }
            out
        } else {
            Vec::new()
        };
        let events = drain_impact_events_lua(&table, "setupLevel")?;
        let trigger_events = drain_trigger_events_lua(&table, "setupLevel")?;
        let trigger_pools = drain_trigger_pools_lua(&table, "setupLevel")?;

        let ui_trees = drain_ui_trees_lua(&table, "setupLevel")?;

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

/// Luau twin of [`is_resource_grant_reaction_js`]. Resource-grant descriptor
/// errors reject setup rather than degrading one reaction entry.
fn is_resource_grant_reaction_lua(value: &LuaValue) -> bool {
    let LuaValue::Table(table) = value else {
        return false;
    };
    let Ok(primitive) = table.get::<LuaValue>("primitive") else {
        return false;
    };
    let LuaValue::String(primitive) = primitive else {
        return false;
    };
    matches!(
        primitive.to_str().ok().as_deref(),
        Some("grantHealth" | "grantAmmo" | "addSlot")
    )
}

/// Drain the strict mod-global switching declaration. Mirrors
/// [`drain_switching_js`] field-for-field: an invalid policy rejects the
/// complete mod-init attempt because it changes simulation authorization.
pub fn drain_switching_lua(
    table: &Table,
    scope: &str,
) -> Result<SwitchingDescriptor, DescriptorError> {
    let raw: LuaValue = table.get("switching").map_err(lua_err)?;
    let switching = match raw {
        LuaValue::Nil => return Ok(SwitchingDescriptor::default()),
        LuaValue::Table(switching) => switching,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "{scope}: `switching` must be a table, got {}",
                    other.type_name()
                ),
            });
        }
    };
    let commit_on_direct_select = get_required_bool_lua(&switching, "commitOnDirectSelect")
        .map_err(|error| switching_field_error(scope, "commitOnDirectSelect", error))?;
    let cycle_commit_dwell_ms = get_required_f32_lua(&switching, "cycleCommitDwellMs")
        .map_err(|error| switching_field_error(scope, "cycleCommitDwellMs", error))?;
    let block_during_reload = get_required_bool_lua(&switching, "blockDuringReload")
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

/// Drain the optional static renderer profile from a Luau mod manifest.
/// Mirrors [`drain_render_profile_js`] field-for-field.
pub fn drain_render_profile_lua(
    table: &Table,
    scope: &str,
) -> Result<ModRenderProfile, DescriptorError> {
    let raw_render: LuaValue = table.get("render").map_err(lua_err)?;
    let render = match raw_render {
        LuaValue::Nil => return Ok(ModRenderProfile::default()),
        LuaValue::Table(render) => render,
        _ => {
            log::warn!(
                "[Scripting] {scope}: `render` must be a table; using the default render profile"
            );
            return Ok(ModRenderProfile::default());
        }
    };

    let raw_bloom: LuaValue = render.get("bloom").map_err(lua_err)?;
    let bloom = match raw_bloom {
        LuaValue::Nil => return Ok(ModRenderProfile::default()),
        LuaValue::Table(bloom) => bloom,
        _ => {
            log::warn!(
                "[Scripting] {scope}: `render.bloom` must be a table; using the default bloom profile"
            );
            return Ok(ModRenderProfile::default());
        }
    };

    let resolution = match bloom.get::<LuaValue>("resolution").map_err(lua_err)? {
        LuaValue::Nil => ModBloomResolution::default(),
        LuaValue::String(value) => match value.to_str().ok().as_deref() {
            Some("half") => ModBloomResolution::Half,
            Some("quarter") => ModBloomResolution::Quarter,
            Some("eighth") => ModBloomResolution::Eighth,
            _ => {
                log::warn!(
                    "[Scripting] {scope}: `render.bloom.resolution` must be `half`, `quarter`, or `eighth`; using `half`"
                );
                ModBloomResolution::default()
            }
        },
        _ => {
            log::warn!(
                "[Scripting] {scope}: `render.bloom.resolution` must be `half`, `quarter`, or `eighth`; using `half`"
            );
            ModBloomResolution::default()
        }
    };

    let pixelated = match bloom.get::<LuaValue>("pixelated").map_err(lua_err)? {
        LuaValue::Nil => false,
        LuaValue::Boolean(value) => value,
        _ => {
            log::warn!(
                "[Scripting] {scope}: `render.bloom.pixelated` must be a boolean; using `false`"
            );
            false
        }
    };

    Ok(ModRenderProfile {
        bloom: ModBloomProfile {
            resolution,
            pixelated,
        },
    })
}

/// Luau twin of the QuickJS mover-default drain.
pub fn drain_mover_defaults_lua(
    table: &Table,
    scope: &str,
) -> Result<ModMoverDefaults, DescriptorError> {
    let raw_movers: LuaValue = table.get("movers").map_err(lua_err)?;
    let movers = match raw_movers {
        LuaValue::Nil => return Ok(ModMoverDefaults::default()),
        LuaValue::Table(movers) => movers,
        _ => {
            log::warn!(
                "[Scripting] {scope}: `movers` must be a table; using default mover settings"
            );
            return Ok(ModMoverDefaults::default());
        }
    };
    let value = match movers.get::<LuaValue>("autoCloseMs").map_err(lua_err)? {
        LuaValue::Nil => return Ok(ModMoverDefaults::default()),
        LuaValue::Integer(value) => value as f64,
        LuaValue::Number(value) => value,
        _ => {
            log::warn!(
                "[Scripting] {scope}: `movers.autoCloseMs` must be a finite non-negative number; using 0"
            );
            return Ok(ModMoverDefaults::default());
        }
    };
    if !value.is_finite() || value < 0.0 || !(value as f32).is_finite() {
        log::warn!(
            "[Scripting] {scope}: `movers.autoCloseMs` must be a finite non-negative number; using 0"
        );
        return Ok(ModMoverDefaults::default());
    }
    Ok(ModMoverDefaults {
        auto_close_ms: value as f32,
    })
}

/// Drain pure SDK `defineImpactEvent` handles from a manifest. The event
/// remains opaque policy data here; Task 5 owns validation, merging, and
/// execution.
pub fn drain_impact_events_lua(
    table: &Table,
    scope: &str,
) -> Result<Vec<ImpactEventDescriptor>, DescriptorError> {
    let Some(arr) = optional_manifest_array_lua(table, "events", scope)? else {
        return Ok(Vec::new());
    };
    let mut len = 0usize;
    for pair in arr.clone().pairs::<LuaValue, LuaValue>() {
        let (key, _) = pair.map_err(lua_err)?;
        if let LuaValue::Integer(index) = key
            && index >= 1
        {
            len = len.max(index as usize);
        }
    }
    if len > MAX_IMPACT_EVENT_CONTAINER_ENTRIES {
        log::warn!(
            "[Scripting] {scope}: `events` exceeds {MAX_IMPACT_EVENT_CONTAINER_ENTRIES} array slots; ignoring the field"
        );
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(len);
    for i in 1..=(len as i64) {
        let value: LuaValue = match arr.get(i) {
            Ok(value) => value,
            Err(error) => {
                log::warn!(
                    "[Scripting] {scope}: events[{i}] could not be read and was skipped: {}",
                    lua_err(error)
                );
                continue;
            }
        };
        match impact_event_from_lua(value) {
            Ok(descriptor) => out.push(descriptor),
            Err(error) => {
                log::warn!("[Scripting] {scope}: events[{i}] is malformed and was skipped: {error}")
            }
        }
    }
    Ok(out)
}

fn impact_event_from_lua(value: LuaValue) -> Result<ImpactEventDescriptor, DescriptorError> {
    let item = lua_table(value, "impact-event entry")?;
    let kind = get_required_string_lua(&item, "kind")?;
    if kind != "impact" {
        return Err(DescriptorError::InvalidShape {
            reason: "impact-event entry `kind` must be `impact`".into(),
        });
    }

    let filter: Table = item
        .get("filter")
        .map_err(|_| DescriptorError::MissingField { field: "filter" })?;
    let filter_tag = get_optional_string_lua(&filter, "tag")?;

    let policy: Table = item
        .get("policy")
        .map_err(|_| DescriptorError::InvalidShape {
            reason: "impact-event entry `policy` must be an array".into(),
        })?;
    let len = validate_dense_lua_array(&policy, "impact-event entry `policy`")?;
    let mut policy_json = Vec::with_capacity(len);
    for i in 1..=(len as i64) {
        let raw: LuaValue = policy.get(i).map_err(lua_err)?;
        policy_json.push(impact_policy_entry_from_lua(raw)?);
    }

    let is_override = get_optional_bool_lua(&item, "isOverride")?.unwrap_or(false);
    if is_override && filter_tag.is_none() {
        return Err(DescriptorError::InvalidShape {
            reason: "impact-event override `filter.tag` is required".into(),
        });
    }

    Ok(ImpactEventDescriptor {
        id: validate_impact_event_id(get_required_string_lua(&item, "id")?)?,
        is_override,
        levels: string_array_from_lua(&item, "levels")?,
        filter_tag,
        policy: policy_json,
    })
}

fn impact_policy_entry_from_lua(raw: LuaValue) -> Result<serde_json::Value, DescriptorError> {
    let LuaValue::Table(item) = &raw else {
        return conv::lua_to_json(raw).map_err(lua_err);
    };
    if !item.contains_key("do").map_err(lua_err)? {
        return conv::lua_to_json(raw).map_err(lua_err);
    }

    let effects: Table = item.get("do").map_err(|_| DescriptorError::InvalidShape {
        reason: "impact policy group `do` must be an array".into(),
    })?;
    let len = validate_dense_lua_array(&effects, "impact policy group `do`")?;
    let mut json = conv::lua_to_json(raw).map_err(lua_err)?;
    let json_object = json
        .as_object_mut()
        .ok_or_else(|| DescriptorError::InvalidShape {
            reason: "impact policy group must be an object".into(),
        })?;
    let mut lowered = Vec::with_capacity(len);
    for i in 1..=(len as i64) {
        let effect: LuaValue = effects.get(i).map_err(lua_err)?;
        lowered.push(conv::lua_to_json(effect).map_err(lua_err)?);
    }
    json_object.insert("do".into(), serde_json::Value::Array(lowered));
    Ok(json)
}

/// Drain the `triggerEvents` array from a Luau manifest table. Mirrors
/// [`drain_trigger_events_js`]: a malformed entry (non-table, or missing/invalid
/// `tag`/`fire`/`levels`) is logged and skipped rather than aborting the whole
/// manifest; an unknown `event` value is likewise logged and skipped.
pub fn drain_trigger_events_lua(
    table: &Table,
    scope: &str,
) -> Result<Vec<TriggerEventDescriptor>, DescriptorError> {
    let Some(arr) = optional_manifest_array_lua(table, "triggerEvents", scope)? else {
        return Ok(Vec::new());
    };
    let len = validate_dense_lua_array(&arr, "`triggerEvents` field")?;
    let mut out = Vec::with_capacity(len);
    for i in 1..=(len as i64) {
        let item: LuaValue = arr.get(i).map_err(lua_err)?;
        match trigger_event_from_lua(item, i, scope) {
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
/// Luau. `Ok(None)` means the entry parsed but its `event` is unrecognized (the
/// caller has already logged the reason); a genuinely malformed entry returns
/// `Err` for the caller to log and skip.
fn trigger_event_from_lua(
    value: LuaValue,
    i: i64,
    scope: &str,
) -> Result<Option<TriggerEventDescriptor>, DescriptorError> {
    let item = lua_table(value, "trigger-event entry")?;
    let event = get_required_string_lua(&item, "event")?;
    if !matches!(event.as_str(), "enter" | "exit") {
        log::warn!(
            "[Scripting] {scope}: triggerEvents[{i}] has unknown event `{event}` and was skipped"
        );
        return Ok(None);
    }
    Ok(Some(TriggerEventDescriptor {
        tag: get_required_string_lua(&item, "tag")?,
        event,
        fire: string_array_from_lua(&item, "fire")?,
        levels: string_array_from_lua(&item, "levels")?,
    }))
}

/// Drain the `triggerPools` array from a Luau manifest table. A malformed
/// entry is logged and skipped so one bad pool does not abort the manifest.
/// Pool tags are unique within this drain; a later duplicate is skipped.
pub fn drain_trigger_pools_lua(
    table: &Table,
    scope: &str,
) -> Result<Vec<TriggerPoolDescriptor>, DescriptorError> {
    let Some(arr) = optional_trigger_pool_array_lua(table, scope)? else {
        return Ok(Vec::new());
    };
    let entries = trigger_pool_entries_lua(&arr, scope)?;
    let mut out = Vec::with_capacity(entries.len());
    let mut seen_tags = BTreeSet::new();
    for (i, value) in entries {
        match trigger_pool_from_lua(value) {
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

fn optional_trigger_pool_array_lua(
    table: &Table,
    scope: &str,
) -> Result<Option<Table>, DescriptorError> {
    let raw: LuaValue = match table.get("triggerPools") {
        Ok(raw) => raw,
        Err(error) => {
            let error = lua_err(error);
            log::warn!(
                "[Scripting] {scope}: could not access `triggerPools`; ignoring the field: {error}"
            );
            return Ok(None);
        }
    };
    match raw {
        LuaValue::Nil => Ok(None),
        LuaValue::Table(table) => Ok(Some(table)),
        other => {
            log::warn!(
                "[Scripting] {scope}: `triggerPools` must be an array, got {}; ignoring the field",
                other.type_name()
            );
            Ok(None)
        }
    }
}

/// Retain sparse positive integer slots within the authoring limit. Any slot
/// above that limit makes the whole container malformed in both runtimes.
fn trigger_pool_entries_lua(
    arr: &Table,
    scope: &str,
) -> Result<Vec<(u64, LuaValue)>, DescriptorError> {
    let mut entries = Vec::new();
    let mut saw_property = false;
    for pair in arr.clone().pairs::<LuaValue, LuaValue>() {
        saw_property = true;
        let (key, value) = pair.map_err(lua_err)?;
        match key {
            LuaValue::Integer(index) if index >= 1 => {
                if (index as u64) > MAX_TRIGGER_POOL_CONTAINER_ENTRIES as u64 {
                    log::warn!(
                        "[Scripting] {scope}: `triggerPools` exceeds the {}-slot limit; ignoring the field",
                        MAX_TRIGGER_POOL_CONTAINER_ENTRIES,
                    );
                    return Ok(Vec::new());
                }
                entries.push((index as u64, value));
            }
            LuaValue::Number(index)
                if index.is_finite() && index >= 1.0 && index.fract() == 0.0 =>
            {
                if index > MAX_TRIGGER_POOL_CONTAINER_ENTRIES as f64 {
                    log::warn!(
                        "[Scripting] {scope}: `triggerPools` exceeds the {}-slot limit; ignoring the field",
                        MAX_TRIGGER_POOL_CONTAINER_ENTRIES,
                    );
                    return Ok(Vec::new());
                }
                entries.push((index as u64, value));
            }
            LuaValue::Integer(index) => log::warn!(
                "[Scripting] {scope}: `triggerPools` index {index} is out of range and was skipped"
            ),
            LuaValue::Number(index) => log::warn!(
                "[Scripting] {scope}: `triggerPools` index {index} is not a positive integer and was skipped"
            ),
            other => log::warn!(
                "[Scripting] {scope}: `triggerPools` entry with {} key was skipped",
                other.type_name()
            ),
        }
    }
    if saw_property && entries.is_empty() {
        log::warn!("[Scripting] {scope}: `triggerPools` must be an array; ignoring the field");
    }
    entries.sort_by_key(|(index, _)| *index);
    Ok(entries)
}

fn trigger_pool_from_lua(value: LuaValue) -> Result<TriggerPoolDescriptor, DescriptorError> {
    let item = lua_table(value, "trigger-pool entry")?;
    let tag = get_required_string_lua(&item, "tag")?;
    if tag.is_empty() {
        return Err(DescriptorError::InvalidShape {
            reason: "trigger-pool `tag` must not be empty".into(),
        });
    }

    let has_arm = item.contains_key("arm").map_err(lua_err)?;
    let has_percentage = item.contains_key("armPercentage").map_err(lua_err)?;
    if has_arm == has_percentage {
        return Err(DescriptorError::InvalidShape {
            reason: "trigger-pool must define exactly one of `arm` or `armPercentage`".into(),
        });
    }

    let arm = if has_arm {
        TriggerPoolArm::Count(get_required_u32_lua(&item, "arm")?)
    } else {
        let raw: LuaValue = item.get("armPercentage").map_err(lua_err)?;
        let percentage = match raw {
            LuaValue::Integer(value) => value as f64,
            LuaValue::Number(value) => value,
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "'armPercentage' must be a number, got {}",
                        other.type_name()
                    ),
                });
            }
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
        levels: string_array_from_lua(&item, "levels")?,
    })
}

/// Drain the `uiTrees` array from a Luau manifest table. Mirrors
/// [`drain_ui_trees_js`]: malformed entries are logged and skipped, a non-table
/// `uiTrees` field is logged and yields empty.
pub fn drain_ui_trees_lua(
    table: &Table,
    scope: &str,
) -> Result<Vec<RegisteredUiTree>, DescriptorError> {
    let raw: LuaValue = table.get("uiTrees").map_err(lua_err)?;
    let arr = match raw {
        LuaValue::Nil => return Ok(Vec::new()),
        LuaValue::Table(t) => t,
        _ => {
            log::warn!(
                "[Scripting] {scope}: `uiTrees` must be an array of registered trees; ignoring the field"
            );
            return Ok(Vec::new());
        }
    };
    let len = dense_lua_prefix_len(&arr, "uiTrees", scope)?;
    let mut out = Vec::with_capacity(len);
    for i in 1..=(len as i64) {
        let item: LuaValue = arr.get(i).map_err(lua_err)?;
        match registered_ui_tree_from_lua(item) {
            Ok(tree) => out.push(tree),
            Err(e) => {
                log::warn!("[Scripting] {scope}: `uiTrees[{i}]` is malformed and was skipped: {e}")
            }
        }
    }
    log_lua_array_extras(&arr, len, "uiTrees", scope)?;
    Ok(out)
}

/// Luau twin of [`drain_presentation_templates_js`]. Invalid entries are
/// contained to the entry so a passive visual cannot make mod init fail.
pub fn drain_presentation_templates_lua(
    table: &Table,
    scope: &str,
) -> Result<Vec<PresentationTemplate>, DescriptorError> {
    let Some(arr) = optional_manifest_array_lua(table, "presentationTemplates", scope)? else {
        return Ok(Vec::new());
    };
    let len = dense_lua_prefix_len(&arr, "presentationTemplates", scope)?;
    let mut seen_ids = BTreeSet::new();
    let mut out = Vec::with_capacity(len);
    for i in 1..=(len as i64) {
        let value: LuaValue = arr.get(i).map_err(lua_err)?;
        match presentation_template_from_lua(value) {
            Ok(template) if !seen_ids.insert(template.id.clone()) => log::warn!(
                "[Scripting] {scope}: `presentationTemplates[{i}]` duplicates `{}` and was skipped",
                template.id
            ),
            Ok(template) => out.push(template),
            Err(error) => log::warn!(
                "[Scripting] {scope}: `presentationTemplates[{i}]` is malformed and was skipped: {error}"
            ),
        }
    }
    log_lua_array_extras(&arr, len, "presentationTemplates", scope)?;
    Ok(out)
}

/// Deserialize one Luau presentation template through the standard VM-free
/// bridge, then validate its authored timing and motion bounds.
pub fn presentation_template_from_lua(
    value: LuaValue,
) -> Result<PresentationTemplate, DescriptorError> {
    let json = conv::lua_to_json(value).map_err(lua_err)?;
    let template = serde_json::from_value::<PresentationTemplate>(json).map_err(|error| {
        DescriptorError::InvalidShape {
            reason: format!("presentation template must match its descriptor shape: {error}"),
        }
    })?;
    template
        .validate()
        .map_err(|reason| DescriptorError::InvalidShape { reason })?;
    Ok(template)
}

/// Parse a single registered-tree entry (`{ name, tree, alwaysOn? }`) from Luau.
/// The `tree` field is converted via the G1a `anchored_tree_from_lua_value`
/// bridge. Returns a named [`DescriptorError`] (never panics) on malformed input.
pub fn registered_ui_tree_from_lua(value: LuaValue) -> Result<RegisteredUiTree, DescriptorError> {
    let table = lua_table(value, "registered UI tree")?;
    let name = get_required_string_lua(&table, "name")?;
    let tree_val: LuaValue = table.get("tree").map_err(lua_err)?;
    if matches!(tree_val, LuaValue::Nil) {
        return Err(DescriptorError::MissingField { field: "tree" });
    }
    let tree = anchored_tree_from_lua_value(tree_val)?;
    let always_on = get_optional_bool_lua(&table, "alwaysOn")?.unwrap_or(false);
    Ok(RegisteredUiTree {
        name,
        tree,
        always_on,
    })
}

/// Drain the optional `theme` token maps from a Luau manifest table. A malformed
/// `theme` field is logged and degraded to default (empty) tokens.
pub fn drain_theme_lua(table: &Table, scope: &str) -> Result<ModThemeTokens, DescriptorError> {
    let raw: LuaValue = table.get("theme").map_err(lua_err)?;
    let theme_table = match raw {
        LuaValue::Nil => return Ok(ModThemeTokens::default()),
        LuaValue::Table(t) => t,
        _ => {
            log::warn!("[Scripting] {scope}: `theme` must be a table; ignoring the field");
            return Ok(ModThemeTokens::default());
        }
    };
    Ok(ModThemeTokens {
        colors: f32_array4_map_from_lua(&theme_table, "colors")?,
        fonts: string_map_from_lua(&theme_table, "fonts")?,
        spacing: f32_map_from_lua(&theme_table, "spacing")?,
    })
}

/// Drain the optional mod frontend declaration from a Luau manifest table.
/// Mirrors [`drain_frontend_js`].
pub fn drain_frontend_lua(
    table: &Table,
    _scope: &str,
) -> Result<Option<Frontend>, DescriptorError> {
    let raw: LuaValue = table.get("frontend").map_err(lua_err)?;
    let frontend_table = match raw {
        LuaValue::Nil => return Ok(None),
        LuaValue::Table(t) => t,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`frontend` must be a table, got {}", other.type_name()),
            });
        }
    };
    Ok(Some(frontend_from_lua(&frontend_table)?))
}

pub fn frontend_from_lua(table: &Table) -> Result<Frontend, DescriptorError> {
    let menu_tree = get_required_string_lua(table, "menuTree")?;
    let background_level = get_optional_string_lua(table, "backgroundLevel")?;
    let camera = menu_camera_from_lua(table)?;
    Ok(Frontend {
        menu_tree,
        background_level,
        camera,
    })
}

pub fn menu_camera_from_lua(table: &Table) -> Result<MenuCamera, DescriptorError> {
    let raw: LuaValue = table.get("camera").map_err(lua_err)?;
    let camera_table = match raw {
        LuaValue::Nil => return Err(DescriptorError::MissingField { field: "camera" }),
        LuaValue::Table(t) => t,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`frontend.camera` must be a table, got {}",
                    other.type_name()
                ),
            });
        }
    };
    let raw_position: LuaValue = camera_table.get("position").map_err(lua_err)?;
    if matches!(raw_position, LuaValue::Nil) {
        return Err(DescriptorError::MissingField { field: "position" });
    }
    let position = validate_finite_array3(
        read_f32_array_n_lua::<3>(raw_position, "frontend.camera.position")?,
        "frontend.camera.position",
    )?;
    Ok(MenuCamera {
        position,
        yaw: validate_finite_f32(
            get_required_f32_lua(&camera_table, "yaw")?,
            "frontend.camera.yaw",
        )?,
        pitch: validate_finite_f32(
            get_required_f32_lua(&camera_table, "pitch")?,
            "frontend.camera.pitch",
        )?,
    })
}

/// Drain the optional `fonts` (family → TTF path) map from a Luau manifest
/// table. A malformed `fonts` field is logged and degraded to empty.
pub fn drain_fonts_lua(table: &Table, scope: &str) -> Result<ModFontAssets, DescriptorError> {
    let raw: LuaValue = table.get("fonts").map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Ok(ModFontAssets::default()),
        LuaValue::Table(_) => Ok(ModFontAssets {
            families: string_map_from_lua(table, "fonts")?,
        }),
        _ => {
            log::warn!(
                "[Scripting] {scope}: `fonts` must be a family→path table; ignoring the field"
            );
            Ok(ModFontAssets::default())
        }
    }
}

/// Drain the optional mod map catalog from a Luau manifest table. Mirrors
/// [`drain_maps_js`].
pub fn drain_maps_lua(table: &Table, scope: &str) -> Result<Vec<ModMapEntry>, DescriptorError> {
    let raw: LuaValue = table.get("maps").map_err(lua_err)?;
    let arr = match raw {
        LuaValue::Nil => return Ok(Vec::new()),
        LuaValue::Table(t) => t,
        _ => {
            log::warn!(
                "[Scripting] {scope}: `maps` must be an array of map catalog entries; ignoring the field"
            );
            return Ok(Vec::new());
        }
    };

    let len = dense_lua_prefix_len(&arr, "maps", scope)?;
    let mut out = Vec::with_capacity(len);
    let mut seen_ids = BTreeSet::new();
    for i in 1..=(len as i64) {
        let item: LuaValue = arr.get(i).map_err(lua_err)?;
        match mod_map_entry_from_lua(item) {
            Ok(entry) => push_valid_map_entry(entry, &mut seen_ids, &mut out, scope, i as usize),
            Err(e) => {
                log::warn!("[Scripting] {scope}: `maps[{i}]` is malformed and was skipped: {e}")
            }
        }
    }
    log_lua_array_extras(&arr, len, "maps", scope)?;
    Ok(out)
}

/// Drain mod-global reaction definitions from a Luau manifest table. Mirrors
/// [`drain_global_reactions_js`].
pub fn drain_global_reactions_lua(
    table: &Table,
    scope: &str,
) -> Result<Vec<ScopedReaction>, DescriptorError> {
    let Some(arr) = optional_manifest_array_lua(table, "reactions", scope)? else {
        return Ok(Vec::new());
    };
    let len = validate_dense_lua_array(&arr, "`reactions` field")?;
    let mut out = Vec::with_capacity(len);
    for i in 1..=(len as i64) {
        let item: LuaValue = arr.get(i).map_err(lua_err)?;
        let item_table = lua_table(item.clone(), "reaction entry")?;
        out.push(ScopedReaction {
            reaction: named_reaction_from_lua(item)?,
            levels: string_array_from_lua(&item_table, "levels")?,
        });
    }
    Ok(out)
}

/// Drain mod-global crossing definitions from a Luau manifest table. Mirrors
/// [`drain_global_crossings_js`].
pub fn drain_global_crossings_lua(
    table: &Table,
    scope: &str,
) -> Result<Vec<ScopedCrossing>, DescriptorError> {
    let Some(arr) = optional_manifest_array_lua(table, "crossings", scope)? else {
        return Ok(Vec::new());
    };
    let len = validate_dense_lua_array(&arr, "`crossings` field")?;
    let mut out = Vec::with_capacity(len);
    for i in 1..=(len as i64) {
        let item: LuaValue = arr.get(i).map_err(lua_err)?;
        let item_table = lua_table(item.clone(), "crossing entry")?;
        out.push(ScopedCrossing {
            crossing: crossing_descriptor_from_lua(item)?,
            levels: string_array_from_lua(&item_table, "levels")?,
        });
    }
    Ok(out)
}

pub fn optional_manifest_array_lua(
    table: &Table,
    field: &'static str,
    scope: &str,
) -> Result<Option<Table>, DescriptorError> {
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Ok(None),
        LuaValue::Table(t) => Ok(Some(t)),
        other => Err(DescriptorError::InvalidShape {
            reason: format!(
                "{scope}: `{field}` must be an array, got {}",
                other.type_name()
            ),
        }),
    }
}

pub fn mod_map_entry_from_lua(value: LuaValue) -> Result<ModMapEntry, DescriptorError> {
    let table = lua_table(value, "map catalog entry")?;
    Ok(ModMapEntry {
        id: get_required_string_lua(&table, "id")?,
        path: get_required_string_lua(&table, "path")?,
        name: get_required_string_lua(&table, "name")?,
        tags: string_array_from_lua(&table, "tags")?,
    })
}

fn dense_lua_prefix_len(
    arr: &Table,
    field: &'static str,
    scope: &str,
) -> Result<usize, DescriptorError> {
    let mut indices = BTreeSet::new();
    for pair in arr.clone().pairs::<LuaValue, LuaValue>() {
        let (key, _) = pair.map_err(lua_err)?;
        if let LuaValue::Integer(index) = key {
            if index >= 1 {
                indices.insert(index);
            }
        }
    }

    let mut len = 0usize;
    while indices.contains(&((len + 1) as i64)) {
        len += 1;
    }

    if len == 0 && !indices.is_empty() {
        log::warn!(
            "[Scripting] {scope}: `{field}` has no dense prefix; non-prefix entries were skipped"
        );
    }

    Ok(len)
}

fn log_lua_array_extras(
    arr: &Table,
    prefix_len: usize,
    field: &'static str,
    scope: &str,
) -> Result<(), DescriptorError> {
    for pair in arr.clone().pairs::<LuaValue, LuaValue>() {
        let (key, _) = pair.map_err(lua_err)?;
        let in_prefix =
            matches!(&key, LuaValue::Integer(index) if *index >= 1 && *index <= prefix_len as i64);
        if !in_prefix {
            log::warn!(
                "[Scripting] {scope}: `{field}` entry with {} key was skipped because `{field}` must be a dense array",
                key.type_name()
            );
        }
    }
    Ok(())
}
