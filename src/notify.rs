//! Desktop notifications when the app is hidden, unfocused, or on another chat.
//!
//! Delivery uses the platform notification service. Each notification runs on
//! its own thread because delivery and click handling can block.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Builds the notification title and body, including the group sender.
pub fn lines(chat_name: &str, is_group: bool, sender: &str, summary: &str) -> (String, String) {
    let body = if is_group {
        format!("{sender}: {summary}")
    } else {
        summary.to_owned()
    };
    (chat_name.to_owned(), body)
}

/// Shows a notification with an optional chat picture. Clicking it queues the
/// chat id and wakes the app on platforms that report clicks.
pub fn show(
    title: String,
    body: String,
    picture: Option<PathBuf>,
    chat: String,
    opened: Arc<Mutex<Vec<String>>>,
    wake: impl Fn() + Send + 'static,
) {
    let spawned = std::thread::Builder::new()
        .name("notification".into())
        .spawn(move || deliver(&title, &body, picture.as_deref(), chat, opened, wake));
    if let Err(error) = spawned {
        log::debug!("no thread for a notification: {error}");
    }
}

#[cfg(target_os = "linux")]
fn deliver(
    title: &str,
    body: &str,
    picture: Option<&std::path::Path>,
    chat: String,
    opened: Arc<Mutex<Vec<String>>>,
    wake: impl Fn() + Send + 'static,
) {
    let mut notification = notify_rust::Notification::new();
    notification
        .appname("FastsApp")
        .summary(title)
        .body(body)
        .icon("fastsapp")
        .action("default", "Open");
    if let Some(picture) = picture {
        notification.image_path(&picture.to_string_lossy());
    }
    match notification.show() {
        Ok(handle) => handle.wait_for_action(|action| {
            if action == "default" {
                opened.lock().unwrap_or_else(|p| p.into_inner()).push(chat);
                wake();
            }
        }),
        Err(error) => log::debug!("no notification: {error}"),
    }
}

#[cfg(not(target_os = "linux"))]
fn deliver(
    title: &str,
    body: &str,
    picture: Option<&std::path::Path>,
    _chat: String,
    _opened: Arc<Mutex<Vec<String>>>,
    _wake: impl Fn() + Send + 'static,
) {
    let mut notification = notify_rust::Notification::new();
    notification.appname("FastsApp").summary(title).body(body);
    // Windows uses the image; macOS always uses the app icon.
    if let Some(picture) = picture {
        notification.image_path(&picture.to_string_lossy());
    }
    if let Err(error) = notification.show() {
        log::debug!("no notification: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shows a test notification with an optional cached picture:
    /// `cargo test --all-features shows_one -- --ignored --nocapture`.
    #[test]
    #[ignore = "shows a real notification"]
    fn shows_one_on_this_desktop() {
        let picture = std::fs::read_dir(crate::paths::AppDirs::discover().avatar_cache_dir())
            .ok()
            .and_then(|entries| entries.flatten().map(|entry| entry.path()).next());
        show(
            "Ada Lovelace".into(),
            "A test from FastsApp, with a picture".into(),
            picture,
            "test".into(),
            Default::default(),
            || {},
        );
        std::thread::sleep(std::time::Duration::from_secs(2));
    }

    #[test]
    fn a_group_names_the_sender_and_a_chat_does_not() {
        assert_eq!(
            lines("Rust Berlin", true, "Mira", "Save me a seat"),
            ("Rust Berlin".to_owned(), "Mira: Save me a seat".to_owned())
        );
        assert_eq!(
            lines("Ada Lovelace", false, "Ada Lovelace", "Photo"),
            ("Ada Lovelace".to_owned(), "Photo".to_owned())
        );
    }
}
