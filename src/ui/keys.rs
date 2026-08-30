//! Keyboard shortcuts.

use egui::{Key, Modifiers};

use crate::app::App;
use crate::model::{Action, Dialog, Page};

pub fn handle(app: &mut App, ctx: &egui::Context) {
    let mut actions = Vec::new();
    ctx.input_mut(|input| {
        let mut key = |modifiers: Modifiers, key: Key, action: Action| {
            if input.consume_key(modifiers, key) {
                actions.push(action);
            }
        };
        key(Modifiers::COMMAND, Key::F, Action::FocusSearch);
        key(Modifiers::COMMAND, Key::K, Action::FocusSearch);
        key(Modifiers::COMMAND, Key::B, Action::ToggleSidebar);
        key(Modifiers::COMMAND, Key::Comma, Action::Open(Page::Settings));
        key(Modifiers::COMMAND, Key::Q, Action::Quit);
        key(Modifiers::COMMAND, Key::W, Action::CloseWindow);
        key(
            Modifiers::COMMAND,
            Key::Slash,
            Action::ShowDialog(Dialog::Shortcuts),
        );
        key(Modifiers::COMMAND, Key::Plus, Action::ZoomBy(0.1));
        key(Modifiers::COMMAND, Key::Equals, Action::ZoomBy(0.1));
        key(Modifiers::COMMAND, Key::Minus, Action::ZoomBy(-0.1));
        key(Modifiers::COMMAND, Key::Num0, Action::ResetZoom);
        key(Modifiers::COMMAND, Key::End, Action::ScrollToBottom);
    });
    // Escape backs out of whatever is on top: a dialog, a reply, then the
    // settings page.
    // An open menu takes Escape itself; taking it here would leave the
    // menu up and act on whatever is underneath.
    let menu_open = egui::Popup::is_any_open(ctx);
    let escape =
        !menu_open && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
    if escape {
        if app.dialog.is_some() {
            actions.push(Action::CloseDialog);
        } else if app.picker.is_some() {
            actions.push(Action::ClosePicker);
        } else if !app.pending.is_empty() {
            actions.push(Action::ClearPending);
        } else if app.editing.is_some() {
            actions.push(Action::CancelEdit);
        } else if app.reply_to.is_some() {
            actions.push(Action::CancelReply);
        } else if app.page == Page::Settings {
            actions.push(Action::Open(Page::Chats));
        } else if !app.search.is_empty() {
            actions.push(Action::Search(String::new()));
        }
    }
    // Alt+Up/Down walk the chat list without leaving the composer.
    let step = ctx.input_mut(|input| {
        if input.consume_key(Modifiers::ALT, Key::ArrowDown) {
            1
        } else if input.consume_key(Modifiers::ALT, Key::ArrowUp) {
            -1
        } else {
            0
        }
    });
    if step != 0 {
        let visible = app.visible_chats();
        if !visible.is_empty() {
            let current = app
                .open_chat
                .as_ref()
                .and_then(|open| visible.iter().position(|chat| chat.id == *open));
            let next = match current {
                Some(index) => (index as i64 + step).rem_euclid(visible.len() as i64) as usize,
                None => 0,
            };
            actions.push(Action::OpenChat(visible[next].id.clone()));
        }
    }
    app.actions.extend(actions);
}

/// The shortcuts, for the dialog that lists them.
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl+F / Ctrl+K", "Search chats"),
    ("Alt+↑ / Alt+↓", "Previous / next chat"),
    ("Enter", "Send (Shift+Enter for a new line)"),
    ("Escape", "Close dialog, cancel edit or reply, clear search"),
    ("Ctrl+V", "Paste text, or send a picture from the clipboard"),
    ("Ctrl+B", "Show or hide the chat list"),
    ("Ctrl+End", "Jump to the newest message"),
    ("Ctrl+,", "Settings"),
    ("Ctrl++ / Ctrl+-", "Zoom in / out"),
    ("Ctrl+0", "Reset zoom"),
    ("Ctrl+/", "This list"),
    ("Ctrl+W", "Close the window (Fastsapp stays in the tray)"),
    ("Ctrl+Q", "Quit"),
];

/// A shortcut as the platform writes it: the Command key stands in for
/// Ctrl on macOS, and Option for Alt.
pub fn label(keys: &str) -> String {
    if cfg!(target_os = "macos") {
        keys.replace("Ctrl", "⌘").replace("Alt", "⌥")
    } else {
        keys.to_owned()
    }
}
