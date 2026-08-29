//! The open chat: its header, the messages, and the composer.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use egui::{
    Align, Align2, Color32, CornerRadius, Frame, Key, KeyboardShortcut, Layout, Margin, Modifiers,
    Rect, Sense, Stroke, Vec2, pos2, vec2,
};

use crate::animation;
use crate::app::{App, Conversation};
use crate::markup;
use crate::model::{
    Action, Chat, ChatId, Content, Delivery, Dialog, LinkPreview, Media, MediaState, Message,
    PickerTab,
};
use crate::theme::{self, Icon, Palette};

use super::widgets;

/// Attachments up to this size are fetched as they come into view.
const AUTO_DOWNLOAD_LIMIT: u64 = 64 * 1024 * 1024;
/// The small picture beside a group message.
const SENDER_AVATAR: f32 = 28.0;
const BODY_SIZE: f32 = 14.5;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let Some(chat) = app.current_chat().cloned() else {
        empty(app, ui);
        return;
    };
    header(app, ui, &chat);
    composer(app, ui, &chat);
    messages(app, ui, &chat);
}

fn empty(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let rect = ui.max_rect();
    let center = rect.center() - vec2(0.0, 30.0);
    theme::logo(
        ui,
        center - vec2(0.0, 60.0),
        72.0,
        palette.surface,
        palette.dim,
    );
    ui.painter().text(
        center,
        Align2::CENTER_CENTER,
        "Fastsapp",
        theme::bold(24.0),
        palette.text,
    );
    ui.painter().text(
        center + vec2(0.0, 30.0),
        Align2::CENTER_CENTER,
        if app.chats.is_empty() {
            "Your chats will appear on the left as they arrive."
        } else {
            "Pick a chat on the left to start reading."
        },
        theme::regular(14.0),
        palette.secondary,
    );
    if app.settings.show_shortcut_hints {
        ui.painter().text(
            center + vec2(0.0, 56.0),
            Align2::CENTER_CENTER,
            "Ctrl+K searches chats · Ctrl+/ lists every shortcut",
            theme::regular(12.5),
            palette.dim,
        );
    }
}

fn header(app: &mut App, ui: &mut egui::Ui, chat: &Chat) {
    let palette = app.palette;
    egui::Panel::top("chat-header")
        .show_separator_line(false)
        .frame(
            Frame::new()
                .fill(palette.panel)
                .inner_margin(Margin::symmetric(14, 8)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if !app.sidebar_visible
                    && theme::icon_button(
                        ui,
                        Icon::PanelLeft,
                        18.0,
                        palette.secondary,
                        palette.text,
                        "Show the chat list (Ctrl+B)",
                    )
                    .clicked()
                {
                    app.actions.push(Action::ToggleSidebar);
                }
                let picture = app.avatar(&chat.id);
                let (subtitle, color) = subtitle(app, chat);
                let right_controls = 52.0;
                // The picture, name, and line under it are one click target
                // that opens the info, as on the phone.
                let block = ui
                    .scope(|ui| {
                        ui.horizontal(|ui| {
                            widgets::avatar(
                                ui,
                                &palette,
                                &chat.name,
                                &chat.id,
                                40.0,
                                picture.as_deref(),
                            );
                            ui.add_space(4.0);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                ui.set_max_width((ui.available_width() - right_controls).max(80.0));
                                if subtitle.is_empty() {
                                    ui.add_space(8.0);
                                    widgets::rich_text(
                                        ui,
                                        &chat.name,
                                        theme::semibold(17.0),
                                        palette.text,
                                    );
                                } else {
                                    widgets::rich_text(
                                        ui,
                                        &chat.name,
                                        theme::semibold(15.0),
                                        palette.text,
                                    );
                                    widgets::rich_text(ui, &subtitle, theme::regular(12.5), color);
                                }
                            });
                        });
                    })
                    .response;
                let block = ui
                    .interact(block.rect, ui.id().with("chat-header-info"), Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if block.clicked() {
                    app.actions
                        .push(Action::ShowDialog(Dialog::ChatInfo(chat.id.clone())));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let more = theme::icon_button(
                        ui,
                        Icon::Ellipsis,
                        18.0,
                        palette.secondary,
                        palette.text,
                        "More",
                    );
                    let width = widgets::menu_width(
                        ui,
                        &[
                            "Info",
                            "Pin to top",
                            "Unarchive",
                            "Copy number",
                            "Close chat",
                        ],
                        true,
                    );
                    egui::Popup::menu(&more)
                        .width(width)
                        .frame(widgets::menu_frame(&palette))
                        .show(|ui| {
                            if widgets::menu_item(ui, &palette, Some(Icon::Info), "Info") {
                                app.actions
                                    .push(Action::ShowDialog(Dialog::ChatInfo(chat.id.clone())));
                            }
                            if widgets::menu_item(
                                ui,
                                &palette,
                                Some(if chat.pinned { Icon::PinOff } else { Icon::Pin }),
                                if chat.pinned { "Unpin" } else { "Pin to top" },
                            ) {
                                app.actions
                                    .push(Action::SetPinned(chat.id.clone(), !chat.pinned));
                            }
                            if widgets::menu_item(
                                ui,
                                &palette,
                                Some(Icon::Archive),
                                if chat.archived {
                                    "Unarchive"
                                } else {
                                    "Archive"
                                },
                            ) {
                                app.actions
                                    .push(Action::SetArchived(chat.id.clone(), !chat.archived));
                            }
                            widgets::menu_separator(ui, &palette);
                            if let Some(phone) = chat.phone()
                                && widgets::menu_item(ui, &palette, Some(Icon::Copy), "Copy number")
                            {
                                app.actions.push(Action::CopyText(format!("+{phone}")));
                            }
                            if widgets::menu_item(ui, &palette, Some(Icon::X), "Close chat") {
                                app.actions.push(Action::CloseChat);
                            }
                        });
                });
            });
        });
}

/// What the header says under the name.
fn subtitle(app: &App, chat: &Chat) -> (String, Color32) {
    let palette = app.palette;
    let typing = app.typing_in(&chat.id);
    if !typing.is_empty() {
        let text = if chat.is_group() {
            format!("{} typing…", typing.join(", "))
        } else {
            "typing…".to_owned()
        };
        return (text, palette.accent);
    }
    if chat.is_group() {
        let names = app.participant_names(chat);
        return (
            if names.is_empty() {
                "Group".to_owned()
            } else {
                names
            },
            palette.secondary,
        );
    }
    if let Some(presence) = app.presence.get(&chat.id) {
        if presence.online {
            return ("online".to_owned(), palette.accent);
        }
        if let Some(seen) = presence.last_seen {
            return (
                format!("last seen {}", crate::util::chat_stamp(seen).to_lowercase()),
                palette.secondary,
            );
        }
    }
    match chat.phone() {
        Some(phone) if !app.is_saved_contact(&chat.id) => {
            (crate::util::phone(phone), palette.secondary)
        }
        _ => (String::new(), palette.secondary),
    }
}

fn composer(app: &mut App, ui: &mut egui::Ui, chat: &Chat) {
    let palette = app.palette;
    egui::Panel::bottom("composer")
        .show_separator_line(false)
        .frame(
            Frame::new()
                .fill(palette.panel)
                .inner_margin(Margin::symmetric(12, 8)),
        )
        .show(ui, |ui| {
            if chat.read_only {
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let width = 230.0;
                        ui.add_space((ui.available_width() - width).max(0.0) / 2.0);
                        theme::text(ui, "Only", theme::regular(13.5), palette.secondary);
                        theme::text(ui, "admins", theme::semibold(13.5), palette.accent);
                        theme::text(
                            ui,
                            "can send messages",
                            theme::regular(13.5),
                            palette.secondary,
                        );
                    });
                    ui.add_space(8.0);
                });
                return;
            }
            if app.editing.is_some() {
                edit_strip(app, ui);
            } else if let Some(reply_id) = app.reply_to.clone() {
                let quoted = app
                    .conversations
                    .get(&chat.id)
                    .and_then(|conversation| conversation.message(&reply_id))
                    .cloned();
                match quoted {
                    Some(quoted) => reply_strip(app, ui, &quoted),
                    None => app.reply_to = None,
                }
            }
            let id = egui::Id::new("composer");
            let has_focus = ui.memory(|memory| memory.has_focus(id));
            let enter_sends = app.settings.enter_sends;
            // `consume_key(NONE, Enter)` would also take Shift+Enter, since
            // egui treats an unrequested modifier as fine; the newline
            // needs an exact match.
            let send_key = has_focus
                && ui.input_mut(|input| {
                    let mut sent = false;
                    input.events.retain(|event| {
                        if sent {
                            return true;
                        }
                        let is_send = matches!(
                            event,
                            egui::Event::Key {
                                key: Key::Enter,
                                pressed: true,
                                modifiers,
                                ..
                            } if !modifiers.shift && !modifiers.alt
                                && (if enter_sends { !modifiers.command && !modifiers.ctrl } else { modifiers.command })
                        );
                        sent |= is_send;
                        !is_send
                    });
                    sent
                });
            let mut send_click = false;
            ui.horizontal(|ui| {
                if app.editing.is_none()
                    && theme::icon_button(
                        ui,
                        Icon::Paperclip,
                        20.0,
                        palette.secondary,
                        palette.text,
                        "Send files (or drop them on the window)",
                    )
                    .clicked()
                {
                    app.actions.push(Action::Attach);
                }
                if app.editing.is_none() {
                    let smile = theme::icon_button(
                        ui,
                        Icon::Smile,
                        20.0,
                        if app.picker.is_some() {
                            palette.accent
                        } else {
                            palette.secondary
                        },
                        palette.text,
                        "Emoji, GIFs, and stickers",
                    );
                    app.picker_anchor = Some(smile.rect);
                    if smile.clicked() {
                        app.actions.push(Action::TogglePicker(PickerTab::Emoji));
                    }
                }
                let button_width = 40.0;
                let field_width = ui.available_width() - button_width - 10.0;
                let line_height = ui
                    .painter()
                    .layout_no_wrap("x".to_owned(), theme::regular(BODY_SIZE), palette.text)
                    .size()
                    .y;
                Frame::new()
                    .fill(palette.surface)
                    .corner_radius(CornerRadius::same(theme::RADIUS + 4))
                    .inner_margin(Margin::symmetric(12, 7))
                    .show(ui, |ui| {
                        ui.set_width(field_width - 24.0);
                        // One line to start with; a long message grows the
                        // field a few lines and scrolls beyond that.
                        egui::ScrollArea::vertical()
                            .id_salt("composer-scroll")
                            .max_height(line_height * 6.0)
                            .min_scrolled_height(0.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                let response = ui.add(
                                    egui::TextEdit::multiline(&mut app.composer)
                                        .id(id)
                                        .frame(Frame::NONE)
                                        .margin(Margin::ZERO)
                                        .hint_text(
                                            egui::RichText::new("Type a message")
                                                .color(palette.dim),
                                        )
                                        .font(theme::regular(BODY_SIZE))
                                        .text_color(palette.text)
                                        .desired_rows(1)
                                        .desired_width(f32::INFINITY)
                                        .return_key(if enter_sends {
                                            Some(KeyboardShortcut::new(
                                                Modifiers::SHIFT,
                                                Key::Enter,
                                            ))
                                        } else {
                                            Some(KeyboardShortcut::new(
                                                Modifiers::NONE,
                                                Key::Enter,
                                            ))
                                        }),
                                );
                                if response.changed() {
                                    app.actions.push(Action::Composing {
                                        chat: chat.id.clone(),
                                        composing: true,
                                    });
                                }
                                if app.focus_composer {
                                    app.focus_composer = false;
                                    response.request_focus();
                                }
                            });
                    });
                let ready = !app.composer.trim().is_empty();
                let (fill, hover, icon) = if ready {
                    (palette.accent, palette.accent_hover, palette.on_accent)
                } else {
                    (palette.surface, palette.surface_hover, palette.dim)
                };
                let icon_kind = if app.editing.is_some() {
                    Icon::Check
                } else {
                    Icon::Send
                };
                if theme::circle_button(ui, icon_kind, button_width, fill, hover, icon, "Send")
                    .clicked()
                {
                    send_click = true;
                }
            });
            if (send_key || send_click) && !app.composer.trim().is_empty() {
                let text = std::mem::take(&mut app.composer);
                app.actions.push(Action::SendText {
                    chat: chat.id.clone(),
                    text,
                    quoting: app.reply_to.clone(),
                });
                app.focus_composer = true;
            }
            if app.settings.show_shortcut_hints {
                let hint = if enter_sends {
                    "Enter sends · Shift+Enter for a new line · *bold* _italic_ ~strike~ · Ctrl+V pastes a picture"
                } else {
                    "Ctrl+Enter sends · *bold* _italic_ ~strike~ · Ctrl+V pastes a picture"
                };
                ui.add_space(2.0);
                theme::text(ui, hint, theme::regular(11.0), palette.dim);
            }
        });
}

fn edit_strip(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::icon(ui, Icon::Pencil, 16.0, palette.accent);
                theme::text(ui, "Editing message", theme::semibold(12.5), palette.accent);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::icon_button(
                        ui,
                        Icon::X,
                        16.0,
                        palette.secondary,
                        palette.text,
                        "Stop editing (Esc)",
                    )
                    .clicked()
                    {
                        app.actions.push(Action::CancelEdit);
                    }
                });
            });
        });
    ui.add_space(6.0);
}

fn reply_strip(app: &mut App, ui: &mut egui::Ui, quoted: &Message) {
    let palette = app.palette;
    let who = if quoted.from_me {
        "You".to_owned()
    } else {
        quoted
            .sender_name
            .clone()
            .unwrap_or_else(|| app.display_name(&quoted.sender))
    };
    let summary = markup::plain(&quoted.summary(), &[]);
    Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (bar, _) = ui.allocate_exact_size(vec2(3.0, 34.0), Sense::hover());
                ui.painter().rect_filled(bar, 2.0, palette.accent);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.set_max_width(ui.available_width() - 40.0);
                    widgets::rich_text(
                        ui,
                        &format!("Replying to {who}"),
                        theme::semibold(12.5),
                        palette.accent,
                    );
                    widgets::rich_text(ui, &summary, theme::regular(12.5), palette.secondary);
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::icon_button(
                        ui,
                        Icon::X,
                        16.0,
                        palette.secondary,
                        palette.text,
                        "Cancel reply (Esc)",
                    )
                    .clicked()
                    {
                        app.actions.push(Action::CancelReply);
                    }
                });
            });
        });
    ui.add_space(6.0);
}

/// What the message list needs from the app while the conversation is
/// checked out of it.
struct View<'a> {
    palette: Palette,
    chat: &'a Chat,
    me: Option<&'a str>,
    auto_download: bool,
    /// Pictures beside every incoming message, not only in groups.
    pictures: bool,
    anchor: Option<&'a str>,
    names: &'a dyn Fn(&str) -> String,
    avatars: &'a HashMap<String, Option<PathBuf>>,
    now: i64,
}

fn messages(app: &mut App, ui: &mut egui::Ui, chat: &Chat) {
    let palette = app.palette;
    // Checked out so the rows can be drawn while actions are collected
    // against the rest of the app; put back before anything else runs.
    let mut conversation = app.conversations.remove(&chat.id).unwrap_or_default();
    let typing = app.typing_in(&chat.id);
    let mut avatars = HashMap::new();
    if chat.is_group() || app.settings.show_sender_pictures {
        let senders: HashSet<String> = conversation
            .messages
            .iter()
            .filter(|message| !message.from_me)
            .map(|message| message.sender.clone())
            .collect();
        for sender in senders {
            let picture = app.avatar(&sender);
            avatars.insert(sender, picture);
        }
    }
    let names = |id: &str| app.display_name(id);
    let view = View {
        palette,
        chat,
        me: app.me.as_deref(),
        auto_download: app.settings.auto_download,
        pictures: app.settings.show_sender_pictures,
        anchor: if conversation.loading_older || conversation.fetching_phone {
            None
        } else {
            app.scroll_anchor.as_deref()
        },
        names: &names,
        avatars: &avatars,
        now: crate::util::now(),
    };
    let mut actions = Vec::new();
    let mut anchored = false;
    let scroll_to_bottom = app.scroll_to_bottom;
    let app_pictures = app.settings.show_sender_pictures;
    let output = egui::ScrollArea::vertical()
        .id_salt(("messages", &chat.id))
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            Frame::new()
                .inner_margin(Margin::symmetric(18, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 3.0;
                    top_of_history(ui, &palette, &conversation, chat, &mut actions);
                    let mut previous: Option<&Message> = None;
                    for message in &conversation.messages {
                        let new_day = previous.is_none_or(|previous| {
                            crate::util::day_key(previous.timestamp)
                                != crate::util::day_key(message.timestamp)
                        });
                        if new_day {
                            ui.add_space(8.0);
                            ui.vertical_centered(|ui| {
                                widgets::chip(
                                    ui,
                                    &palette,
                                    &crate::util::day_label(message.timestamp),
                                );
                            });
                            ui.add_space(4.0);
                        }
                        let show_sender = (chat.is_group() || app_pictures)
                            && !message.from_me
                            && (new_day
                                || previous.is_none_or(|previous| {
                                    previous.sender != message.sender || previous.from_me
                                }));
                        if let Some(response) =
                            bubble(ui, &view, message, show_sender, &mut actions)
                            && view.anchor == Some(message.id.as_str())
                        {
                            response.scroll_to_me(Some(Align::Center));
                            anchored = true;
                        }
                        previous = Some(message);
                    }
                    if !typing.is_empty() {
                        typing_bubble(ui, &palette, &typing, chat.is_group());
                    }
                    ui.add_space(4.0);
                    if scroll_to_bottom {
                        // Aim past the end: the offset is clamped to the
                        // content, which leaves the view stuck to the
                        // bottom, so it stays there as pictures and
                        // previews arrive and the content grows.
                        let end = ui.cursor().min + vec2(0.0, 64.0);
                        ui.scroll_to_rect(
                            Rect::from_min_size(end, Vec2::ZERO),
                            Some(Align::BOTTOM),
                        );
                    }
                });
        });
    let at_bottom =
        output.state.offset.y + output.inner_rect.height() >= output.content_size.y - 24.0;
    let complete = conversation.complete;
    let loading = conversation.loading_older;
    let fetching = conversation.fetching_phone;
    let exhausted = conversation.phone_exhausted;
    app.conversations
        .insert(chat.id.clone(), std::mem::take(&mut conversation));
    app.at_bottom = at_bottom;
    if app.scroll_to_bottom {
        app.scroll_to_bottom = false;
    }
    if anchored {
        app.scroll_anchor = None;
    } else if let Some(anchor) = app.scroll_anchor.clone()
        && !loading
        && !fetching
        && !app
            .conversations
            .get(&chat.id)
            .is_some_and(|c| c.message(&anchor).is_some())
    {
        // Asked for a message that is not loaded; nothing to scroll to.
        app.scroll_anchor = None;
    }
    // Reaching the top asks for more, from the archive and then the phone;
    // a chat too short to scroll asks right away.
    let fits = output.content_size.y <= output.inner_rect.height() + 1.0;
    let near_top = output.state.offset.y < 80.0;
    if (near_top || fits) && ((!complete && !loading) || (complete && !fetching && !exhausted)) {
        actions.push(Action::LoadOlder(chat.id.clone()));
    }
    app.actions.extend(actions);
    // A way back down while reading old messages.
    if !at_bottom {
        let rect = output.inner_rect;
        let center = pos2(rect.right() - 34.0, rect.bottom() - 34.0);
        let button = Rect::from_center_size(center, Vec2::splat(40.0));
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(button)
                .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
        );
        let unread = app.chat(&chat.id).map_or(0, |chat| chat.unread);
        if theme::circle_button(
            &mut child,
            Icon::ArrowDown,
            40.0,
            palette.overlay,
            palette.surface_hover,
            palette.text,
            "Newest message",
        )
        .clicked()
        {
            app.actions.push(Action::ScrollToBottom);
        }
        if unread > 0 {
            widgets::badge(
                ui,
                &palette,
                pos2(center.x + 14.0, center.y - 16.0),
                unread,
                false,
            );
        }
    }
}

/// What sits above the first loaded message: a spinner while more is on
/// its way from the archive or the phone.
fn top_of_history(
    ui: &mut egui::Ui,
    palette: &Palette,
    conversation: &Conversation,
    _chat: &Chat,
    _actions: &mut [Action],
) {
    ui.vertical_centered(|ui| {
        if !conversation.complete {
            if conversation.loading_older {
                theme::spinner(ui, 18.0, palette.accent);
            } else {
                ui.add_space(18.0);
            }
        } else if conversation.fetching_phone {
            ui.horizontal(|ui| {
                let width = 260.0;
                ui.add_space((ui.available_width() - width).max(0.0) / 2.0);
                theme::spinner(ui, 16.0, palette.accent);
                theme::text(
                    ui,
                    "Fetching older messages from your phone…",
                    theme::regular(12.5),
                    palette.secondary,
                );
            });
        } else if conversation.messages.is_empty() {
            ui.add_space(24.0);
            widgets::chip(ui, palette, "No messages here yet");
        } else {
            ui.add_space(6.0);
        }
    });
}

fn typing_bubble(ui: &mut egui::Ui, palette: &Palette, typers: &[String], group: bool) {
    Frame::new()
        .fill(palette.bubble_in)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(12, 7))
        .show(ui, |ui| {
            let label = if group {
                format!("{} typing…", typers.join(", "))
            } else {
                "typing…".to_owned()
            };
            widgets::rich_text(ui, &label, theme::regular(13.5), palette.secondary);
        });
}

/// One message, aligned to its side, with the sender's picture in a
/// group. Returns the bubble's response so the caller can scroll to it.
fn bubble(
    ui: &mut egui::Ui,
    view: &View<'_>,
    message: &Message,
    show_sender: bool,
    actions: &mut Vec<Action>,
) -> Option<egui::Response> {
    let own = message.from_me;
    let with_avatar = !own && (view.chat.is_group() || view.pictures);
    let max_width = (ui.available_width() * 0.72).min(560.0)
        - if with_avatar {
            SENDER_AVATAR + 8.0
        } else {
            0.0
        };
    let mut response = None;
    ui.with_layout(
        Layout::top_down(if own { Align::Max } else { Align::Min }),
        |ui| {
            if with_avatar {
                ui.horizontal_top(|ui| {
                    let (rect, avatar) =
                        ui.allocate_exact_size(Vec2::splat(SENDER_AVATAR), Sense::click());
                    if show_sender
                        && avatar
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                    {
                        actions.push(Action::ShowDialog(Dialog::ChatInfo(message.sender.clone())));
                    }
                    if show_sender && ui.is_rect_visible(rect) {
                        let name = message
                            .sender_name
                            .clone()
                            .unwrap_or_else(|| (view.names)(&message.sender));
                        widgets::paint_avatar(
                            ui,
                            &view.palette,
                            rect,
                            name.trim_start_matches('~'),
                            &message.sender,
                            view.avatars
                                .get(&message.sender)
                                .and_then(|picture| picture.as_deref()),
                        );
                    }
                    // The row is horizontal; the bubble's insides are not.
                    ui.vertical(|ui| {
                        response = Some(bubble_frame(
                            ui,
                            view,
                            message,
                            show_sender,
                            max_width,
                            actions,
                        ));
                    });
                });
            } else {
                response = Some(bubble_frame(
                    ui,
                    view,
                    message,
                    show_sender,
                    max_width,
                    actions,
                ));
            }
            if !message.reactions.is_empty() {
                ui.add_space(-7.0);
                ui.horizontal(|ui| {
                    if with_avatar {
                        ui.add_space(SENDER_AVATAR + 8.0);
                    }
                    reactions(ui, view, message, actions);
                });
                ui.add_space(2.0);
            }
        },
    );
    response
}

/// The id of a message's bubble, the same on every frame and knowable from
/// outside, so tests can find the bubble and its menu.
pub fn bubble_id(chat: &str, message: &str) -> egui::Id {
    egui::Id::new(("bubble", chat, message))
}

/// The bubble itself: sender, quote, content, footer, and its menu.
fn bubble_frame(
    ui: &mut egui::Ui,
    view: &View<'_>,
    message: &Message,
    show_sender: bool,
    max_width: f32,
    actions: &mut Vec<Action>,
) -> egui::Response {
    let palette = view.palette;
    let own = message.from_me;
    // A sticker stands on its own, as on the phone; everything else gets a
    // bubble.
    let fill = if matches!(message.content, Content::Sticker { .. }) {
        Color32::TRANSPARENT
    } else if own {
        palette.bubble_out
    } else {
        palette.bubble_in
    };
    // The bubble's own click target is registered before its contents,
    // from where it was last frame, so links, quotes, and attachments
    // inside it stay on top and get their clicks; the bubble keeps the
    // right-click for its menu.
    let bubble_id = bubble_id(&view.chat.id, &message.id);
    let early = ui
        .ctx()
        .read_response(bubble_id)
        .map(|previous| ui.interact(previous.rect, bubble_id, Sense::click()));
    let inner = Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin {
            left: 10,
            right: 10,
            top: 6,
            bottom: 5,
        })
        .show(ui, |ui| {
            ui.set_max_width(max_width);
            ui.spacing_mut().item_spacing.y = 4.0;
            if show_sender && view.chat.is_group() {
                let name = message
                    .sender_name
                    .clone()
                    .unwrap_or_else(|| (view.names)(&message.sender));
                let response = widgets::rich_text(
                    ui,
                    &name,
                    theme::semibold(13.0),
                    palette.sender(crate::util::hue(&message.sender)),
                );
                let response = ui
                    .interact(
                        response.rect,
                        ui.id().with(("sender", &message.id)),
                        Sense::click(),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if response.clicked() {
                    actions.push(Action::ShowDialog(Dialog::ChatInfo(message.sender.clone())));
                }
            }
            if message.forwarded {
                mirrored_row(
                    ui,
                    own,
                    |ui| {
                        theme::icon(ui, Icon::Forward, 14.0, palette.dim);
                    },
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("Forwarded")
                                    .font(theme::regular(12.5))
                                    .italics()
                                    .color(palette.dim),
                            )
                            .selectable(false),
                        );
                    },
                );
            }
            if let Some(quoted) = &message.quoted {
                quote_block(ui, view, message, quoted, actions);
            }
            let reserve = footer_width(ui, message);
            let slot = content(ui, view, message, max_width - 20.0, reserve, actions);
            footer(ui, &palette, message, slot);
        });
    let bubble =
        early.unwrap_or_else(|| ui.interact(inner.response.rect, bubble_id, Sense::click()));
    // The right-click is read from the input rather than from the bubble's
    // response: a quote, a link, or a preview card inside the bubble owns
    // the pointer where it sits, and the menu must open there too.
    // `layer_id_at` only knows floating areas (menus, dialogs, the
    // picker): over the plain chat panel it answers `None`, which is the
    // case that must open the menu; another area on top must not.
    let right_clicked = ui.input(|input| {
        input.pointer.secondary_clicked()
            && input
                .pointer
                .interact_pos()
                .is_some_and(|pos| bubble.rect.contains(pos))
    }) && ui
        .input(|input| input.pointer.interact_pos())
        .is_some_and(|pos| {
            ui.ctx()
                .layer_id_at(pos)
                .is_none_or(|layer| layer == bubble.layer_id)
        });
    let quick = quick_reactions(message).len() as f32;
    let width = widgets::menu_width(
        ui,
        &["Delete for everyone", "Show in folder", "Sent Yesterday"],
        true,
    )
    .max(quick * 36.0 + 12.0);
    egui::Popup::menu(&bubble)
        .open_memory(if right_clicked {
            Some(egui::SetOpenCommand::Bool(true))
        } else if bubble.clicked() {
            Some(egui::SetOpenCommand::Bool(false))
        } else {
            None
        })
        .at_pointer_fixed()
        .width(width)
        .frame(widgets::menu_frame(&palette))
        .show(|ui| {
            context_menu(ui, view, message, actions);
        });
    bubble
}

fn quote_block(
    ui: &mut egui::Ui,
    view: &View<'_>,
    message: &Message,
    quoted: &crate::model::Quoted,
    actions: &mut Vec<Action>,
) {
    let palette = view.palette;
    let own = message.from_me;
    let who = if view.me == Some(quoted.sender.as_str()) {
        "You".to_owned()
    } else {
        quoted
            .sender_name
            .clone()
            .unwrap_or_else(|| (view.names)(&quoted.sender))
    };
    let summary = markup::plain(&quoted.summary, &[]);
    let response = Frame::new()
        .fill(palette.window.gamma_multiply(0.35))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin {
            left: 8,
            right: 10,
            top: 5,
            bottom: 5,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width().min(420.0));
            let bar = |ui: &mut egui::Ui| {
                let (bar, _) = ui.allocate_exact_size(vec2(3.0, 30.0), Sense::hover());
                ui.painter().rect_filled(bar, 2.0, palette.accent);
            };
            let body = |ui: &mut egui::Ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.set_max_width(ui.available_width() - 6.0);
                    widgets::rich_text(ui, &who, theme::semibold(12.5), palette.accent);
                    widgets::rich_text(ui, &summary, theme::regular(12.5), palette.secondary);
                });
            };
            mirrored_row(ui, own, bar, body);
        })
        .response;
    let response = ui
        .interact(
            response.rect,
            ui.id().with(("quote", &message.id, &quoted.id)),
            Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.clicked() {
        actions.push(Action::ScrollTo(quoted.id.clone()));
    }
}

/// A row that reads `first` then `second` in both bubble kinds. An own
/// bubble hugs the right edge, which egui achieves by laying its rows out
/// right to left; the parts are added in reverse there so the eye still
/// reads them left to right.
fn mirrored_row(
    ui: &mut egui::Ui,
    own: bool,
    first: impl FnOnce(&mut egui::Ui),
    second: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        if own {
            second(ui);
            first(ui);
        } else {
            first(ui);
            second(ui);
        }
    });
}

/// How wide the time, ticks, and "edited" of a message are.
fn footer_width(ui: &egui::Ui, message: &Message) -> f32 {
    let font = theme::regular(11.0);
    let time = ui
        .painter()
        .layout_no_wrap(
            crate::util::clock(message.timestamp),
            font.clone(),
            Color32::WHITE,
        )
        .size()
        .x;
    let edited = if message.edited {
        ui.painter()
            .layout_no_wrap("edited".to_owned(), font, Color32::WHITE)
            .size()
            .x
            + 4.0
    } else {
        0.0
    };
    time + edited + if message.from_me { 19.0 } else { 0.0 }
}

/// The time and ticks, hugging the right edge of the bubble. Painted by
/// hand so the row is exactly as wide as the bubble's content: a layout
/// would claim the whole available width and stretch every bubble. When
/// the text left a `slot` beside its last line, the footer sits there.
fn footer(ui: &mut egui::Ui, palette: &Palette, message: &Message, slot: Option<Rect>) {
    let font = theme::regular(11.0);
    let time = ui.painter().layout_no_wrap(
        crate::util::clock(message.timestamp),
        font.clone(),
        palette.secondary,
    );
    let edited = message.edited.then(|| {
        ui.painter()
            .layout_no_wrap("edited".to_owned(), font, palette.dim)
    });
    let tick_width = if message.from_me { 19.0 } else { 0.0 };
    let width =
        time.size().x + edited.as_ref().map_or(0.0, |galley| galley.size().x + 4.0) + tick_width;
    let rect = match slot {
        Some(slot) => slot,
        None => {
            let row_width = ui.min_rect().width().max(width);
            let (rect, _) = ui.allocate_exact_size(vec2(row_width, 15.0), Sense::hover());
            rect
        }
    };
    let mut x = rect.right();
    if message.from_me {
        let ticks = Rect::from_center_size(pos2(x - 7.5, rect.center().y), Vec2::splat(15.0));
        widgets::ticks(ui, palette, ticks, message.status);
        x -= tick_width;
    }
    x -= time.size().x;
    ui.painter().galley(
        pos2(x, rect.center().y - time.size().y / 2.0),
        time,
        palette.secondary,
    );
    if let Some(edited) = edited {
        x -= edited.size().x + 4.0;
        ui.painter().galley(
            pos2(x, rect.center().y - edited.size().y / 2.0),
            edited,
            palette.dim,
        );
    }
}

fn reactions(ui: &mut egui::Ui, view: &View<'_>, message: &Message, actions: &mut Vec<Action>) {
    let palette = view.palette;
    let mut counts: Vec<(String, u32, bool)> = Vec::new();
    for reaction in &message.reactions {
        match counts
            .iter_mut()
            .find(|(emoji, _, _)| *emoji == reaction.emoji)
        {
            Some((_, count, mine)) => {
                *count += 1;
                *mine |= reaction.from_me;
            }
            None => counts.push((reaction.emoji.clone(), 1, reaction.from_me)),
        }
    }
    ui.spacing_mut().item_spacing.x = 3.0;
    for (emoji, count, mine) in counts {
        let label = if count > 1 {
            format!("{emoji} {count}")
        } else {
            emoji.clone()
        };
        let line = widgets::line(ui, &label, theme::regular(13.0), palette.text, 200.0, 1);
        let size = line.size() + vec2(12.0, 6.0);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        if ui.is_rect_visible(rect) {
            ui.painter()
                .rect_filled(rect, rect.height() / 2.0, palette.overlay);
            ui.painter().rect_stroke(
                rect,
                rect.height() / 2.0,
                Stroke::new(1.0, if mine { palette.accent } else { palette.chat }),
                egui::StrokeKind::Inside,
            );
            line.paint(ui, rect.center() - line.size() / 2.0, palette.text);
        }
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        if response.clicked() {
            // Clicking a reaction of ours takes it back; another's adds
            // the same one.
            actions.push(Action::React {
                chat: view.chat.id.clone(),
                message: message.id.clone(),
                emoji: if mine { String::new() } else { emoji },
            });
        }
    }
}

const QUICK_REACTIONS: [&str; 6] = ["👍", "❤️", "😂", "😮", "😢", "🙏"];

/// The reaction we already gave a message, if any.
fn own_reaction(message: &Message) -> Option<&str> {
    message
        .reactions
        .iter()
        .find(|reaction| reaction.from_me)
        .map(|reaction| reaction.emoji.as_str())
}

/// The quick reactions row: the usual six, plus ours when it is another.
fn quick_reactions(message: &Message) -> Vec<&str> {
    let mut list = QUICK_REACTIONS.to_vec();
    if let Some(mine) = own_reaction(message)
        && !list.contains(&mine)
    {
        list.push(mine);
    }
    list
}

fn context_menu(ui: &mut egui::Ui, view: &View<'_>, message: &Message, actions: &mut Vec<Action>) {
    let palette = view.palette;
    let chat = &view.chat.id;
    let mine = own_reaction(message);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for emoji in quick_reactions(message) {
            let chosen = mine == Some(emoji);
            let line = widgets::line(ui, emoji, theme::regular(20.0), palette.text, 40.0, 1);
            let (rect, response) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::click());
            if chosen {
                ui.painter()
                    .circle_filled(rect.center(), 17.0, palette.surface_active);
                ui.painter()
                    .circle_stroke(rect.center(), 16.0, Stroke::new(1.5, palette.accent));
            } else if response.hovered() {
                ui.painter()
                    .circle_filled(rect.center(), 17.0, palette.surface_hover);
            }
            line.paint(ui, rect.center() - line.size() / 2.0, palette.text);
            let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
            let response = if chosen {
                response.on_hover_text("Remove your reaction")
            } else {
                response
            };
            if response.clicked() {
                // Picking our own reaction again takes it back, as on the
                // phone.
                actions.push(Action::React {
                    chat: chat.clone(),
                    message: message.id.clone(),
                    emoji: if chosen {
                        String::new()
                    } else {
                        emoji.to_owned()
                    },
                });
                ui.close();
            }
        }
    });
    widgets::menu_separator(ui, &palette);
    if !matches!(message.content, Content::Revoked)
        && widgets::menu_item(ui, &palette, Some(Icon::Reply), "Reply")
    {
        actions.push(Action::Reply(message.id.clone()));
    }
    let text = match &message.content {
        Content::Text { text, .. } => Some(text.clone()),
        Content::Image { caption, .. }
        | Content::Video { caption, .. }
        | Content::Document { caption, .. } => caption.clone(),
        Content::Location {
            latitude,
            longitude,
            ..
        } => Some(format!("{latitude},{longitude}")),
        Content::Contact { vcard, .. } => Some(vcard.clone()),
        _ => None,
    };
    if let Some(text) = text
        && widgets::menu_item(ui, &palette, Some(Icon::Copy), "Copy text")
    {
        let mentions = mentions_of(view, message);
        actions.push(Action::CopyText(markup::plain(&text, &mentions)));
    }
    let age = view.now - message.timestamp;
    let can_edit = message.from_me
        && matches!(message.content, Content::Text { .. })
        && age <= crate::app::EDIT_WINDOW.as_secs() as i64;
    let can_revoke = message.from_me
        && !matches!(message.content, Content::Revoked)
        && age <= crate::app::REVOKE_WINDOW.as_secs() as i64;
    if can_edit && widgets::menu_item(ui, &palette, Some(Icon::Pencil), "Edit") {
        actions.push(Action::Edit(message.id.clone()));
    }
    if can_revoke && widgets::menu_item(ui, &palette, Some(Icon::Trash), "Delete for everyone") {
        actions.push(Action::DeleteForEveryone(message.id.clone()));
    }
    if widgets::menu_item(ui, &palette, Some(Icon::EyeOff), "Delete for me") {
        actions.push(Action::DeleteForMe(message.id.clone()));
    }
    if let Some(media) = message.content.media() {
        match &media.path {
            Some(path) => {
                if widgets::menu_item(ui, &palette, Some(Icon::ExternalLink), "Open file") {
                    actions.push(Action::OpenFile(path.clone()));
                }
                if let Some(folder) = path.parent()
                    && widgets::menu_item(ui, &palette, Some(Icon::FileText), "Show in folder")
                {
                    actions.push(Action::OpenFile(folder.to_path_buf()));
                }
            }
            None => {
                if widgets::menu_item(ui, &palette, Some(Icon::Download), "Download") {
                    actions.push(Action::Download {
                        chat: chat.clone(),
                        message: message.id.clone(),
                    });
                }
            }
        }
    }
    widgets::menu_separator(ui, &palette);
    if widgets::menu_item(
        ui,
        &palette,
        Some(Icon::Info),
        &format!("Sent {}", crate::util::chat_stamp(message.timestamp)),
    ) {
        actions.push(Action::CopyText(message.id.clone()));
    }
}

fn mentions_of(view: &View<'_>, message: &Message) -> Vec<markup::Mention> {
    message
        .mentions
        .iter()
        .map(|mention| markup::Mention {
            user: mention.user.clone(),
            name: (view.names)(&mention.id),
        })
        .collect()
}

/// Draws the message body. Returns the room beside the last line of text
/// when the footer may sit there.
fn content(
    ui: &mut egui::Ui,
    view: &View<'_>,
    message: &Message,
    width: f32,
    reserve: f32,
    actions: &mut Vec<Action>,
) -> Option<Rect> {
    let palette = view.palette;
    let own = message.from_me;
    match &message.content {
        Content::Text { text, preview } => {
            if let Some(preview) = preview {
                preview_card(ui, view, message, preview, width, actions);
            }
            rich_body(ui, view, message, text, width, Some(reserve), actions)
        }
        Content::Image { caption, media } => {
            picture(ui, view, message, media, width, None, actions);
            caption.as_ref().and_then(|caption| {
                rich_body(ui, view, message, caption, width, Some(reserve), actions)
            })
        }
        Content::Sticker { media, animated } => {
            picture(ui, view, message, media, width, Some(*animated), actions);
            None
        }
        Content::Video {
            caption,
            media,
            seconds,
            gif,
        } => {
            video(ui, view, message, media, *seconds, *gif, width, actions);
            caption.as_ref().and_then(|caption| {
                rich_body(ui, view, message, caption, width, Some(reserve), actions)
            })
        }
        Content::Audio {
            media,
            seconds,
            voice_note,
        } => {
            let title = if *voice_note {
                "Voice message"
            } else {
                "Audio"
            };
            let detail = match seconds {
                Some(seconds) => crate::util::duration(*seconds),
                None => crate::util::bytes(media.size),
            };
            attachment(ui, view, message, media, Icon::Mic, title, &detail, actions);
            None
        }
        Content::Document {
            media,
            file_name,
            caption,
            pages,
        } => {
            let mut detail = Vec::new();
            if let Some(pages) = pages {
                detail.push(format!(
                    "{pages} page{}",
                    if *pages == 1 { "" } else { "s" }
                ));
            }
            detail.push(crate::util::bytes(media.size));
            attachment(
                ui,
                view,
                message,
                media,
                Icon::FileText,
                file_name,
                &detail.join(" · "),
                actions,
            );
            caption.as_ref().and_then(|caption| {
                rich_body(ui, view, message, caption, width, Some(reserve), actions)
            })
        }
        Content::Location {
            latitude,
            longitude,
            name,
            address,
        } => {
            let icon = |ui: &mut egui::Ui| {
                theme::icon(ui, Icon::MapPin, 18.0, palette.accent);
            };
            mirrored_row(ui, own, icon, |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    widgets::rich_text(
                        ui,
                        name.as_deref().unwrap_or("Location"),
                        theme::medium(14.0),
                        palette.text,
                    );
                    if let Some(address) = address {
                        widgets::rich_text(ui, address, theme::regular(12.5), palette.secondary);
                    }
                    if theme::link(ui, "Open in a map", theme::regular(12.5), palette.link)
                        .clicked()
                    {
                        actions.push(Action::OpenUrl(format!(
                            "https://www.openstreetmap.org/?mlat={latitude}&mlon={longitude}#map=16/{latitude}/{longitude}"
                        )));
                    }
                });
            });
            None
        }
        Content::Contact {
            display_name,
            vcard,
        } => {
            let icon = |ui: &mut egui::Ui| {
                theme::icon(ui, Icon::Contact, 18.0, palette.accent);
            };
            mirrored_row(ui, own, icon, |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    widgets::rich_text(ui, display_name, theme::medium(14.0), palette.text);
                    let phone = vcard
                        .lines()
                        .find(|line| line.starts_with("TEL"))
                        .and_then(|line| line.rsplit(':').next())
                        .map(str::trim)
                        .unwrap_or("");
                    if !phone.is_empty() {
                        theme::text(ui, phone, theme::regular(12.5), palette.secondary);
                    }
                });
            });
            None
        }
        Content::Poll { question, options } => {
            // A poll reads left to right on either side, as on the phone.
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                ui.set_max_width(width);
                widgets::rich_text(ui, question, theme::semibold(14.0), palette.text);
                for option in options {
                    ui.horizontal(|ui| {
                        theme::icon(ui, Icon::CircleCheck, 14.0, palette.dim);
                        widgets::rich_text(ui, option, theme::regular(13.5), palette.text);
                    });
                }
                theme::text(ui, "Vote on your phone", theme::regular(11.5), palette.dim);
            });
            None
        }
        Content::Revoked => {
            mirrored_row(
                ui,
                own,
                |ui| {
                    theme::icon(ui, Icon::Ban, 14.0, palette.dim);
                },
                |ui| {
                    theme::text(
                        ui,
                        "This message was deleted",
                        theme::regular(13.5),
                        palette.secondary,
                    );
                },
            );
            None
        }
        Content::Unsupported { what } => {
            mirrored_row(
                ui,
                own,
                |ui| {
                    theme::icon(ui, Icon::CircleAlert, 14.0, palette.dim);
                },
                |ui| {
                    theme::text(
                        ui,
                        format!("Unsupported: {what}"),
                        theme::regular(13.5),
                        palette.secondary,
                    );
                },
            );
            None
        }
    }
}

/// A run of message text with WhatsApp's markup, links, mentions, and
/// emoji. With `reserve`, the text leaves that much room beside its last
/// line for the footer when the line is short enough, and returns where.
fn rich_body(
    ui: &mut egui::Ui,
    view: &View<'_>,
    message: &Message,
    text: &str,
    width: f32,
    reserve: Option<f32>,
    actions: &mut Vec<Action>,
) -> Option<Rect> {
    let palette = view.palette;
    let mentions = mentions_of(view, message);
    let style = markup::Style {
        size: BODY_SIZE,
        color: palette.text,
        secondary: palette.secondary,
        link: palette.link,
        mention: palette.accent,
    };
    let laid = markup::layout(ui, text, &mentions, &style, width);
    let size = laid.galley.size();
    let last_row = laid.galley.rows.last().map_or(0.0, |row| row.row.size.x);
    let inline = reserve.filter(|reserve| last_row + 8.0 + reserve <= width);
    let allocation = match inline {
        Some(reserve) => vec2(size.x.max(last_row + 8.0 + reserve), size.y),
        None => size,
    };
    let sense = if laid.links.is_empty() {
        Sense::hover()
    } else {
        Sense::click()
    };
    let (rect, response) = ui.allocate_exact_size(allocation, sense);
    if ui.is_rect_visible(rect) {
        markup::paint(ui, &laid, rect.min, palette.text);
    }
    if !laid.links.is_empty()
        && let Some(pos) = response.hover_pos()
    {
        let cursor = laid.galley.cursor_from_pos(pos - rect.min);
        if let Some(url) = laid.link_at(cursor.index.0) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            if response.clicked() {
                actions.push(Action::OpenUrl(url.to_owned()));
            }
        }
    }
    inline.map(|reserve| {
        Rect::from_min_max(
            pos2(rect.right() - reserve, rect.bottom() - 15.0),
            rect.right_bottom(),
        )
    })
}

/// The card WhatsApp builds for a link: picture, title, description.
fn preview_card(
    ui: &mut egui::Ui,
    view: &View<'_>,
    message: &Message,
    preview: &LinkPreview,
    width: f32,
    actions: &mut Vec<Action>,
) {
    let palette = view.palette;
    let own = message.from_me;
    let thumbnail = message
        .thumbnail
        .as_deref()
        .map(|bytes| thumbnail_uri(ui.ctx(), &message.chat, &message.id, bytes));
    let domain = preview
        .url
        .split("://")
        .nth(1)
        .unwrap_or(&preview.url)
        .split('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    let response = Frame::new()
        .fill(palette.window.gamma_multiply(0.35))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(width.clamp(200.0, 420.0));
            let picture = |ui: &mut egui::Ui| {
                if let Some(uri) = &thumbnail {
                    ui.add(
                        egui::Image::new(uri)
                            .fit_to_exact_size(Vec2::splat(64.0))
                            .corner_radius(4.0),
                    );
                }
            };
            let text = |ui: &mut egui::Ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.set_max_width(ui.available_width());
                    if let Some(title) = &preview.title {
                        widgets::rich_text(ui, title, theme::semibold(13.5), palette.text);
                    }
                    if let Some(description) = &preview.description {
                        let line = widgets::line(
                            ui,
                            description,
                            theme::regular(12.5),
                            palette.secondary,
                            ui.available_width(),
                            2,
                        );
                        let (rect, _) = ui.allocate_exact_size(line.size(), Sense::hover());
                        line.paint(ui, rect.min, palette.secondary);
                    }
                    theme::text(ui, &domain, theme::regular(12.0), palette.dim);
                });
            };
            mirrored_row(ui, own, picture, text);
        })
        .response;
    let response = ui
        .interact(
            response.rect,
            ui.id().with(("preview", &message.id)),
            Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.clicked() {
        actions.push(Action::OpenUrl(preview.url.clone()));
    }
}

/// Every thumbnail handed to egui so far, per context, so the bytes are
/// registered once rather than copied every frame.
#[derive(Clone, Default)]
struct Thumbnails(Arc<Mutex<HashSet<String>>>);

fn thumbnail_uri(ctx: &egui::Context, chat: &str, id: &str, bytes: &[u8]) -> String {
    let uri = format!(
        "bytes://thumb-{}-{}",
        chat.chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>(),
        id
    );
    let known: Thumbnails = ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Thumbnails>(egui::Id::new("thumbnails"))
            .clone()
    });
    let fresh = known
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(uri.clone());
    if fresh {
        ctx.include_bytes(uri.clone(), bytes.to_vec());
    }
    uri
}

/// WhatsApp shows pictures up to about this wide, and tall ones taller
/// than wide.
const PICTURE_WIDTH: f32 = 340.0;
const PICTURE_HEIGHT: f32 = 440.0;
const STICKER_SIDE: f32 = 180.0;

/// A size for a `width`×`height` picture within the given bounds, never
/// blown up past its own size and never below a readable minimum.
fn fit_picture(width: f32, height: f32, max_width: f32, max_height: f32) -> Vec2 {
    let (width, height) = if width > 0.0 && height > 0.0 {
        (width, height)
    } else {
        (4.0, 3.0)
    };
    let scale = (max_width / width).min(max_height / height).min(1.0);
    let scale = if width * scale < 120.0 {
        (120.0 / width).min(max_width / width)
    } else {
        scale
    };
    vec2(width * scale, (height * scale).max(90.0))
}

/// A sticker fills its square, whatever the file's own size: stickers are
/// drawn at one size on the phone too.
fn fit_sticker(width: f32, height: f32) -> Vec2 {
    let (width, height) = if width > 0.0 && height > 0.0 {
        (width, height)
    } else {
        (1.0, 1.0)
    };
    let scale = (STICKER_SIDE / width).min(STICKER_SIDE / height);
    vec2(width * scale, height * scale)
}

/// The box a picture or video occupies before and after download.
fn frame_size(media: &Media, thumbnail_hint: Option<(u32, u32)>, limit: f32) -> Vec2 {
    let (w, h) = match (media.width, media.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w as f32, h as f32),
        _ => match thumbnail_hint {
            Some((w, h)) if w > 0 && h > 0 => (w as f32, h as f32),
            _ => (4.0, 3.0),
        },
    };
    fit_picture(w, h, limit, PICTURE_HEIGHT.min(limit * 1.3))
}

/// A picture or sticker: the file once fetched, playing if it moves; the
/// blurred preview with a download mark until then. `sticker` carries
/// whether the sticker is animated.
fn picture(
    ui: &mut egui::Ui,
    view: &View<'_>,
    message: &Message,
    media: &Media,
    width: f32,
    sticker: Option<bool>,
    actions: &mut Vec<Action>,
) {
    let palette = view.palette;
    let (max_width, max_height) = match sticker {
        Some(_) => (STICKER_SIDE, STICKER_SIDE),
        None => (width.min(PICTURE_WIDTH), PICTURE_HEIGHT),
    };
    if let Some(path) = &media.path {
        let animated = sticker == Some(true);
        let playing = animated.then(|| animation::frame(ui.ctx(), path));
        if let Some(animation::Frame::Ready(texture)) = &playing {
            let size = texture.size_vec2();
            let size = fit_sticker(size.x, size.y);
            let (rect, response) = ui.allocate_exact_size(size, Sense::click());
            if ui.is_rect_visible(rect) {
                ui.painter().image(
                    texture.id(),
                    rect,
                    Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            if response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                actions.push(Action::OpenFile(path.clone()));
            }
            return;
        }
        let image = egui::Image::new(file_uri(path));
        match image.load_for_size(ui.ctx(), vec2(max_width, max_height)) {
            Ok(egui::load::TexturePoll::Ready { texture }) => {
                let size = if sticker.is_some() {
                    fit_sticker(texture.size.x, texture.size.y)
                } else {
                    fit_picture(texture.size.x, texture.size.y, max_width, max_height)
                };
                let response = ui.add(
                    image
                        .fit_to_exact_size(size)
                        .corner_radius(if sticker.is_some() { 0.0 } else { 6.0 })
                        .sense(Sense::click()),
                );
                if response
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    actions.push(Action::OpenFile(path.clone()));
                }
            }
            Ok(egui::load::TexturePoll::Pending { .. }) => {
                let size = if sticker.is_some() {
                    Vec2::splat(STICKER_SIDE)
                } else {
                    frame_size(media, None, max_width)
                };
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                if ui.is_rect_visible(rect) {
                    ui.painter().rect_filled(rect, 6.0, palette.surface);
                    theme::paint_spinner(ui, rect, 22.0, palette.accent);
                }
            }
            Err(_) => {
                let size = if sticker.is_some() {
                    Vec2::splat(STICKER_SIDE)
                } else {
                    frame_size(media, None, max_width)
                };
                let (rect, response) = ui.allocate_exact_size(size, Sense::click());
                if ui.is_rect_visible(rect) {
                    ui.painter().rect_filled(rect, 6.0, palette.surface);
                    theme::paint_icon(ui, Icon::CircleAlert, rect, 24.0, palette.danger);
                    ui.painter().text(
                        rect.center() + vec2(0.0, 24.0),
                        Align2::CENTER_CENTER,
                        "Cannot show this picture; click to open it",
                        theme::regular(11.5),
                        palette.secondary,
                    );
                }
                if response.clicked() {
                    actions.push(Action::OpenFile(path.clone()));
                }
            }
        }
        return;
    }
    let size = frame_size(media, None, max_width);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let thumbnail = message
            .thumbnail
            .as_deref()
            .filter(|_| sticker.is_none())
            .map(|bytes| thumbnail_uri(ui.ctx(), &message.chat, &message.id, bytes));
        match thumbnail {
            Some(uri) => {
                egui::Image::new(uri)
                    .fit_to_exact_size(size)
                    .corner_radius(6.0)
                    .paint_at(ui, rect);
                ui.painter()
                    .rect_filled(rect, 6.0, Color32::from_black_alpha(60));
            }
            None => {
                ui.painter().rect_filled(rect, 6.0, palette.surface);
            }
        }
        let disc = Rect::from_center_size(rect.center(), Vec2::splat(44.0));
        match &media.state {
            MediaState::Downloading => {
                ui.painter()
                    .circle_filled(disc.center(), 22.0, Color32::from_black_alpha(120));
                theme::paint_spinner(ui, disc, 22.0, Color32::WHITE);
            }
            MediaState::Failed(_) => {
                ui.painter()
                    .circle_filled(disc.center(), 22.0, Color32::from_black_alpha(120));
                theme::paint_icon(ui, Icon::CircleAlert, disc, 22.0, palette.danger);
                ui.painter().text(
                    rect.center() + vec2(0.0, 34.0),
                    Align2::CENTER_CENTER,
                    "Could not download; click to retry",
                    theme::regular(11.5),
                    Color32::WHITE,
                );
            }
            MediaState::Idle => {
                ui.painter()
                    .circle_filled(disc.center(), 22.0, Color32::from_black_alpha(120));
                theme::paint_icon(
                    ui,
                    if sticker.is_some() {
                        Icon::Sticker
                    } else {
                        Icon::Download
                    },
                    disc,
                    22.0,
                    Color32::WHITE,
                );
                if sticker.is_none() {
                    ui.painter().text(
                        rect.center() + vec2(0.0, 34.0),
                        Align2::CENTER_CENTER,
                        crate::util::bytes(media.size),
                        theme::regular(11.5),
                        Color32::WHITE,
                    );
                }
            }
        }
    }
    let wants = response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
        && !matches!(media.state, MediaState::Downloading);
    let auto = ui.is_rect_visible(rect)
        && matches!(media.state, MediaState::Idle)
        && (sticker.is_some() || (view.auto_download && media.size <= AUTO_DOWNLOAD_LIMIT));
    if wants || auto {
        actions.push(Action::Download {
            chat: view.chat.id.clone(),
            message: message.id.clone(),
        });
    }
}

/// A video: its poster with a play mark and length, fetched on a click and
/// opened with the desktop's player.
#[allow(clippy::too_many_arguments)]
fn video(
    ui: &mut egui::Ui,
    view: &View<'_>,
    message: &Message,
    media: &Media,
    seconds: Option<u32>,
    gif: bool,
    width: f32,
    actions: &mut Vec<Action>,
) {
    let palette = view.palette;
    let Some(thumbnail) = message.thumbnail.as_deref() else {
        let title = if gif { "GIF" } else { "Video" };
        let mut detail = Vec::new();
        if let Some(seconds) = seconds {
            detail.push(crate::util::duration(seconds));
        }
        detail.push(crate::util::bytes(media.size));
        attachment(
            ui,
            view,
            message,
            media,
            Icon::Video,
            title,
            &detail.join(" · "),
            actions,
        );
        return;
    };
    let uri = thumbnail_uri(ui.ctx(), &message.chat, &message.id, thumbnail);
    let size = frame_size(media, Some((16, 9)), width.min(PICTURE_WIDTH));
    // A GIF plays in place once it is here; other videos keep their poster.
    let playing = match (&media.path, gif) {
        (Some(path), true) => Some(animation::frame(ui.ctx(), path)),
        _ => None,
    };
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if let Some(animation::Frame::Ready(texture)) = &playing {
        if ui.is_rect_visible(rect) {
            ui.painter().image(
                texture.id(),
                rect,
                Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        if response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
            && let Some(path) = &media.path
        {
            actions.push(Action::OpenFile(path.clone()));
        }
        return;
    }
    if ui.is_rect_visible(rect) {
        egui::Image::new(uri)
            .fit_to_exact_size(size)
            .corner_radius(6.0)
            .paint_at(ui, rect);
        ui.painter()
            .rect_filled(rect, 6.0, Color32::from_black_alpha(40));
        let disc = Rect::from_center_size(rect.center(), Vec2::splat(48.0));
        ui.painter()
            .circle_filled(disc.center(), 24.0, Color32::from_black_alpha(140));
        match (&media.path, &media.state) {
            (Some(_), _) if matches!(playing, Some(animation::Frame::Pending)) => {
                theme::paint_spinner(ui, disc, 24.0, Color32::WHITE)
            }
            (Some(_), _) => theme::paint_icon(ui, Icon::ExternalLink, disc, 22.0, Color32::WHITE),
            (None, MediaState::Downloading) => theme::paint_spinner(ui, disc, 24.0, Color32::WHITE),
            (None, MediaState::Failed(_)) => {
                theme::paint_icon(ui, Icon::CircleAlert, disc, 22.0, palette.danger)
            }
            (None, MediaState::Idle) => {
                theme::paint_icon(ui, Icon::Play, disc, 22.0, Color32::WHITE)
            }
        }
        let mut label = Vec::new();
        if gif {
            label.push("GIF".to_owned());
        }
        if let Some(seconds) = seconds {
            label.push(crate::util::duration(seconds));
        }
        if media.path.is_none() {
            label.push(crate::util::bytes(media.size));
        }
        if !label.is_empty() {
            let galley =
                ui.painter()
                    .layout_no_wrap(label.join(" · "), theme::medium(11.5), Color32::WHITE);
            let chip = Rect::from_min_size(
                pos2(rect.left() + 8.0, rect.bottom() - galley.size().y - 14.0),
                galley.size() + vec2(12.0, 6.0),
            );
            ui.painter()
                .rect_filled(chip, chip.height() / 2.0, Color32::from_black_alpha(140));
            ui.painter()
                .galley(chip.min + vec2(6.0, 3.0), galley, Color32::WHITE);
        }
    }
    let auto = ui.is_rect_visible(rect)
        && media.path.is_none()
        && matches!(media.state, MediaState::Idle)
        && view.auto_download
        && media.size <= AUTO_DOWNLOAD_LIMIT;
    if auto {
        actions.push(Action::Download {
            chat: view.chat.id.clone(),
            message: message.id.clone(),
        });
    } else if response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        match &media.path {
            Some(path) => actions.push(Action::OpenFile(path.clone())),
            None if !matches!(media.state, MediaState::Downloading) => {
                actions.push(Action::Download {
                    chat: view.chat.id.clone(),
                    message: message.id.clone(),
                })
            }
            None => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn attachment(
    ui: &mut egui::Ui,
    view: &View<'_>,
    message: &Message,
    media: &Media,
    icon: Icon,
    title: &str,
    detail: &str,
    actions: &mut Vec<Action>,
) {
    let palette = view.palette;
    let own = message.from_me;
    let response = Frame::new()
        .fill(palette.window.gamma_multiply(0.35))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width().clamp(200.0, 360.0));
            let disc = |ui: &mut egui::Ui| {
                let (disc, _) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::hover());
                ui.painter().circle_filled(
                    disc.center(),
                    18.0,
                    palette.accent.gamma_multiply(0.25),
                );
                theme::paint_icon(ui, icon, disc, 18.0, palette.accent);
            };
            let action = |ui: &mut egui::Ui| match (&media.path, &media.state) {
                (Some(_), _) => {
                    theme::icon(ui, Icon::ExternalLink, 18.0, palette.secondary);
                }
                (None, MediaState::Downloading) => {
                    theme::spinner(ui, 18.0, palette.accent);
                }
                (None, _) => {
                    theme::icon(ui, Icon::Download, 18.0, palette.secondary);
                }
            };
            let column = |ui: &mut egui::Ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.set_width(ui.available_width() - 34.0);
                    widgets::rich_text(ui, title, theme::medium(14.0), palette.text);
                    let detail = match &media.state {
                        MediaState::Failed(error) => format!("Failed: {error}"),
                        _ => detail.to_owned(),
                    };
                    theme::text(ui, detail, theme::regular(12.0), palette.secondary);
                });
            };
            ui.horizontal(|ui| {
                if own {
                    action(ui);
                    column(ui);
                    disc(ui);
                } else {
                    disc(ui);
                    column(ui);
                    action(ui);
                }
            });
        })
        .response;
    let auto = ui.is_rect_visible(response.rect)
        && media.path.is_none()
        && matches!(media.state, MediaState::Idle)
        && view.auto_download
        && media.size <= AUTO_DOWNLOAD_LIMIT;
    if auto {
        actions.push(Action::Download {
            chat: view.chat.id.clone(),
            message: message.id.clone(),
        });
    }
    let response = ui
        .interact(
            response.rect,
            ui.id().with(("attachment", &message.id)),
            Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.clicked() && !auto {
        match &media.path {
            Some(path) => actions.push(Action::OpenFile(path.clone())),
            None if !matches!(media.state, MediaState::Downloading) => {
                actions.push(Action::Download {
                    chat: view.chat.id.clone(),
                    message: message.id.clone(),
                })
            }
            None => {}
        }
    }
}

fn file_uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

/// Whether a chat's conversation has anything to show; for tests.
#[allow(dead_code)]
pub fn has_messages(conversation: &Conversation) -> bool {
    !conversation.messages.is_empty()
}

#[allow(dead_code)]
fn status_label(status: Delivery) -> &'static str {
    match status {
        Delivery::None => "",
        Delivery::Pending => "sending",
        Delivery::Sent => "sent",
        Delivery::Delivered => "delivered",
        Delivery::Read => "read",
        Delivery::Played => "played",
        Delivery::Failed => "failed",
    }
}

#[allow(dead_code)]
fn chat_of(chat: &ChatId) -> &str {
    chat
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(w: Option<u32>, h: Option<u32>) -> Media {
        Media {
            mime: "image/jpeg".into(),
            size: 1,
            width: w,
            height: h,
            path: None,
            state: MediaState::Idle,
        }
    }

    #[test]
    fn picture_frames_keep_their_shape_within_the_limit() {
        let landscape = frame_size(&media(Some(1600), Some(1200)), None, 340.0);
        assert!((landscape.x - 340.0).abs() < 0.01);
        assert!((landscape.y - 255.0).abs() < 0.01);
        let tall = frame_size(&media(Some(600), Some(1200)), None, 340.0);
        assert!(tall.y > 340.0 && tall.y <= PICTURE_HEIGHT);
        let exact = fit_picture(1200.0, 1600.0, PICTURE_WIDTH, PICTURE_HEIGHT);
        assert!((exact.y - PICTURE_HEIGHT).abs() < 0.01);
        assert!(exact.x < PICTURE_WIDTH);
        let unknown = frame_size(&media(None, None), Some((16, 9)), 340.0);
        assert!(unknown.x > unknown.y);
        let tiny = frame_size(&media(Some(40), Some(40)), None, 340.0);
        assert!(tiny.x >= 120.0);
    }
}

#[cfg(test)]
mod reaction_tests {
    use super::*;
    use crate::model::{Delivery, Reaction};

    fn with_reactions(reactions: Vec<Reaction>) -> Message {
        Message {
            id: "m1".into(),
            chat: "a@s.whatsapp.net".into(),
            sender: "a@s.whatsapp.net".into(),
            sender_name: None,
            from_me: false,
            timestamp: 0,
            content: Content::text("hi"),
            status: Delivery::None,
            quoted: None,
            reactions,
            edited: false,
            mentions: Vec::new(),
            forwarded: false,
            thumbnail: None,
        }
    }

    fn reaction(from_me: bool, emoji: &str) -> Reaction {
        Reaction {
            sender: if from_me { "me" } else { "them" }.into(),
            from_me,
            emoji: emoji.into(),
        }
    }

    #[test]
    fn only_our_own_reaction_counts_as_chosen() {
        let message = with_reactions(vec![reaction(false, "😂"), reaction(true, "❤️")]);
        assert_eq!(own_reaction(&message), Some("❤️"));
        assert_eq!(quick_reactions(&message), QUICK_REACTIONS.to_vec());
        assert_eq!(
            own_reaction(&with_reactions(vec![reaction(false, "😂")])),
            None
        );
    }

    #[test]
    fn an_unusual_reaction_of_ours_joins_the_quick_row() {
        let message = with_reactions(vec![reaction(true, "🦀")]);
        let quick = quick_reactions(&message);
        assert_eq!(quick.len(), QUICK_REACTIONS.len() + 1);
        assert_eq!(quick.last(), Some(&"🦀"));
    }
}
