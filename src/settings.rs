//! User preferences, stored as one readable JSON file.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
    System,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [Self::Dark, Self::Light, Self::System];

    pub fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
            Self::System => "Follow system",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    /// Interface zoom, egui's zoom factor; Ctrl+plus/minus changes it.
    pub zoom: f32,
    pub sidebar_width: f32,
    /// Enter sends the message; Shift+Enter inserts a line break. Off, the
    /// two are swapped.
    pub enter_sends: bool,
    /// Tell the sender when their message has been read here. WhatsApp
    /// applies the account's own privacy setting on top of this.
    pub send_read_receipts: bool,
    /// Show "typing…" to the other side while composing.
    pub send_typing: bool,
    /// Fetch attachments as they come into view rather than on click.
    #[serde(alias = "auto_download_images")]
    pub auto_download: bool,
    /// Show the sender's picture beside every message, not only in groups.
    pub show_sender_pictures: bool,
    /// Which chat was open, to reopen it at the next start.
    pub last_chat: Option<String>,
    pub show_shortcut_hints: bool,
    /// Emoji picked lately, newest first.
    pub recent_emoji: Vec<String>,
    /// A GIPHY API key, for the GIF search; empty means the built-in one,
    /// if this build carries any.
    pub giphy_key: String,
    /// Closing the window hides to the tray and keeps the link up.
    pub keep_running_in_background: bool,
    /// Desktop notifications for messages that arrive while away.
    pub notifications: bool,
    /// People are named as the address book has them (else as they call
    /// themselves); off, the other way round. One rule everywhere.
    pub names_from_contacts: bool,
    /// A contact saved here also lands in the phone's own address book,
    /// not only in WhatsApp.
    pub save_contacts_to_phone: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::Dark,
            zoom: 1.0,
            sidebar_width: 320.0,
            enter_sends: true,
            send_read_receipts: true,
            send_typing: true,
            auto_download: true,
            show_sender_pictures: false,
            last_chat: None,
            show_shortcut_hints: true,
            recent_emoji: Vec::new(),
            giphy_key: String::new(),
            keep_running_in_background: true,
            notifications: true,
            names_from_contacts: true,
            save_contacts_to_phone: true,
        }
    }
}

/// A GIPHY key baked in when the app was built, from the
/// `FASTSAPP_GIPHY_KEY` environment variable; none in a plain build.
pub const BUILT_IN_GIPHY_KEY: Option<&str> = option_env!("FASTSAPP_GIPHY_KEY");

impl Settings {
    /// The GIPHY key to search with: the user's own, else the built-in
    /// one, else none.
    pub fn effective_giphy_key(&self) -> Option<String> {
        let own = self.giphy_key.trim();
        if !own.is_empty() {
            return Some(own.to_owned());
        }
        BUILT_IN_GIPHY_KEY
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_owned)
    }

    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(settings) => settings,
                Err(error) => {
                    log::warn!("settings file is unreadable, using defaults: {error}");
                    Self::default()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                log::warn!("could not read settings: {error}");
                Self::default()
            }
        }
    }

    /// Writes the file whole, through a temporary name, so a crash halfway
    /// leaves the previous settings intact.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, contents)?;
        std::fs::rename(&temp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        let parsed: Settings =
            serde_json::from_str(r#"{"theme":"light","future_field":1}"#).expect("parses");
        assert_eq!(parsed.theme, ThemeChoice::Light);
        assert!(parsed.enter_sends);
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("fastsapp-settings-{}", std::process::id()));
        let path = dir.join("settings.json");
        let settings = Settings {
            zoom: 1.25,
            enter_sends: false,
            ..Settings::default()
        };
        settings.save(&path).expect("saves");
        assert_eq!(Settings::load(&path), settings);
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod giphy_tests {
    use super::*;

    #[test]
    fn the_users_key_wins_and_is_trimmed() {
        let settings = Settings {
            giphy_key: "  abc  ".into(),
            ..Settings::default()
        };
        assert_eq!(settings.effective_giphy_key().as_deref(), Some("abc"));
    }

    #[test]
    fn without_a_key_of_their_own_the_built_in_one_is_used() {
        let settings = Settings {
            giphy_key: "   ".into(),
            ..Settings::default()
        };
        let expected = BUILT_IN_GIPHY_KEY
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_owned);
        assert_eq!(settings.effective_giphy_key(), expected);
    }
}
