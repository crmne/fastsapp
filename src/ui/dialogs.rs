//! Modal dialogs: shortcuts, about, unlink, pairing by phone, chat info.

use egui::{Align, CornerRadius, Frame, Layout, Margin, Stroke};

use crate::app::App;
use crate::model::{Action, Dialog};
use crate::theme::{self, Icon};

pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(dialog) = app.dialog.clone() else {
        return;
    };
    let palette = app.palette;
    let frame = Frame::new()
        .fill(palette.overlay)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(theme::RADIUS + 4))
        .inner_margin(Margin::same(22))
        .shadow(egui::epaint::Shadow {
            offset: [0, 12],
            blur: 40,
            spread: 0,
            color: palette.shadow,
        });
    let response = egui::Modal::new(egui::Id::new("dialog"))
        .frame(frame)
        .backdrop_color(palette.shadow)
        .show(ctx, |ui| {
            ui.set_width(match dialog {
                Dialog::Shortcuts => 420.0,
                Dialog::About => 380.0,
                Dialog::ConfirmUnlink => 380.0,
                Dialog::PairWithPhone => 380.0,
                Dialog::ChatInfo(_) => 360.0,
            });
            ui.spacing_mut().item_spacing.y = 8.0;
            match dialog {
                Dialog::Shortcuts => shortcuts(app, ui),
                Dialog::About => about(app, ui),
                Dialog::ConfirmUnlink => confirm_unlink(app, ui),
                Dialog::PairWithPhone => pair_with_phone(app, ui),
                Dialog::ChatInfo(id) => chat_info(app, ui, &id),
            }
        });
    if response.should_close() {
        app.actions.push(Action::CloseDialog);
    }
}

fn title(ui: &mut egui::Ui, app: &mut App, label: &str) {
    let palette = app.palette;
    ui.horizontal(|ui| {
        theme::text(ui, label, theme::bold(18.0), palette.text);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::icon_button(ui, Icon::X, 16.0, palette.secondary, palette.text, "Close")
                .clicked()
            {
                app.actions.push(Action::CloseDialog);
            }
        });
    });
    ui.add_space(4.0);
}

fn shortcuts(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    title(ui, app, "Keyboard shortcuts");
    egui::Grid::new("shortcuts")
        .num_columns(2)
        .spacing([18.0, 8.0])
        .show(ui, |ui| {
            for (keys, what) in super::keys::SHORTCUTS {
                theme::text(
                    ui,
                    super::keys::label(keys),
                    theme::semibold(13.0),
                    palette.text,
                );
                theme::text(ui, *what, theme::regular(13.0), palette.secondary);
                ui.end_row();
            }
        });
}

fn about(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    title(ui, app, "About");
    ui.horizontal(|ui| {
        let (logo, _) = ui.allocate_exact_size(egui::Vec2::splat(44.0), egui::Sense::hover());
        theme::logo(
            ui,
            logo.center(),
            44.0,
            palette.accent,
            egui::Color32::WHITE,
        );
        ui.vertical(|ui| {
            theme::text(ui, "Fastsapp", theme::bold(17.0), palette.text);
            theme::text(
                ui,
                format!("Version {}", env!("CARGO_PKG_VERSION")),
                theme::regular(13.0),
                palette.secondary,
            );
        });
    });
    ui.add_space(6.0);
    theme::paragraph(
        ui,
        "A native WhatsApp client written in Rust with egui, speaking the WhatsApp Web protocol through whatsapp-rust. Messages are end-to-end encrypted on this computer.",
        theme::regular(13.0),
        palette.text,
    );
    theme::paragraph(
        ui,
        "This is an unofficial client. Using it may be against WhatsApp's terms of service and could get an account suspended.",
        theme::regular(12.5),
        palette.secondary,
    );
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if theme::link(ui, "Source code", theme::medium(13.0), palette.link).clicked() {
            app.actions
                .push(Action::OpenUrl(env!("CARGO_PKG_REPOSITORY").to_owned()));
        }
        theme::text(ui, "·", theme::regular(13.0), palette.dim);
        if theme::link(ui, "whatsapp-rust", theme::medium(13.0), palette.link).clicked() {
            app.actions.push(Action::OpenUrl(
                "https://github.com/oxidezap/whatsapp-rust".to_owned(),
            ));
        }
    });
}

fn confirm_unlink(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    title(ui, app, "Unlink this computer?");
    theme::paragraph(
        ui,
        "WhatsApp on your phone will forget this device, and the chats kept here will be deleted. You can link again at any time by scanning a new code.",
        theme::regular(13.5),
        palette.text,
    );
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if danger_button(ui, app, "Unlink") {
                app.actions.push(Action::Unlink);
            }
            if theme::pill_button(ui, &palette, "Keep", false).clicked() {
                app.actions.push(Action::CloseDialog);
            }
        });
    });
}

fn pair_with_phone(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    title(ui, app, "Link with a phone number");
    theme::paragraph(
        ui,
        "Type the number WhatsApp is registered to, with the country code and no leading zeros or plus sign. WhatsApp will show a code to enter on the phone.",
        theme::regular(13.0),
        palette.secondary,
    );
    ui.add_space(4.0);
    let id = egui::Id::new("pair-phone");
    let submit = ui.memory(|memory| memory.has_focus(id))
        && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                theme::text(ui, "+", theme::semibold(16.0), palette.secondary);
                let response = ui.add(
                    egui::TextEdit::singleline(&mut app.pair_phone)
                        .id(id)
                        .hint_text(egui::RichText::new("15551234567").color(palette.dim))
                        .font(theme::regular(16.0))
                        .text_color(palette.text)
                        .frame(egui::Frame::NONE)
                        .desired_width(f32::INFINITY),
                );
                if !response.has_focus() && app.pair_phone.is_empty() {
                    response.request_focus();
                }
            });
        });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let ready = app.pair_phone.chars().filter(char::is_ascii_digit).count() >= 7;
            if (theme::pill_button(ui, &palette, "Get a code", ready).clicked() || submit) && ready
            {
                app.actions
                    .push(Action::PairWithPhone(app.pair_phone.clone()));
            }
            if theme::pill_button(ui, &palette, "Cancel", false).clicked() {
                app.actions.push(Action::CloseDialog);
            }
        });
    });
}

fn chat_info(app: &mut App, ui: &mut egui::Ui, id: &str) {
    let palette = app.palette;
    // A chat, or someone met in a group who has no chat of their own.
    let chat = app
        .chat(id)
        .cloned()
        .unwrap_or_else(|| crate::model::Chat::new(id.to_owned(), app.display_name(id)));
    let has_chat = app.chat(id).is_some();
    let name = app.chat_title(&chat);
    title(ui, app, if chat.is_group() { "Group" } else { "Contact" });
    let picture = app.avatar_full(id).or_else(|| app.avatar(id));
    ui.vertical_centered(|ui| {
        super::widgets::avatar(ui, &palette, &name, id, 160.0, picture.as_deref());
        ui.add_space(6.0);
        super::widgets::rich_text(ui, &name, theme::bold(19.0), palette.text);
        if let Some(phone) = chat.phone() {
            theme::text(
                ui,
                crate::util::phone(phone),
                theme::regular(13.5),
                palette.secondary,
            );
        }
        if chat.is_group() && !chat.participants.is_empty() {
            theme::text(
                ui,
                format!("{} members", chat.participants.len()),
                theme::regular(13.5),
                palette.secondary,
            );
        }
        if let Some(presence) = app.presence.get(id) {
            let status = if presence.online {
                "online".to_owned()
            } else if let Some(seen) = presence.last_seen {
                format!("last seen {}", crate::util::chat_stamp(seen).to_lowercase())
            } else {
                String::new()
            };
            if !status.is_empty() {
                theme::text(ui, status, theme::regular(12.5), palette.dim);
            }
        }
    });
    ui.add_space(8.0);
    if chat.is_group() && !chat.participants.is_empty() {
        theme::text(ui, "Members", theme::medium(12.5), palette.secondary);
        ui.add(
            egui::Label::new(
                egui::RichText::new(app.participant_names(&chat))
                    .font(theme::regular(12.5))
                    .color(palette.text),
            )
            .wrap(),
        );
        ui.add_space(6.0);
    }
    if let Some(until) = chat.muted_until {
        theme::text(
            ui,
            if until == 0 {
                "Muted".to_owned()
            } else {
                format!("Muted until {}", crate::util::chat_stamp(until))
            },
            theme::regular(12.5),
            palette.secondary,
        );
    }
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        if let Some(phone) = chat.phone()
            && theme::soft_button(ui, &palette, Some(Icon::Copy), "Copy number", false).clicked()
        {
            app.actions.push(Action::CopyText(format!("+{phone}")));
        }
        if !has_chat {
            return;
        }
        if theme::soft_button(
            ui,
            &palette,
            Some(if chat.pinned { Icon::PinOff } else { Icon::Pin }),
            if chat.pinned { "Unpin" } else { "Pin" },
            false,
        )
        .clicked()
        {
            app.actions
                .push(Action::SetPinned(chat.id.clone(), !chat.pinned));
        }
        if theme::soft_button(
            ui,
            &palette,
            Some(Icon::Archive),
            if chat.archived {
                "Unarchive"
            } else {
                "Archive"
            },
            false,
        )
        .clicked()
        {
            app.actions
                .push(Action::SetArchived(chat.id.clone(), !chat.archived));
            app.actions.push(Action::CloseDialog);
        }
    });
}

/// A filled button in the danger colour, for the one irreversible action.
fn danger_button(ui: &mut egui::Ui, app: &mut App, label: &str) -> bool {
    let palette = app.palette;
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        theme::semibold(13.0),
        egui::Color32::WHITE,
    );
    let size = galley.size() + egui::vec2(36.0, 16.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if response.hovered() {
            palette.danger.gamma_multiply(0.85)
        } else {
            palette.danger
        };
        ui.painter().rect_filled(rect, rect.height() / 2.0, fill);
        ui.painter().galley(
            rect.center() - galley.size() / 2.0,
            galley,
            egui::Color32::WHITE,
        );
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}
