//! The left panel: the chat list.

use egui::{Align, Frame, Layout, Margin, Rect, Sense, Vec2, pos2, vec2};

use crate::app::App;
use crate::model::{Action, Chat, Dialog, Page};
use crate::theme::{self, Icon, Palette};

use super::widgets;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let panel = egui::Panel::left("chats")
        .resizable(true)
        .default_size(app.settings.sidebar_width)
        .size_range(260.0..=520.0)
        .show_separator_line(false)
        .frame(Frame::new().fill(palette.panel).inner_margin(Margin::ZERO));
    let response = panel.show(ui, |ui| {
        header(app, ui);
        list(app, ui);
    });
    let width = response.response.rect.width();
    if (width - app.settings.sidebar_width).abs() > 1.0 {
        app.settings.sidebar_width = width;
        app.actions.push(Action::SettingsChanged);
    }
    // The panel's right edge, so the list reads as its own surface.
    let rect = response.response.rect;
    ui.painter().vline(
        rect.right(),
        rect.y_range(),
        egui::Stroke::new(1.0, palette.outline),
    );
}

fn header(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    Frame::new()
        .inner_margin(Margin {
            left: 14,
            right: 10,
            top: 12,
            bottom: 8,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if app.show_archived {
                    if theme::icon_button(
                        ui,
                        Icon::ArrowLeft,
                        18.0,
                        palette.secondary,
                        palette.text,
                        "Back to chats",
                    )
                    .clicked()
                    {
                        app.show_archived = false;
                    }
                    theme::text(ui, "Archived", theme::bold(20.0), palette.text);
                } else {
                    let me = app.me.clone().unwrap_or_default();
                    let name = app.me_name.clone().unwrap_or_else(|| "You".to_owned());
                    let picture = app.avatar(&me);
                    let tooltip = match &app.me_about {
                        Some(about) => format!("{name}\n{about}"),
                        None => name.clone(),
                    };
                    let response =
                        widgets::avatar(ui, &palette, &name, &me, 34.0, picture.as_deref())
                            .interact(Sense::click())
                            .on_hover_text(tooltip)
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if response.clicked() {
                        app.actions.push(Action::Open(Page::Settings));
                    }
                    ui.add_space(2.0);
                    theme::text(ui, "Chats", theme::bold(20.0), palette.text);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::icon_button(
                        ui,
                        Icon::Settings,
                        18.0,
                        palette.secondary,
                        palette.text,
                        "Settings (Ctrl+,)",
                    )
                    .clicked()
                    {
                        app.actions.push(Action::Open(Page::Settings));
                    }
                    if theme::icon_button(
                        ui,
                        Icon::PanelLeft,
                        18.0,
                        palette.secondary,
                        palette.text,
                        "Hide the chat list (Ctrl+B)",
                    )
                    .clicked()
                    {
                        app.actions.push(Action::ToggleSidebar);
                    }
                });
            });
            ui.add_space(6.0);
            let id = egui::Id::new("chat-search");
            let width = ui.available_width();
            let mut text = app.search.clone();
            let response =
                widgets::search_field(ui, &palette, id, &mut text, "Search chats", width);
            if text != app.search {
                app.actions.push(Action::Search(text));
            }
            if app.focus_search {
                app.focus_search = false;
                response.request_focus();
            }
        });
}

fn list(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let chats: Vec<Chat> = app.visible_chats().into_iter().cloned().collect();
    let archived = app.archived_count();
    let show_archive_row = !app.show_archived && archived > 0 && app.search.trim().is_empty();
    if chats.is_empty() && !show_archive_row {
        let (title, body) = if !app.search.trim().is_empty() {
            ("No chats match", "Try another name or number.")
        } else if app.show_archived {
            ("Nothing archived", "Archived chats will show up here.")
        } else if app.syncing {
            (
                "Bringing in your chats",
                "History is on its way from your phone.",
            )
        } else {
            (
                "No chats yet",
                "Messages arrive here as they come in. Start a chat from your phone.",
            )
        };
        widgets::empty_state(ui, &palette, Icon::MessageCircle, title, body);
        return;
    }
    let row_height = theme::ROW_HEIGHT;
    let total = chats.len() + usize::from(show_archive_row);
    egui::ScrollArea::vertical()
        .id_salt("chat-list")
        .auto_shrink([false, false])
        .show_rows(ui, row_height, total, |ui, range| {
            for index in range {
                if show_archive_row && index == 0 {
                    archive_row(app, ui, archived);
                    continue;
                }
                let chat = &chats[index - usize::from(show_archive_row)];
                // Keyed by the chat, not the row: an open menu follows its
                // chat when the list reorders under it.
                ui.push_id(("chat", &chat.id), |ui| row(app, ui, chat));
            }
        });
}

fn archive_row(app: &mut App, ui: &mut egui::Ui, count: usize) {
    let palette = app.palette;
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::ROW_HEIGHT),
        Sense::click(),
    );
    if ui.is_rect_visible(rect) {
        if response.hovered() {
            ui.painter().rect_filled(rect, 0.0, palette.surface_hover);
        }
        let icon_rect =
            Rect::from_center_size(pos2(rect.left() + 38.0, rect.center().y), Vec2::splat(22.0));
        Icon::Archive
            .image(palette.accent, 22.0)
            .paint_at(ui, icon_rect);
        ui.painter().text(
            pos2(rect.left() + 76.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            "Archived",
            theme::medium(14.5),
            palette.text,
        );
        ui.painter().text(
            pos2(rect.right() - 16.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            count.to_string(),
            theme::regular(12.5),
            palette.accent,
        );
        ui.painter().hline(
            (rect.left() + 76.0)..=rect.right(),
            rect.bottom() - 0.5,
            egui::Stroke::new(1.0, palette.outline),
        );
    }
    if response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        app.show_archived = true;
    }
}

fn row(app: &mut App, ui: &mut egui::Ui, chat: &Chat) {
    let palette = app.palette;
    let title = app.chat_title(chat);
    let selected = app.open_chat.as_deref() == Some(chat.id.as_str());
    let now = crate::util::now();
    let muted = chat.muted(now);
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::ROW_HEIGHT),
        Sense::click(),
    );
    if ui.is_rect_visible(rect) {
        if selected {
            ui.painter().rect_filled(rect, 0.0, palette.surface_active);
        } else if response.hovered() {
            ui.painter().rect_filled(rect, 0.0, palette.surface_hover);
        }
        let avatar_rect =
            Rect::from_center_size(pos2(rect.left() + 38.0, rect.center().y), Vec2::splat(48.0));
        let picture = app.avatar(&chat.id);
        widgets::paint_avatar(
            ui,
            &palette,
            avatar_rect,
            &title,
            &chat.id,
            picture.as_deref(),
        );

        let left = rect.left() + 76.0;
        let right = rect.right() - 14.0;
        let stamp = if chat.last_activity > 0 {
            crate::util::chat_stamp(chat.last_activity)
        } else {
            String::new()
        };
        let unread = chat.unread > 0;
        let stamp_color = if unread && !muted {
            palette.accent
        } else {
            palette.dim
        };
        let stamp_galley = ui
            .painter()
            .layout_no_wrap(stamp, theme::regular(11.5), stamp_color);
        let name_top = rect.top() + 14.0;
        ui.painter().galley(
            pos2(right - stamp_galley.size().x, name_top + 1.0),
            stamp_galley.clone(),
            stamp_color,
        );
        let name_width = (right - stamp_galley.size().x - 8.0 - left).max(0.0);
        let name_font = if unread {
            theme::semibold(14.5)
        } else {
            theme::medium(14.5)
        };
        let name = widgets::line(ui, &title, name_font, palette.text, name_width, 1);
        name.paint(ui, pos2(left, name_top), palette.text);

        // The second line: what the newest message says, and who it is
        // from in a group, with room for the badges on the right.
        let mut badge_right = right;
        let line_y = rect.top() + 38.0;
        if unread {
            let width = widgets::badge(
                ui,
                &palette,
                pos2(badge_right - 10.0, line_y + 8.0),
                chat.unread,
                muted,
            );
            badge_right -= width + 6.0;
        }
        if muted {
            let icon_rect =
                Rect::from_center_size(pos2(badge_right - 8.0, line_y + 8.0), Vec2::splat(15.0));
            Icon::VolumeX
                .image(palette.dim, 15.0)
                .paint_at(ui, icon_rect);
            badge_right -= 20.0;
        }
        if chat.pinned {
            let icon_rect =
                Rect::from_center_size(pos2(badge_right - 8.0, line_y + 8.0), Vec2::splat(14.0));
            Icon::Pin.image(palette.dim, 14.0).paint_at(ui, icon_rect);
            badge_right -= 20.0;
        }
        let mut x = left;
        let typing = app.typing_in(&chat.id);
        let preview_color = if unread && !muted {
            palette.secondary
        } else {
            palette.dim
        };
        let preview = if !typing.is_empty() {
            let who = if chat.is_group() {
                format!("{} is typing…", typing[0].trim_start_matches('~'))
            } else {
                "typing…".to_owned()
            };
            widgets::line(
                ui,
                &who,
                theme::medium(13.0),
                palette.accent,
                badge_right - x,
                1,
            )
        } else if let Some(last) = &chat.last {
            if last.from_me {
                let tick_rect =
                    Rect::from_center_size(pos2(x + 8.0, line_y + 8.0), Vec2::splat(16.0));
                widgets::ticks(ui, &palette, tick_rect, last.status);
                x += 20.0;
            } else if chat.is_group() {
                let sender = app.display_name_or(&last.sender, last.sender_name.as_deref());
                let first = sender.split_whitespace().next().unwrap_or(&sender);
                let sender = widgets::line(
                    ui,
                    &format!("{first}: "),
                    theme::regular(13.0),
                    preview_color,
                    (badge_right - x) * 0.5,
                    1,
                );
                let width = sender.size().x;
                sender.paint(ui, pos2(x, line_y), preview_color);
                x += width;
            }
            widgets::line(
                ui,
                &crate::markup::plain(&app.resolve_mention_tokens(&last.summary), &[]),
                theme::regular(13.0),
                preview_color,
                (badge_right - x).max(0.0),
                1,
            )
        } else {
            widgets::line(ui, "", theme::regular(13.0), preview_color, 1.0, 1)
        };
        preview.paint(ui, pos2(x, line_y), preview_color);
        ui.painter().hline(
            left..=rect.right(),
            rect.bottom() - 0.5,
            egui::Stroke::new(1.0, palette.outline),
        );
    }
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.clicked() {
        app.actions.push(Action::OpenChat(chat.id.clone()));
    }
    let menu_palette = palette;
    egui::Popup::context_menu(&response)
        .frame(widgets::menu_frame(&menu_palette))
        .show(|ui| {
            ui.set_min_width(190.0);
            context_menu(app, ui, chat, &menu_palette);
        });
}

fn context_menu(app: &mut App, ui: &mut egui::Ui, chat: &Chat, palette: &Palette) {
    if chat.unread > 0 && widgets::menu_item(ui, palette, Some(Icon::CheckCheck), "Mark as read") {
        app.actions.push(Action::MarkRead(chat.id.clone()));
    }
    if widgets::menu_item(
        ui,
        palette,
        Some(if chat.pinned { Icon::PinOff } else { Icon::Pin }),
        if chat.pinned { "Unpin" } else { "Pin to top" },
    ) {
        app.actions
            .push(Action::SetPinned(chat.id.clone(), !chat.pinned));
    }
    if widgets::menu_item(
        ui,
        palette,
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
    let now = crate::util::now();
    if chat.muted(now) {
        if widgets::menu_item(ui, palette, Some(Icon::Bell), "Unmute") {
            app.actions.push(Action::SetMuted(chat.id.clone(), None));
        }
    } else {
        for (label, until) in [
            ("Mute for 8 hours", Some(now + 8 * 3600)),
            ("Mute for a week", Some(now + 7 * 86_400)),
            ("Mute always", Some(0)),
        ] {
            if widgets::menu_item(ui, palette, Some(Icon::BellOff), label) {
                app.actions.push(Action::SetMuted(chat.id.clone(), until));
            }
        }
    }
    widgets::menu_separator(ui, palette);
    if let Some(phone) = chat.phone()
        && widgets::menu_item(ui, palette, Some(Icon::Copy), "Copy number")
    {
        app.actions.push(Action::CopyText(format!("+{phone}")));
    }
    if widgets::menu_item(ui, palette, Some(Icon::Info), "Info") {
        app.actions
            .push(Action::ShowDialog(Dialog::ChatInfo(chat.id.clone())));
    }
}
