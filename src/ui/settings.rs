//! The settings page.

use egui::{CornerRadius, Frame, Margin};

use crate::app::App;
use crate::model::{Action, Dialog, Page};
use crate::settings::ThemeChoice;
use crate::theme::{self, Icon};

use super::widgets;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    egui::ScrollArea::vertical()
        .id_salt("settings")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            Frame::new()
                .inner_margin(Margin::symmetric(32, 24))
                .show(ui, |ui| {
                    ui.set_max_width(640.0);
                    ui.horizontal(|ui| {
                        if theme::icon_button(
                            ui,
                            Icon::ArrowLeft,
                            20.0,
                            palette.secondary,
                            palette.text,
                            "Back (Esc)",
                        )
                        .clicked()
                        {
                            app.actions.push(Action::Open(Page::Chats));
                        }
                        theme::text(ui, "Settings", theme::bold(24.0), palette.text);
                    });
                    ui.add_space(18.0);

                    section(ui, app, "Appearance");
                    widgets::setting_row(ui, &palette, "Theme", "", |ui| {
                        for choice in ThemeChoice::ALL.iter().rev() {
                            let active = app.settings.theme == *choice;
                            if theme::soft_button(ui, &palette, None, choice.label(), active).clicked()
                                && !active
                            {
                                app.settings.theme = *choice;
                                app.actions.push(Action::SettingsChanged);
                            }
                        }
                    });
                    widgets::setting_row(
                        ui,
                        &palette,
                        "Zoom",
                        "You can also use Ctrl+plus and Ctrl+minus.",
                        |ui| {
                            if theme::icon_button(ui, Icon::Plus, 16.0, palette.secondary, palette.text, "Larger").clicked() {
                                app.actions.push(Action::ZoomBy(0.1));
                            }
                            theme::text(
                                ui,
                                format!("{:.0}%", app.settings.zoom * 100.0),
                                theme::medium(13.5),
                                palette.text,
                            );
                            if theme::icon_button(ui, Icon::Minus, 16.0, palette.secondary, palette.text, "Smaller").clicked() {
                                app.actions.push(Action::ZoomBy(-0.1));
                            }
                        },
                    );

                    section(ui, app, "Chats");
                    toggle(ui, app, "Enter sends", "When off, Enter adds a line and Ctrl+Enter sends.", |settings| &mut settings.enter_sends);
                    let receipts_note = if app.account_receipts_off {
                        "Read receipts are disabled for your WhatsApp account. One-to-one chats will not send them. Groups always do."
                    } else {
                        "Let people see when you read their messages. Your WhatsApp privacy setting still applies."
                    };
                    toggle(ui, app, "Send read receipts", receipts_note, |settings| &mut settings.send_read_receipts);
                    toggle(ui, app, "Show when you are typing", "", |settings| &mut settings.send_typing);
                    toggle(ui, app, "Download attachments automatically", "Download pictures, videos, voice messages, and documents up to 64 MB when they enter view. When off, click a file to download it.", |settings| &mut settings.auto_download);
                    toggle(ui, app, "Show sender pictures in every chat", "WhatsApp shows them in groups only.", |settings| &mut settings.show_sender_pictures);
                    toggle(ui, app, "Names from your address book", "Prefer saved contact names. When off, prefer public WhatsApp profile names. This applies throughout the app.", |settings| &mut settings.names_from_contacts);
                    toggle(ui, app, "Save contacts to the phone's address book", "Also add contacts saved here to your phone's address book. When off, they remain WhatsApp contacts. Names sync to linked devices either way.", |settings| &mut settings.save_contacts_to_phone);
                    toggle(ui, app, "Show shortcut hints", "", |settings| &mut settings.show_shortcut_hints);

                    section(ui, app, "Window");
                    toggle(ui, app, "Keep running when the window closes", "Keep FastsApp linked in the system tray. Quit from the tray menu or with Ctrl+Q.", |settings| &mut settings.keep_running_in_background);
                    toggle(ui, app, "Notify about new messages", "Show desktop notifications when the window is hidden, in the background, or showing another chat. Muted chats do not notify you.", |settings| &mut settings.notifications);
                    toggle(ui, app, "Check for updates", "Ask GitHub once a day whether a newer FastsApp release exists. The request identifies only FastsApp and its version.", |settings| &mut settings.check_for_updates);

                    widgets::setting_row(
                        ui,
                        &palette,
                        "GIPHY API key",
                        if crate::settings::BUILT_IN_GIPHY_KEY.is_some() {
                            "Used for GIF search. This build includes a key. Enter a key from developers.giphy.com to replace it."
                        } else {
                            "Required for GIF search. Get a free key from developers.giphy.com."
                        },
                        |ui| {
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut app.settings.giphy_key)
                                    .font(theme::regular(13.0))
                                    .text_color(palette.text)
                                    .desired_width(220.0),
                            );
                            if response.changed() {
                                app.actions.push(Action::SettingsChanged);
                            }
                        },
                    );

                    section(ui, app, "Account");
                    let name = app.me_name.clone().unwrap_or_default();
                    let me = app.me.clone().unwrap_or_default();
                    let phone = crate::model::phone_of(&me)
                        .map(crate::util::phone)
                        .unwrap_or_else(|| me.clone());
                    let description = match &app.me_about {
                        Some(about) => format!("{phone} · {about}"),
                        None => phone,
                    };
                    ui.horizontal(|ui| {
                        let picture = app.avatar_full(&me).or_else(|| app.avatar(&me));
                        widgets::avatar(ui, &palette, &name, &me, 56.0, picture.as_deref());
                    });
                    ui.add_space(6.0);
                    widgets::setting_row(
                        ui,
                        &palette,
                        if name.is_empty() { "Linked device" } else { &name },
                        &description,
                        |ui| {
                            if theme::soft_button(ui, &palette, Some(Icon::LogOut), "Unlink this computer", false).clicked() {
                                app.actions.push(Action::ShowDialog(Dialog::ConfirmUnlink));
                            }
                        },
                    );

                    section(ui, app, "Files");
                    let archive = app.dirs.archive_db();
                    widgets::setting_row(
                        ui,
                        &palette,
                        "Message archive",
                        &archive.display().to_string(),
                        |ui| {
                            if theme::soft_button(ui, &palette, Some(Icon::ExternalLink), "Open folder", false).clicked() {
                                app.actions.push(Action::OpenFile(app.dirs.state.clone()));
                            }
                        },
                    );
                    let media = app.dirs.media_cache_dir();
                    widgets::setting_row(
                        ui,
                        &palette,
                        "Downloaded attachments",
                        &media.display().to_string(),
                        |ui| {
                            if theme::soft_button(ui, &palette, Some(Icon::ExternalLink), "Open folder", false).clicked() {
                                let _ = std::fs::create_dir_all(&media);
                                app.actions.push(Action::OpenFile(media.clone()));
                            }
                        },
                    );
                    let log = app.dirs.log_file();
                    widgets::setting_row(ui, &palette, "Log of this run", &log.display().to_string(), |ui| {
                        if theme::soft_button(ui, &palette, Some(Icon::FileText), "Open", false).clicked() {
                            app.actions.push(Action::OpenFile(log.clone()));
                        }
                    });

                    section(ui, app, "About");
                    widgets::setting_row(
                        ui,
                        &palette,
                        &format!("FastsApp {}", env!("CARGO_PKG_VERSION")),
                        "A native WhatsApp client built with Rust, egui, and whatsapp-rust.",
                        |ui| {
                            if theme::soft_button(ui, &palette, Some(Icon::Info), "About", false).clicked() {
                                app.actions.push(Action::ShowDialog(Dialog::About));
                            }
                            if theme::soft_button(ui, &palette, Some(Icon::Keyboard), "Shortcuts", false).clicked() {
                                app.actions.push(Action::ShowDialog(Dialog::Shortcuts));
                            }
                        },
                    );
                });
        });
}

fn section(ui: &mut egui::Ui, app: &App, label: &str) {
    let palette = app.palette;
    ui.add_space(10.0);
    Frame::new()
        .fill(palette.panel)
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin::symmetric(14, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::text(ui, label, theme::semibold(12.5), palette.accent);
        });
    ui.add_space(8.0);
}

fn toggle(
    ui: &mut egui::Ui,
    app: &mut App,
    label: &str,
    description: &str,
    field: impl Fn(&mut crate::settings::Settings) -> &mut bool,
) {
    let palette = app.palette;
    let mut value = *field(&mut app.settings);
    let mut changed = false;
    widgets::setting_row(ui, &palette, label, description, |ui| {
        changed = widgets::switch(ui, &palette, &mut value).changed();
    });
    if changed {
        *field(&mut app.settings) = value;
        app.actions.push(Action::SettingsChanged);
    }
}
