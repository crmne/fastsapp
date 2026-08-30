//! Desktop notifications for messages that arrive while the reader is away:
//! the window hidden, unfocused, or showing another chat.
//!
//! They go through the desktop's own notification service (D-Bus on Linux,
//! Notification Center on macOS, toasts on Windows), so they look and behave
//! like every other app's. Showing one can block briefly, and on Linux
//! waiting for a click blocks until the notification is gone, so each is
//! handled on its own thread.

use std::sync::{Arc, Mutex};

/// The title and body for a message: the chat's name, and in a group the
/// sender before the text, since the title does not say who wrote it.
pub fn lines(chat_name: &str, is_group: bool, sender: &str, summary: &str) -> (String, String) {
    let body = if is_group {
        format!("{sender}: {summary}")
    } else {
        summary.to_owned()
    };
    (chat_name.to_owned(), body)
}

/// Shows one. A click on it (where the desktop reports clicks) puts `chat`
/// into `opened` and wakes the app, which then shows that chat.
pub fn show(
    title: String,
    body: String,
    chat: String,
    opened: Arc<Mutex<Vec<String>>>,
    wake: impl Fn() + Send + 'static,
) {
    let spawned = std::thread::Builder::new()
        .name("notification".into())
        .spawn(move || deliver(&title, &body, chat, opened, wake));
    if let Err(error) = spawned {
        log::debug!("no thread for a notification: {error}");
    }
}

#[cfg(target_os = "linux")]
fn deliver(
    title: &str,
    body: &str,
    chat: String,
    opened: Arc<Mutex<Vec<String>>>,
    wake: impl Fn() + Send + 'static,
) {
    let mut notification = notify_rust::Notification::new();
    notification
        .appname("Fastsapp")
        .summary(title)
        .body(body)
        .icon("fastsapp")
        .action("default", "Open");
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
    _chat: String,
    _opened: Arc<Mutex<Vec<String>>>,
    _wake: impl Fn() + Send + 'static,
) {
    let mut notification = notify_rust::Notification::new();
    notification.appname("Fastsapp").summary(title).body(body);
    if let Err(error) = notification.show() {
        log::debug!("no notification: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
