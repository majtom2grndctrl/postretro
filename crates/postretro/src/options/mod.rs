// Per-human runtime preferences, persisted to a human-editable `settings.toml`
// (`persist.rs`). Pure data + filesystem — no wgpu, renderer, or UI coupling.
// See: context/lib/player_options.md

use serde::{Deserialize, Serialize};

use crate::input::DEFAULT_MOUSE_SENSITIVITY;

mod accessibility;
pub(crate) mod boot;
mod bridge;
mod document;
mod game;
mod graphics;
mod panel_actions;
mod persist;
mod resolved;
mod window;

pub use accessibility::AccessibilityOptions;
pub(crate) use bridge::OptionsBridge;
use document::StoredDocument;
pub use graphics::{FogQuality, RenderResolution, ShadowQuality, SurfaceDepthQuality};
pub(crate) use panel_actions::{PanelActionOutcome, apply_panel_action, is_numeric_field};
pub(crate) use persist::PlayerOptionsLoadStatus;
pub(crate) use resolved::{OsPreferences, apply_to_audio, reduce_motion_from_slots};
pub(crate) use window::{DisplayMode, WindowMode};

/// Registered dev-mod options tree whose open/close boundaries seed and flush
/// the session-owned settings bridge.
pub(crate) const OPTIONS_MENU_TREE_NAME: &str = "frontend.options";

/// `settings.toml` field keys, dotted for fields inside a table. A bridge or
/// panel write marks its key so the next save replaces a stored value this
/// build could not read.
pub(crate) mod keys {
    pub(crate) use super::accessibility::keys::*;

    pub(crate) const PLAYER_ID: &str = "player_id";
    pub(crate) const MOUSE_SENSITIVITY: &str = "mouse_sensitivity";
    pub(crate) const INVERT_Y: &str = "invert_y";
    pub(crate) const VIEW_FEEL_SCALE: &str = "view_feel_scale";
    pub(crate) const CROUCH_MODE: &str = "crouch_mode";
    pub(crate) const SHADOW_QUALITY: &str = "shadow_quality";
    pub(crate) const FOG_QUALITY: &str = "fog_quality";
    pub(crate) const SURFACE_DEPTH_QUALITY: &str = "surface_depth_quality";
    pub(crate) const WINDOW_MODE: &str = "window_mode";
    pub(crate) const RENDER_RESOLUTION: &str = "render_resolution";
    pub(crate) const SWITCH_CYCLE_DWELL_MS: &str = "switch_cycle_dwell_ms";
    pub(crate) const SCROLL_NOTCH_PIXELS: &str = "scroll_notch_pixels";
    pub(crate) const ACCESSIBILITY_PANEL_SHOWN: &str = "accessibility_panel_shown";
}

/// Filename written into the platform config directory
/// (`startup::app_dirs::AppDirs::settings_path`).
pub(crate) const SETTINGS_FILENAME: &str = "settings.toml";
const DEFAULT_SCROLL_NOTCH_PIXELS: f32 = 120.0;
const MAX_SCROLL_NOTCH_PIXELS: f32 = 4_096.0;
const MAX_SWITCH_CYCLE_DWELL_MS: u32 = 60_000;

/// How the crouch action is interpreted by the input layer. Resolved upstream
/// of the movement intent: the movement intent only ever sees the single
/// resolved per-tick bit (`MovementInput::crouch_intent`), never this mode.
///
/// Wire format is snake_case (matching the rest of `PlayerOptions`): TOML values
/// are `"hold"` / `"toggle"`. Defaults to `Hold` — hold-to-crouch is the
/// boomer-shooter baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrouchMode {
    /// One press latches crouch on, a second press latches it off (edge-driven).
    Toggle,
    /// Crouch intent is active only while the button is held (level signal).
    #[default]
    Hold,
}

impl CrouchMode {
    fn slot_value(self) -> &'static str {
        match self {
            Self::Hold => "hold",
            Self::Toggle => "toggle",
        }
    }

    fn from_slot_value(value: &str) -> Option<Self> {
        match value {
            "hold" => Some(Self::Hold),
            "toggle" => Some(Self::Toggle),
            _ => None,
        }
    }
}

/// Per-human runtime preferences, persisted as TOML.
///
/// Wire format is deliberately snake_case: this is a human-editable config,
/// distinct from the project's camelCase script-facing surface. Each field
/// loads on its own: an absent key takes its default, and a value this build
/// cannot read falls back for that field alone, with a warning, and stays in
/// the file until the player writes that field.
#[derive(Debug, Clone)]
pub struct PlayerOptions {
    /// Device-local player identity asserted when connecting to a multiplayer
    /// host. `None` is the anonymous fallback when this installation cannot
    /// persist an identity.
    pub player_id: Option<[u8; 16]>,

    /// Radians per raw mouse unit. Must be finite and > 0; defaults to the
    /// input subsystem's `DEFAULT_MOUSE_SENSITIVITY`.
    pub mouse_sensitivity: f32,

    /// When true, the pitch (Y) mouse axis is negated.
    pub invert_y: bool,

    /// Accessibility scale for view feel (head bob, kick, sway). Clamped to
    /// `[0, 1]` on load rather than rejected, so a hand-edited out-of-range
    /// value degrades gracefully. Part of the accessibility group, but keeps
    /// its top-level key.
    pub view_feel_scale: f32,

    /// How the crouch action is interpreted by the input layer (hold vs toggle).
    /// Engine-internal config — no SDK type / scripting surface (see
    /// player_options.md §6).
    pub crouch_mode: CrouchMode,

    /// Spot-shadow map resolution tier, applied on renderer full-init or the
    /// next level install.
    pub shadow_quality: ShadowQuality,

    /// Volumetric-fog march density tier, applied live and on renderer full-init.
    pub fog_quality: FogQuality,

    /// Surface Depth (texel-space parallax) switch, applied live by rewriting
    /// the per-material uniform buffers, and re-applied on renderer full-init.
    pub surface_depth_quality: SurfaceDepthQuality,

    /// Scene render resolution, applied live and re-applied on renderer
    /// full-init before the scene targets are built.
    pub render_resolution: RenderResolution,

    pub(crate) window_mode: WindowMode,
    pub(crate) display_mode: Option<DisplayMode>,

    /// Optional local override for the mod's cycle-selection dwell. `None`
    /// preserves the mod policy; an explicit zero selects immediately.
    pub switch_cycle_dwell_ms: Option<u32>,

    /// Pixel distance treated as one scroll-wheel notch. This is a concrete
    /// per-device setting, not a policy override; 120 matches the OS-standard
    /// wheel quantum.
    pub scroll_notch_pixels: f32,

    /// The `[accessibility]` table.
    pub accessibility: AccessibilityOptions,

    /// Whether the player has closed the engine accessibility panel once. The
    /// first-launch hold shows the panel until this is written.
    pub accessibility_panel_shown: bool,

    /// This session's writes to `[game."<mod_id>".bindings]` rows.
    game_binding_writes: game::GameBindingWrites,

    /// The loaded document a save rewrites.
    stored: StoredDocument,
}

/// Preference equality: the loaded document is persistence state, not a
/// preference.
impl PartialEq for PlayerOptions {
    fn eq(&self, other: &Self) -> bool {
        self.player_id == other.player_id
            && self.mouse_sensitivity == other.mouse_sensitivity
            && self.invert_y == other.invert_y
            && self.view_feel_scale == other.view_feel_scale
            && self.crouch_mode == other.crouch_mode
            && self.shadow_quality == other.shadow_quality
            && self.fog_quality == other.fog_quality
            && self.surface_depth_quality == other.surface_depth_quality
            && self.render_resolution == other.render_resolution
            && self.window_mode == other.window_mode
            && self.display_mode == other.display_mode
            && self.switch_cycle_dwell_ms == other.switch_cycle_dwell_ms
            && self.scroll_notch_pixels == other.scroll_notch_pixels
            && self.accessibility == other.accessibility
            && self.accessibility_panel_shown == other.accessibility_panel_shown
            && self.game_binding_writes == other.game_binding_writes
    }
}

fn default_mouse_sensitivity() -> f32 {
    DEFAULT_MOUSE_SENSITIVITY
}

fn default_invert_y() -> bool {
    false
}

fn default_view_feel_scale() -> f32 {
    1.0
}

fn default_scroll_notch_pixels() -> f32 {
    DEFAULT_SCROLL_NOTCH_PIXELS
}

/// Warn that the stored value for `field` (dotted key) is not finite and is
/// about to fall back to its default. TOML's `nan`/`inf` float literals parse
/// cleanly, so this is the only place a hand-edited non-finite value surfaces.
fn warn_non_finite(field: &str, value: f32) {
    log::warn!("[Options] `{field}` is not a finite number ({value}); using its default");
}

impl Default for PlayerOptions {
    fn default() -> Self {
        Self {
            player_id: None,
            mouse_sensitivity: default_mouse_sensitivity(),
            invert_y: default_invert_y(),
            view_feel_scale: default_view_feel_scale(),
            crouch_mode: CrouchMode::default(),
            shadow_quality: ShadowQuality::default(),
            fog_quality: FogQuality::default(),
            surface_depth_quality: SurfaceDepthQuality::default(),
            render_resolution: RenderResolution::default(),
            window_mode: WindowMode::default(),
            display_mode: None,
            switch_cycle_dwell_ms: None,
            scroll_notch_pixels: default_scroll_notch_pixels(),
            accessibility: AccessibilityOptions::default(),
            accessibility_panel_shown: false,
            game_binding_writes: Default::default(),
            stored: StoredDocument::default(),
        }
    }
}

impl PlayerOptions {
    /// Clamp loaded values into their valid ranges. Applied after
    /// deserialization so hand-edited out-of-range values are corrected rather
    /// than rejected. Every f32 field falls back to its default when not
    /// finite: TOML accepts `nan`/`inf` as valid float literals, so a
    /// hand-edited value sails past deserialization, and `f32::clamp` (unlike
    /// a `<`/`>` fallback check) leaves NaN untouched rather than clamping it —
    /// an unclamped NaN then fails every downstream `PartialEq` comparison,
    /// which is what let a NaN `view_feel_scale` keep the options bridge from
    /// ever settling. `mouse_sensitivity` also falls back when non-positive (a
    /// zero/negative sensitivity would break look input).
    fn sanitize(&mut self) {
        if self.view_feel_scale.is_finite() {
            self.view_feel_scale = self.view_feel_scale.clamp(0.0, 1.0);
        } else {
            warn_non_finite(keys::VIEW_FEEL_SCALE, self.view_feel_scale);
            self.view_feel_scale = default_view_feel_scale();
        }

        if !self.mouse_sensitivity.is_finite() {
            warn_non_finite(keys::MOUSE_SENSITIVITY, self.mouse_sensitivity);
        }
        if !(self.mouse_sensitivity.is_finite() && self.mouse_sensitivity > 0.0) {
            self.mouse_sensitivity = default_mouse_sensitivity();
        }

        self.switch_cycle_dwell_ms = self
            .switch_cycle_dwell_ms
            .map(|dwell| dwell.min(MAX_SWITCH_CYCLE_DWELL_MS));

        if !self.scroll_notch_pixels.is_finite() {
            warn_non_finite(keys::SCROLL_NOTCH_PIXELS, self.scroll_notch_pixels);
        }
        if !self.scroll_notch_pixels.is_finite() || self.scroll_notch_pixels <= 0.0 {
            self.scroll_notch_pixels = default_scroll_notch_pixels();
        } else {
            self.scroll_notch_pixels = self.scroll_notch_pixels.min(MAX_SCROLL_NOTCH_PIXELS);
        }
        self.accessibility.sanitize();
    }

    /// Record that the player (or the engine on the player's behalf) wrote the
    /// field at dotted `key`, so the next save replaces a stored value this
    /// build could not read.
    pub(crate) fn mark_written(&mut self, key: &str) {
        self.stored.clear_unrecognized(key);
    }

    /// False when the settings file exists but could not be read or parsed.
    /// Nothing replaces such a file.
    pub(crate) fn can_persist(&self) -> bool {
        !self.stored.is_read_only()
    }
}

#[cfg(test)]
mod tests;
