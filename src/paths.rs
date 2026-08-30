//! Where Fastsapp keeps its files.
//!
//! Configuration, durable state (the linked device's keys and the message
//! archive), and disposable caches (media, avatars) live in the platform's
//! conventional directories, so clearing a cache never unlinks the device
//! and a config backup never contains a key.

use std::path::PathBuf;

use directories::ProjectDirs;

#[derive(Clone, Debug)]
pub struct AppDirs {
    pub config: PathBuf,
    pub state: PathBuf,
    pub cache: PathBuf,
}

impl AppDirs {
    pub fn discover() -> Self {
        let dirs = match Self::of("fastsapp") {
            Some(dirs) => dirs,
            None => {
                let fallback = std::env::current_dir().unwrap_or_default();
                Self {
                    config: fallback.join("fastsapp-config"),
                    state: fallback.join("fastsapp-state"),
                    cache: fallback.join("fastsapp-cache"),
                }
            }
        };
        dirs.adopt_previous_name();
        dirs
    }

    /// The platform's directories for an app of this name.
    fn of(name: &str) -> Option<Self> {
        let project = ProjectDirs::from("me", "paolino", name)?;
        Some(Self {
            config: project.config_dir().to_path_buf(),
            state: project
                .state_dir()
                .map(|path| path.to_path_buf())
                .unwrap_or_else(|| project.data_local_dir().to_path_buf()),
            cache: project.cache_dir().to_path_buf(),
        })
    }

    /// The app was called fastwhatsapp until August 2026. What it kept
    /// under that name, the linked device above all, is moved over once,
    /// so the rename unlinks nobody; nothing is copied or deleted.
    fn adopt_previous_name(&self) {
        let Some(old) = Self::of("fastwhatsapp") else {
            return;
        };
        for (from, to) in [
            (&old.config, &self.config),
            (&old.state, &self.state),
            (&old.cache, &self.cache),
        ] {
            if from.is_dir() && !to.exists() {
                match std::fs::rename(from, to) {
                    Ok(()) => eprintln!("moved {} to {}", from.display(), to.display()),
                    Err(error) => eprintln!(
                        "could not move {} to {}: {error}",
                        from.display(),
                        to.display()
                    ),
                }
            }
        }
    }

    /// Everything under one directory, for tests and throwaway runs.
    pub fn under(root: &std::path::Path) -> Self {
        Self {
            config: root.join("config"),
            state: root.join("state"),
            cache: root.join("cache"),
        }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config.join("settings.json")
    }

    /// The linked device: identity, Signal sessions, and app state keys.
    /// Owned by whatsapp-rust; deleting it unlinks this computer.
    pub fn session_db(&self) -> PathBuf {
        self.state.join("session.db")
    }

    /// The message archive this app keeps, since WhatsApp only replays
    /// history once, when the device is linked.
    pub fn archive_db(&self) -> PathBuf {
        self.state.join("archive.db")
    }

    /// The log of the current run, replaced at every start.
    pub fn log_file(&self) -> PathBuf {
        self.state.join("fastsapp.log")
    }

    /// Where a panic is recorded before the process dies of it.
    pub fn panic_log(&self) -> PathBuf {
        self.state.join("panic.log")
    }

    /// Downloaded and decrypted attachments, by message id.
    pub fn media_cache_dir(&self) -> PathBuf {
        self.cache.join("media")
    }

    /// Profile pictures, by chat.
    pub fn avatar_cache_dir(&self) -> PathBuf {
        self.cache.join("avatars")
    }

    /// The phone's recent stickers, by file hash.
    pub fn sticker_cache_dir(&self) -> PathBuf {
        self.cache.join("stickers")
    }

    /// Where a chat's or person's picture is kept, once fetched; `full`
    /// for the large one an info dialog shows.
    pub fn avatar_file(&self, id: &str, full: bool) -> PathBuf {
        let stem: String = id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        self.avatar_cache_dir()
            .join(format!("{stem}{}.jpg", if full { "-full" } else { "" }))
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        for dir in [&self.config, &self.state, &self.cache] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}
