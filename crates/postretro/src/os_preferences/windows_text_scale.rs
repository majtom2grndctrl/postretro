// Windows text scale (`UISettings::TextScaleFactor`) with change events.
// See: context/lib/player_options.md §5

use std::sync::mpsc::Sender;

use windows::Foundation::TypedEventHandler;
use windows::UI::ViewManagement::UISettings;

use super::OsUpdate;

/// Keeps the change handler registered; dropping it unregisters.
pub(super) struct TextScaleSubscription {
    settings: UISettings,
    token: i64,
}

impl Drop for TextScaleSubscription {
    fn drop(&mut self) {
        let _ = self.settings.RemoveTextScaleFactorChanged(self.token);
    }
}

/// Send the current text scale, then every change. `None` when the platform
/// API is unavailable; text scale then follows the engine default.
pub(super) fn subscribe(tx: Sender<OsUpdate>) -> Option<TextScaleSubscription> {
    let settings = match UISettings::new() {
        Ok(settings) => settings,
        Err(err) => {
            log::warn!("[Options] Windows text scale unavailable: {err}");
            return None;
        }
    };
    if let Ok(scale) = settings.TextScaleFactor() {
        let _ = tx.send(OsUpdate::TextScale(scale as f32));
    }
    let token = settings
        .TextScaleFactorChanged(&TypedEventHandler::new(
            move |sender: windows::core::Ref<UISettings>, _| {
                if let Some(settings) = sender.as_ref()
                    && let Ok(scale) = settings.TextScaleFactor()
                {
                    let _ = tx.send(OsUpdate::TextScale(scale as f32));
                }
                Ok(())
            },
        ))
        .inspect_err(|err| log::warn!("[Options] Windows text scale changes unavailable: {err}"))
        .ok()?;
    Some(TextScaleSubscription { settings, token })
}
