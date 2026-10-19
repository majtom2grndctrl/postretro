// `movement.viewFeel.pose` validation, shared by the QuickJS and Luau
// view-feel parsers so both runtimes accept and reject alike, with the same
// diagnostics. Each VM only reads its value into a `PoseValue` tree.
// See: context/lib/movement.md §1 (view feel) · context/lib/scripting.md §1 (parser twins)

use std::collections::{BTreeMap, BTreeSet};

use postretro_foundation::data_descriptors::{
    LedgeFocus, PoseChannels, PoseParams, PoseRow, VaultPoseRow,
};

use super::DescriptorError;

const POSE: &str = "movement.viewFeel.pose";

/// How many nested blocks a pose holds: the `pose` table, a row, and a row's
/// `ledgeFocus`. A VM reader classifies a table any deeper as
/// [`PoseValue::Other`], which bounds the read.
pub(crate) const POSE_BLOCK_DEPTH: u8 = 3;

/// How many prototypes (JS) or `__index` tables (Luau) a reader walks looking
/// for inherited entries. A chain still going past it is reported unnamed.
pub(crate) const MAX_INHERITANCE_WALK: usize = 16;

/// A present, non-nil pose value as one VM read it. Absent, `null`,
/// `undefined` and `nil` entries are never recorded, because Luau cannot hold
/// a nil entry and both runtimes must see the same keys.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PoseValue {
    /// Kept as `f64` so a range check sees the authored value before f32
    /// narrowing can round it onto a bound.
    Number(f64),
    Block(PoseBlock),
    /// Neither a number nor a block of named fields: a string, a boolean, an
    /// array, a function, a Luau table with array entries or a non-string
    /// key, or a table nested past [`POSE_BLOCK_DEPTH`].
    Other,
}

/// A block of named fields: a JS object, or a Luau table holding only string
/// keys.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct PoseBlock {
    /// Own entries: JS own string-keyed properties, or Luau raw entries.
    pub fields: BTreeMap<String, PoseValue>,
    /// Keys the block would inherit: enumerable string keys on the JS
    /// prototype chain, or string keys of Luau `__index` tables.
    pub inherited: BTreeSet<String>,
    /// The block inherits entries no key can name: a Luau `__index` function,
    /// a non-string `__index` key, or a chain longer than
    /// [`MAX_INHERITANCE_WALK`].
    pub inherits_unnamed: bool,
}

fn invalid(reason: String) -> DescriptorError {
    DescriptorError::InvalidShape { reason }
}

/// A leftover `movement.viewFeel.slide` or `.vault` block. `viewFeel`
/// otherwise ignores unknown keys, so a retired block would silently pose
/// nothing.
pub(crate) fn moved_view_block(key: &str) -> DescriptorError {
    invalid(format!(
        "`movement.viewFeel.{key}` has moved; author `{POSE}.{key}` instead"
    ))
}

/// `movement.viewFeel` inherits `pose` rather than owning it.
pub(crate) fn inherited_pose_block() -> DescriptorError {
    inherited_key("movement.viewFeel", "pose")
}

fn inherited_key(block: &str, key: &str) -> DescriptorError {
    invalid(format!(
        "`{block}.{key}` is inherited; author it directly on `{block}`"
    ))
}

/// A bound or authored value as a message shows it: plain decimals, and
/// exponent notation for magnitudes whose decimal expansion runs long.
fn show(value: f64) -> String {
    let magnitude = value.abs();
    if value.is_finite() && magnitude != 0.0 && !(1.0e-6..1.0e9).contains(&magnitude) {
        format!("{value:e}")
    } else {
        value.to_string()
    }
}

/// The f64 an f32 bound was written as: `0.1f32` widens to 0.10000000149…,
/// which would reject an authored `0.1`.
fn decimal(value: f32) -> f64 {
    value.to_string().parse().unwrap_or(f64::from(value))
}

/// An accepted closed or half-open range, checked on the authored f64.
#[derive(Clone, Copy)]
struct Range {
    min: f64,
    max: f64,
    min_exclusive: bool,
}

impl Range {
    fn closed(min: f32, max: f32) -> Self {
        Self {
            min: decimal(min),
            max: decimal(max),
            min_exclusive: false,
        }
    }

    fn angle() -> Self {
        Self::closed(-PoseChannels::MAX_ANGLE, PoseChannels::MAX_ANGLE)
    }

    fn fov() -> Self {
        Self::closed(-PoseChannels::MAX_FOV, PoseChannels::MAX_FOV)
    }

    /// Metres down, capped at the f32 ceiling so a huge authored drop cannot
    /// narrow to infinity. The exact widened ceiling, not its decimal text,
    /// which rounds past it.
    fn eye_drop() -> Self {
        Self {
            min: 0.0,
            max: f64::from(f32::MAX),
            min_exclusive: false,
        }
    }

    fn rate() -> Self {
        Self::closed(PoseRow::MIN_RATE, PoseRow::MAX_RATE)
    }

    fn unit() -> Self {
        Self::closed(0.0, 1.0)
    }

    fn max_pitch() -> Self {
        Self {
            min_exclusive: true,
            ..Self::closed(0.0, LedgeFocus::MAX_PITCH)
        }
    }

    fn check(self, path: &str, value: f64) -> Result<f64, DescriptorError> {
        let above_min = if self.min_exclusive {
            value > self.min
        } else {
            value >= self.min
        };
        if !value.is_finite() || !above_min || value > self.max {
            let open = if self.min_exclusive { '(' } else { '[' };
            return Err(invalid(format!(
                "`{path}` must be finite in {open}{}, {}], got {}",
                show(self.min),
                show(self.max),
                show(value)
            )));
        }
        Ok(value)
    }
}

/// Validate `movement.viewFeel.pose` as one VM read it; `None` is an absent
/// or nil block.
pub(crate) fn pose_params_from_value(
    value: Option<PoseValue>,
) -> Result<Option<PoseParams>, DescriptorError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let mut pose = Fields::open(
        value,
        POSE.to_string(),
        |key| format!("`{POSE}.{key}` is not a pose row; expected slide or vault"),
        &["slide", "vault"],
    )?;
    let slide = match pose.take_block("slide") {
        Some((value, path)) => Some(slide_row(value, path)?),
        None => None,
    };
    let vault = match pose.take_block("vault") {
        Some((value, path)) => Some(vault_row(value, path)?),
        None => None,
    };
    Ok(Some(PoseParams { slide, vault }))
}

/// One block's own fields, after its shape, inheritance and key set passed.
struct Fields {
    path: String,
    fields: BTreeMap<String, PoseValue>,
}

impl Fields {
    /// Accept `value` as a block at `path` whose keys are all in `allowed`.
    /// A block that inherits anything is rejected before its keys are read,
    /// and an unknown key is reported through `unknown`, smallest first, so
    /// both runtimes name the same one.
    fn open(
        value: PoseValue,
        path: String,
        unknown: impl Fn(&str) -> String,
        allowed: &[&str],
    ) -> Result<Self, DescriptorError> {
        let PoseValue::Block(block) = value else {
            return Err(invalid(format!("`{path}` must be an object")));
        };
        if let Some(key) = block.inherited.first() {
            return Err(inherited_key(&path, key));
        }
        if block.inherits_unnamed {
            return Err(invalid(format!(
                "`{path}` inherits fields through its prototype or `__index`; author them directly on `{path}`"
            )));
        }
        if let Some(key) = block
            .fields
            .keys()
            .find(|key| !allowed.contains(&key.as_str()))
        {
            return Err(invalid(unknown(key)));
        }
        Ok(Self {
            path,
            fields: block.fields,
        })
    }

    fn path_of(&self, key: &str) -> String {
        format!("{}.{key}", self.path)
    }

    fn take_block(&mut self, key: &str) -> Option<(PoseValue, String)> {
        let path = self.path_of(key);
        self.fields.remove(key).map(|value| (value, path))
    }

    fn number(&mut self, key: &str, range: Range) -> Result<Option<f64>, DescriptorError> {
        let path = self.path_of(key);
        match self.fields.remove(key) {
            None => Ok(None),
            Some(PoseValue::Number(value)) => range.check(&path, value).map(Some),
            Some(_) => Err(invalid(format!("`{path}` must be a number"))),
        }
    }

    fn required(&mut self, key: &str, range: Range) -> Result<f64, DescriptorError> {
        let path = self.path_of(key);
        self.number(key, range)?
            .ok_or_else(|| invalid(format!("`{path}` is required")))
    }

    /// A missing channel holds nothing.
    fn channel(&mut self, key: &str, range: Range) -> Result<f32, DescriptorError> {
        Ok(self.number(key, range)?.map_or(0.0, |value| value as f32))
    }

    fn channels(&mut self) -> Result<PoseChannels, DescriptorError> {
        Ok(PoseChannels {
            pitch: self.channel("pitch", Range::angle())?,
            roll: self.channel("roll", Range::angle())?,
            eye_drop: self.channel("eyeDrop", Range::eye_drop())?,
            fov: self.channel("fov", Range::fov())?,
        })
    }
}

const CHANNEL_KEYS: [&str; 4] = ["pitch", "roll", "eyeDrop", "fov"];

/// The wording shared by every row for a key it does not take. `row_hint`
/// covers the keys another row takes.
fn unknown_row_field(path: &str, key: &str, expected: &str, row_hint: Option<&str>) -> String {
    if key == "fovIncrease" {
        return format!("`{path}.fovIncrease` has been renamed; author `{path}.fov` instead");
    }
    let row = path.rsplit('.').next().unwrap_or(path);
    match row_hint {
        Some(hint) => format!("`{path}.{key}` is not a {row} pose field; {hint}"),
        None => format!("`{path}.{key}` is not a {row} pose field; expected {expected}"),
    }
}

fn slide_row(value: PoseValue, path: String) -> Result<PoseRow, DescriptorError> {
    let expected = "pitch, roll, eyeDrop, fov, enterRate or exitRate";
    let unknown_path = path.clone();
    let mut row = Fields::open(
        value,
        path,
        |key| {
            let hint = match key {
                "easeIn" | "easeOut" => {
                    Some("the slide row is timed by `enterRate` and `exitRate`, not shares")
                }
                "ledgeFocus" => Some("`ledgeFocus` is vault-only"),
                _ => None,
            };
            unknown_row_field(&unknown_path, key, expected, hint)
        },
        &[CHANNEL_KEYS.as_slice(), &["enterRate", "exitRate"]].concat(),
    )?;
    Ok(PoseRow {
        channels: row.channels()?,
        enter_rate: row.required("enterRate", Range::rate())? as f32,
        exit_rate: row.required("exitRate", Range::rate())? as f32,
    })
}

fn vault_row(value: PoseValue, path: String) -> Result<VaultPoseRow, DescriptorError> {
    let expected = "pitch, roll, eyeDrop, fov, easeIn, easeOut or ledgeFocus";
    let unknown_path = path.clone();
    let mut row = Fields::open(
        value,
        path,
        |key| {
            let hint = matches!(key, "enterRate" | "exitRate").then_some(
                "the vault row is timed by the climb's progress through `easeIn` and `easeOut` shares, not rates",
            );
            unknown_row_field(&unknown_path, key, expected, hint)
        },
        &[
            CHANNEL_KEYS.as_slice(),
            &["easeIn", "easeOut", "ledgeFocus"],
        ]
        .concat(),
    )?;
    let channels = row.channels()?;
    // A default share is a widened f32 (0.2 reads 0.2000000003), so allow that
    // much over 1: an authored 0.8 with the default 0.2 still fits.
    let ease_in = row
        .number("easeIn", Range::unit())?
        .unwrap_or(f64::from(VaultPoseRow::DEFAULT_EASE_IN));
    let ease_out = row
        .number("easeOut", Range::unit())?
        .unwrap_or(f64::from(VaultPoseRow::DEFAULT_EASE_OUT));
    let shares = ease_in + ease_out;
    if shares > 1.0 + 1.0e-6 {
        let path = &row.path;
        return Err(invalid(format!(
            "`{path}.easeIn` plus `{path}.easeOut` must be at most 1, got {}",
            show(shares)
        )));
    }
    let ledge_focus = match row.take_block("ledgeFocus") {
        Some((value, path)) => Some(ledge_focus(value, path)?),
        None => None,
    };
    Ok(VaultPoseRow {
        channels,
        ease_in: ease_in as f32,
        ease_out: ease_out as f32,
        ledge_focus,
    })
}

fn ledge_focus(value: PoseValue, path: String) -> Result<LedgeFocus, DescriptorError> {
    let unknown_path = path.clone();
    let mut focus = Fields::open(
        value,
        path,
        |key| {
            format!(
                "`{unknown_path}.{key}` is not a ledge focus field; expected strength or maxPitch"
            )
        },
        &["strength", "maxPitch"],
    )?;
    Ok(LedgeFocus {
        strength: focus.required("strength", Range::unit())? as f32,
        max_pitch: focus.required("maxPitch", Range::max_pitch())? as f32,
    })
}
