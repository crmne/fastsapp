//! Window layout: panels, overlays, keyboard shortcuts.

pub mod chats;
pub mod conversation;
pub mod dialogs;
pub mod keys;
pub mod login;
pub mod picker;
pub mod settings;
pub mod widgets;

use egui::{Align2, CornerRadius, Frame, Margin, Stroke, vec2};

use crate::app::App;
use crate::backend::LinkStatus;
use crate::model::{Action, Page, ToastKind};
use crate::theme::{self, Icon};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let ctx = &ctx;
    keys::handle(app, ctx);
    titlebar_strip(app, ui);
    if !app.is_linked() {
        login::show(app, ui);
        dialogs::show(app, ctx);
        toasts(app, ctx);
        return;
    }
    banner(app, ui);
    if app.sidebar_visible {
        chats::show(app, ui);
    }
    let palette = app.palette;
    egui::CentralPanel::default()
        .frame(Frame::new().fill(palette.chat))
        .show(ui, |ui| match app.page {
            Page::Settings => settings::show(app, ui),
            Page::Chats => conversation::show(app, ui),
        });
    picker::show(app, ctx);
    dialogs::show(app, ctx);
    drop_target(app, ctx);
    toasts(app, ctx);
}

/// Says where dragged files will go.
fn drop_target(app: &mut App, ctx: &egui::Context) {
    if !app.dropping {
        return;
    }
    let palette = app.palette;
    let name = app
        .current_chat()
        .map(|chat| chat.name.clone())
        .unwrap_or_default();
    egui::Area::new(egui::Id::new("drop-target"))
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            Frame::new()
                .fill(palette.overlay)
                .stroke(Stroke::new(2.0, palette.accent))
                .corner_radius(CornerRadius::same(theme::RADIUS + 4))
                .inner_margin(Margin::symmetric(28, 20))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        theme::icon(ui, Icon::Paperclip, 28.0, palette.accent);
                        theme::text(
                            ui,
                            format!("Drop to send to {name}"),
                            theme::semibold(15.0),
                            palette.text,
                        );
                    });
                });
        });
}

/// A strip across the top while the connection is not simply up.
fn banner(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let (icon, text, color, retry) = match &app.link {
        LinkStatus::Connected if app.syncing => (
            Icon::Refresh,
            match app.sync_percent {
                Some(percent) => format!("Bringing in your chat history… {percent}%"),
                None => "Bringing in your chat history…".to_owned(),
            },
            palette.accent,
            false,
        ),
        LinkStatus::Connected => return,
        LinkStatus::Starting | LinkStatus::Connecting => (
            Icon::Refresh,
            "Connecting to WhatsApp…".to_owned(),
            palette.secondary,
            false,
        ),
        LinkStatus::Disconnected { reason } => (
            Icon::WifiOff,
            format!("Offline ({reason}); reconnecting"),
            palette.warning,
            true,
        ),
        LinkStatus::Failed(message) => (Icon::CircleAlert, message.clone(), palette.danger, true),
        LinkStatus::Unlinked { .. } | LinkStatus::LoggedOut => (
            Icon::Smartphone,
            "Not linked to a phone".to_owned(),
            palette.warning,
            false,
        ),
    };
    egui::Panel::top("banner")
        .show_separator_line(false)
        .frame(
            Frame::new()
                .fill(palette.panel)
                .inner_margin(Margin::symmetric(14, 6)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if matches!(
                    app.link,
                    LinkStatus::Starting | LinkStatus::Connecting | LinkStatus::Connected
                ) {
                    theme::spinner(ui, 14.0, color);
                } else {
                    theme::icon(ui, icon, 15.0, color);
                }
                theme::text(ui, text, theme::medium(13.0), palette.text);
                if retry {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::soft_button(ui, &palette, Some(Icon::Refresh), "Retry", false)
                            .clicked()
                        {
                            app.actions.push(Action::Reconnect);
                        }
                    });
                }
            });
        });
}

fn toasts(app: &mut App, ctx: &egui::Context) {
    if app.toasts.is_empty() {
        return;
    }
    let palette = app.palette;
    egui::Area::new(egui::Id::new("toasts"))
        .anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -20.0))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            for toast in &app.toasts {
                let age = toast.created.elapsed().as_secs_f32();
                let alpha = if age < 0.15 {
                    age / 0.15
                } else if age > 2.8 {
                    ((3.2 - age) / 0.4).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                ui.set_opacity(alpha);
                Frame::new()
                    .fill(palette.overlay)
                    .stroke(Stroke::new(1.0, palette.outline))
                    .corner_radius(CornerRadius::same(theme::RADIUS))
                    .inner_margin(Margin::symmetric(14, 10))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 4],
                        blur: 16,
                        spread: 0,
                        color: palette.shadow,
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (icon, color) = match toast.kind {
                                ToastKind::Info => (Icon::CircleCheck, palette.accent),
                                ToastKind::Error => (Icon::CircleAlert, palette.danger),
                            };
                            theme::icon(ui, icon, 16.0, color);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&toast.message)
                                        .font(theme::medium(13.5))
                                        .color(palette.text),
                                )
                                .wrap(),
                            );
                        });
                    });
            }
        });
}

/// On macOS the window has no title bar: this strip stands where it was,
/// the traffic lights float over it, and dragging it moves the window.
fn titlebar_strip(app: &App, ui: &mut egui::Ui) {
    let inset = theme::titlebar_inset(ui.ctx());
    if inset == 0.0 {
        return;
    }
    let fill = if app.is_linked() {
        app.palette.panel
    } else {
        app.palette.window
    };
    egui::Panel::top("titlebar")
        .exact_size(inset)
        .show_separator_line(false)
        .frame(Frame::new().fill(fill))
        .show(ui, |ui| {
            let rect = ui.max_rect();
            titlebar_drag(ui, rect);
        });
}

/// Makes `rect` behave like the title bar that is no longer there: dragging
/// it moves the window.
pub fn titlebar_drag(ui: &mut egui::Ui, rect: egui::Rect) {
    let response = ui.interact(
        rect,
        ui.id().with("titlebar-drag"),
        egui::Sense::click_and_drag(),
    );
    // AppKit begins the move from the mouse-down event that is still live,
    // so the command has to go out on the press itself; waiting for egui's
    // drag threshold leaves the event stale and the window stays put.
    if response.is_pointer_button_down_on() && ui.input(|input| input.pointer.primary_pressed()) {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
}
