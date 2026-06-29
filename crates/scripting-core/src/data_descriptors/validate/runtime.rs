// Runtime-side descriptor validators and parsers for scripting-core. These
// translate VM values into scripting-core UI descriptor types.
// See: context/lib/scripting.md §12 (Crate Architecture)

use std::collections::BTreeSet;

use mlua::{Table, Value as LuaValue};

use crate::ui::descriptor::{Align, CaptureMode, Easing, FocusKind, Priority, Role};
use crate::ui::layout::Anchor;

use super::super::{DescriptorError, lua_err};

pub fn validate_dense_lua_array(table: &Table, field_name: &str) -> Result<usize, DescriptorError> {
    let mut keys = BTreeSet::new();
    let mut max_index = 0_i64;

    for pair in table.clone().pairs::<LuaValue, LuaValue>() {
        let (key, _) = pair.map_err(lua_err)?;
        let LuaValue::Integer(index) = key else {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "{field_name} must be a dense array; found {} key",
                    key.type_name()
                ),
            });
        };
        if index < 1 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "{field_name} must be a dense array; index {index} is out of range"
                ),
            });
        }
        keys.insert(index);
        max_index = max_index.max(index);
    }

    if keys.len() != max_index as usize {
        return Err(DescriptorError::InvalidShape {
            reason: format!("{field_name} must be a dense array; holes are not allowed"),
        });
    }

    Ok(max_index as usize)
}

pub fn parse_anchor(s: &str) -> Result<Anchor, DescriptorError> {
    Ok(match s {
        "topLeft" => Anchor::TopLeft,
        "top" => Anchor::Top,
        "topRight" => Anchor::TopRight,
        "left" => Anchor::Left,
        "center" => Anchor::Center,
        "right" => Anchor::Right,
        "bottomLeft" => Anchor::BottomLeft,
        "bottom" => Anchor::Bottom,
        "bottomRight" => Anchor::BottomRight,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`anchor` must be a placement anchor, got \"{other}\""),
            });
        }
    })
}

pub fn parse_align(s: &str) -> Result<Align, DescriptorError> {
    Ok(match s {
        "start" => Align::Start,
        "center" => Align::Center,
        "end" => Align::End,
        "stretch" => Align::Stretch,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`align` must be \"start\"|\"center\"|\"end\"|\"stretch\", got \"{other}\""
                ),
            });
        }
    })
}

pub fn parse_capture_mode(s: &str) -> Result<CaptureMode, DescriptorError> {
    Ok(match s {
        "capture" => CaptureMode::Capture,
        "passthrough" => CaptureMode::Passthrough,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`captureMode` must be \"capture\"|\"passthrough\", got \"{other}\""
                ),
            });
        }
    })
}

pub fn parse_easing(s: &str) -> Result<Easing, DescriptorError> {
    Ok(match s {
        "linear" => Easing::Linear,
        "easeIn" => Easing::EaseIn,
        "easeOut" => Easing::EaseOut,
        "easeInOut" => Easing::EaseInOut,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`easing` must be \"linear\"|\"easeIn\"|\"easeOut\"|\"easeInOut\", got \"{other}\""
                ),
            });
        }
    })
}

pub fn parse_focus_kind(s: &str) -> Result<FocusKind, DescriptorError> {
    Ok(match s {
        "linear" => FocusKind::Linear,
        "spatial" => FocusKind::Spatial,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`focus.policy` must be \"linear\"|\"spatial\", got \"{other}\""),
            });
        }
    })
}

pub fn parse_role(s: &str) -> Result<Role, DescriptorError> {
    Ok(match s {
        "tab" => Role::Tab,
        "tablist" => Role::Tablist,
        "checkbox" => Role::Checkbox,
        "radio" => Role::Radio,
        "listitem" => Role::Listitem,
        "button" => Role::Button,
        "slider" => Role::Slider,
        "progressbar" => Role::Progressbar,
        "image" => Role::Image,
        "group" => Role::Group,
        "none" => Role::None,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("unknown widget `role` \"{other}\""),
            });
        }
    })
}

pub fn parse_priority(s: &str) -> Result<Priority, DescriptorError> {
    Ok(match s {
        "polite" => Priority::Polite,
        "assertive" => Priority::Assertive,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`announce.priority` must be \"polite\"|\"assertive\", got \"{other}\""
                ),
            });
        }
    })
}
