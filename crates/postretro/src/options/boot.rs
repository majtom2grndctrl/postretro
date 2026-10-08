// Side-effect-free pre-window settings read and post-present identity completion.
// See: context/lib/boot_sequence.md §1 · context/lib/player_options.md §3

use super::{PlayerOptions, PlayerOptionsLoadStatus};
use std::path::PathBuf;

pub(crate) struct BootOptions {
    options: PlayerOptions,
    path: Option<PathBuf>,
    status: PlayerOptionsLoadStatus,
}

impl BootOptions {
    pub(crate) fn load(path: Option<PathBuf>) -> Self {
        let (options, status) = match path.as_deref() {
            Some(path) => PlayerOptions::load_with_status(path),
            None => {
                log::warn!(
                    "[Options] no platform config directory; running on in-memory defaults without persistence"
                );
                (
                    PlayerOptions::default(),
                    PlayerOptionsLoadStatus::Unavailable,
                )
            }
        };
        Self {
            options,
            path,
            status,
        }
    }

    pub(crate) fn options(&self) -> &PlayerOptions {
        &self.options
    }

    pub(crate) fn finish(self) -> (PlayerOptions, Option<PathBuf>) {
        let mut player_options = self.options;
        let load_status = self.status;
        if let Some(path) = self.path.as_deref() {
            let missing_settings = load_status == PlayerOptionsLoadStatus::Missing;
            let can_persist = load_status != PlayerOptionsLoadStatus::Unavailable;
            let mut generated_identity = false;

            if player_options.player_id.is_none() && can_persist {
                let mut player_id = [0; 16];
                match getrandom::fill(&mut player_id) {
                    Ok(()) => {
                        player_options.player_id = Some(player_id);
                        // An unreadable stored id is useless; the new one replaces it.
                        player_options.mark_written(super::keys::PLAYER_ID);
                        generated_identity = true;
                    }
                    Err(err) => log::warn!(
                        "[Options] failed to generate device identity: {err}; connecting anonymously"
                    ),
                }
            }

            if missing_settings || generated_identity {
                match player_options.save(path) {
                    Ok(()) if missing_settings => log::info!(
                        "[Options] no settings file found; wrote defaults to {}",
                        path.display()
                    ),
                    Ok(()) => {}
                    Err(err) => {
                        log::warn!(
                            "[Options] failed to persist device identity to {}: {err}; connecting anonymously",
                            path.display()
                        );
                        if generated_identity {
                            player_options.player_id = None;
                        }
                    }
                }
            }
        }
        (player_options, self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preload_writes_nothing_and_completion_does_not_reread_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/settings.toml");
        let preload = BootOptions::load(Some(path.clone()));
        assert!(!path.exists());
        assert_eq!(preload.options().player_id, None);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "invert_y = true\nwindow_mode = \"borderless\"\n").unwrap();
        let (options, _) = preload.finish();
        assert!(
            !options.invert_y,
            "completion must use the preloaded document"
        );
        assert_eq!(options.window_mode, super::super::WindowMode::Windowed);
        assert_eq!(PlayerOptions::load(&path).player_id, options.player_id);
    }
}
