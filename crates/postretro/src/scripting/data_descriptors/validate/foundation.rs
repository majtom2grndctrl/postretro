// Foundation-pure descriptor validators: numeric, path, and IR checks only.
// See: context/lib/scripting.md §12 (Crate Architecture)

use std::path::{Component, Path};

use crate::movement::MovementScope;
use crate::scripting::ir::{BakedIr, CURRENT_IR_VERSION, IrNode, IrType, bind};

use super::super::DescriptorError;

pub(crate) fn validate_at(value: f32) -> Result<f32, DescriptorError> {
    if !(0.0..=1.0).contains(&value) {
        return Err(DescriptorError::AtThresholdOutOfRange { value });
    }
    Ok(value)
}

pub(crate) fn validate_primitive_name(name: String) -> Result<String, DescriptorError> {
    if name.is_empty() {
        return Err(DescriptorError::EmptyPrimitiveName);
    }
    Ok(name)
}

/// Validate a dash expression node at declaration: wrap it in a read-only
/// [`BakedIr`] envelope and `bind` it against [`MovementScope::for_validation`],
/// then require the bound program's root type to match the field's expected
/// type.
pub(crate) fn validate_dash_expr(
    node: IrNode,
    expected: IrType,
    field: &str,
) -> Result<IrNode, DescriptorError> {
    let baked = BakedIr {
        version: CURRENT_IR_VERSION,
        output: None,
        root: node,
    };
    let scope = MovementScope::for_validation();
    let program = bind(&baked, &scope).map_err(|e| DescriptorError::InvalidShape {
        reason: format!("`{field}` expression is invalid: {e}"),
    })?;
    if program.root_type != expected {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{field}` expression must produce a {}, but its root produces a {}",
                ir_type_label(expected),
                ir_type_label(program.root_type)
            ),
        });
    }
    Ok(baked.root)
}

pub(crate) fn ir_node_from_json(
    value: serde_json::Value,
    field: &str,
) -> Result<IrNode, DescriptorError> {
    serde_json::from_value(value).map_err(|e| DescriptorError::InvalidShape {
        reason: format!("`{field}` is not a recognizable runtime expression: {e}"),
    })
}

pub(crate) fn ir_type_label(ty: IrType) -> &'static str {
    match ty {
        IrType::Number => "number",
        IrType::Bool => "boolean",
    }
}

pub(crate) fn validate_positive_finite(value: f32, field: &str) -> Result<f32, DescriptorError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be a finite value > 0.0, got {value}"),
        });
    }
    Ok(value)
}

pub(crate) fn validate_non_negative_finite(
    value: f32,
    field: &str,
) -> Result<f32, DescriptorError> {
    if !value.is_finite() || value < 0.0 {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be a finite value >= 0.0, got {value}"),
        });
    }
    Ok(value)
}

pub(crate) fn validate_in_range_finite(
    value: f32,
    min: f32,
    max: f32,
    field: &str,
) -> Result<f32, DescriptorError> {
    if !value.is_finite() || value < min || value > max {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be a finite value in [{min}, {max}], got {value}"),
        });
    }
    Ok(value)
}

/// Validate a finite value in `(min, max]` — strictly greater than `min`, at
/// most `max`.
pub(crate) fn validate_in_range_finite_exclusive_min(
    value: f32,
    min: f32,
    max: f32,
    field: &str,
) -> Result<f32, DescriptorError> {
    if !value.is_finite() || value <= min || value > max {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be a finite value in ({min}, {max}], got {value}"),
        });
    }
    Ok(value)
}

pub(crate) fn validate_interactive_name(
    kind: &str,
    has_label: bool,
    has_labelled_by: bool,
) -> Result<(), DescriptorError> {
    match (has_label, has_labelled_by) {
        (true, false) | (false, true) => Ok(()),
        (false, false) => Err(DescriptorError::InvalidShape {
            reason: format!(
                "a `{kind}` needs an accessible name: set exactly one of `label` or `labelledBy` (got neither)"
            ),
        }),
        (true, true) => Err(DescriptorError::InvalidShape {
            reason: format!("a `{kind}` must set exactly one of `label` or `labelledBy`, not both"),
        }),
    }
}

pub(crate) fn validate_image_name(
    has_label: bool,
    decorative: bool,
) -> Result<(), DescriptorError> {
    match (has_label, decorative) {
        (true, false) | (false, true) => Ok(()),
        (false, false) => Err(DescriptorError::InvalidShape {
            reason: "an `image` needs an accessible name: set exactly one of `label` or `decorative: true` (got neither)".to_string(),
        }),
        (true, true) => Err(DescriptorError::InvalidShape {
            reason: "an `image` must set exactly one of `label` or `decorative: true`, not both".to_string(),
        }),
    }
}

pub(crate) fn is_catalog_path_relative_to_content_root(path: &str) -> bool {
    let path = Path::new(path);
    if path.is_absolute() {
        return false;
    }
    path.components().all(|component| {
        !matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        )
    })
}

pub(crate) fn validate_finite_f32(value: f32, field: &str) -> Result<f32, DescriptorError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be finite, got {value}"),
        })
    }
}

pub(crate) fn validate_finite_array3(
    value: [f32; 3],
    field: &str,
) -> Result<[f32; 3], DescriptorError> {
    for (index, item) in value.iter().enumerate() {
        if !item.is_finite() {
            return Err(DescriptorError::InvalidShape {
                reason: format!("`{field}[{index}]` must be finite, got {item}"),
            });
        }
    }
    Ok(value)
}
