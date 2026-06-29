// Focus-traversal descriptor types: the container `focus` policy union, its kind
// and hold-to-repeat timing, and per-node directional neighbor overrides.
// See: context/lib/ui.md §4

use serde::{Deserialize, Serialize};

/// How a container moves focus among its children (M13 Goal F, Task 3). Authored
/// on a container as the additive `focus` field — an untagged union so the wire
/// form is either a bare string (`"linear"` / `"spatial"`) or an object carrying
/// the policy plus optional `wrap`/`repeat`. The string forms are shorthand for
/// the object with default `wrap`/`repeat`. The focus engine (app-side) reads the
/// resolved policy off the exported focus-rect list to move focus through the tree.
///
/// `Shorthand` is declared FIRST so a bare JSON string lands on it (untagged
/// variants are tried in declaration order; a string can only match `Shorthand`,
/// an object only `Detailed`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FocusPolicy {
    /// Bare-string shorthand: `"linear"` or `"spatial"`, default wrap, no repeat.
    Shorthand(FocusKind),
    /// Object form: the policy kind plus optional `wrap` and hold-to-repeat config.
    Detailed {
        policy: FocusKind,
        /// Whether directional/next-prev nav wraps past the ends (defaults true).
        #[serde(default = "default_wrap", skip_serializing_if = "is_true")]
        wrap: bool,
        /// Hold-to-repeat timing for held directions; absent means no repeat.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repeat: Option<RepeatPolicy>,
    },
}

/// The two focus-traversal kinds. `Linear` walks the container's children in tree
/// order; `Spatial` picks the nearest child center in the pressed direction's
/// half-plane (grid navigation). Maps to the camelCase wire literals `"linear"` /
/// `"spatial"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FocusKind {
    Linear,
    Spatial,
}

/// Hold-to-repeat timing for a container's directional nav (M13 Goal F, Task 3).
/// `initial_delay_ms` is the dwell before the first auto-repeat; `interval_ms` is
/// the cadence after that. The focus engine accumulates dt against these. Confirm
/// and cancel never repeat regardless of this policy.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepeatPolicy {
    pub initial_delay_ms: f32,
    pub interval_ms: f32,
}

impl FocusPolicy {
    /// The traversal kind, regardless of which wire form authored it.
    pub fn kind(&self) -> FocusKind {
        match self {
            FocusPolicy::Shorthand(kind) => *kind,
            FocusPolicy::Detailed { policy, .. } => *policy,
        }
    }

    /// Whether nav wraps past the ends. The shorthand form defaults to `true`.
    pub fn wrap(&self) -> bool {
        match self {
            FocusPolicy::Shorthand(_) => true,
            FocusPolicy::Detailed { wrap, .. } => *wrap,
        }
    }

    /// The hold-to-repeat policy, if the container declared one.
    pub fn repeat(&self) -> Option<RepeatPolicy> {
        match self {
            FocusPolicy::Shorthand(_) => None,
            FocusPolicy::Detailed { repeat, .. } => *repeat,
        }
    }
}

/// serde default for `FocusPolicy::Detailed::wrap` — wrap is on unless authored off.
fn default_wrap() -> bool {
    true
}

/// `skip_serializing_if` predicate: omit `wrap` when it is the `true` default, so
/// a wrap-on container round-trips without emitting the key.
fn is_true(b: &bool) -> bool {
    *b
}

/// Per-direction focus-neighbor overrides authored on a node (M13 Goal F, Task 3).
/// Each field, when set, names the node id focus jumps to when that direction is
/// pressed while this node is focused — overriding the container's focus policy.
/// All fields default to absent, and the whole struct skip-serializes when empty,
/// so a node that authors no override round-trips byte-identically.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FocusNeighbors {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub up: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub down: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<String>,
}

impl FocusNeighbors {
    /// True when no direction is overridden — the `skip_serializing_if` predicate
    /// so an override-less node omits the `focusNeighbors` key entirely.
    pub fn is_empty(&self) -> bool {
        self.up.is_none() && self.down.is_none() && self.left.is_none() && self.right.is_none()
    }
}
