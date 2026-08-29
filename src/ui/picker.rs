//! The picker above the composer: emoji, GIFs, and stickers.

use std::path::Path;

use egui::{
    Align2, CornerRadius, Frame, Key, Margin, Modifiers, Rect, Sense, Stroke, Vec2, pos2, vec2,
};

use crate::app::App;
use crate::model::{Action, PickerTab};
use crate::theme::{self, Icon, Palette};

use super::widgets;

const WIDTH: f32 = 420.0;
const HEIGHT: f32 = 400.0;
/// An emoji cell is at least this wide; the grid takes as many columns as
/// fit and stretches them to fill the width.
const CELL: f32 = 40.0;

/// One row of the emoji list: a heading or one row's worth of emoji.
enum Row {
    Header(&'static str),
    Emoji(Vec<&'static str>),
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(tab) = app.picker else {
        return;
    };
    let palette = app.palette;
    let Some(anchor) = app.picker_anchor else {
        return;
    };
    let screen = ctx.content_rect();
    let x = anchor
        .left()
        .clamp(screen.left() + 8.0, (screen.right() - WIDTH - 8.0).max(8.0));
    let y = (anchor.top() - HEIGHT - 10.0).max(screen.top() + 8.0);
    let area = egui::Area::new(egui::Id::new("picker"))
        .fixed_pos(pos2(x, y))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            Frame::new()
                .fill(palette.overlay)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(theme::RADIUS + 4))
                .inner_margin(Margin::same(10))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 8],
                    blur: 28,
                    spread: 0,
                    color: palette.shadow,
                })
                .show(ui, |ui| {
                    ui.set_width(WIDTH);
                    ui.set_height(HEIGHT);
                    ui.spacing_mut().item_spacing.y = 6.0;
                    let body_height = HEIGHT - 44.0;
                    ui.allocate_ui_with_layout(
                        vec2(WIDTH, body_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| match tab {
                            PickerTab::Emoji => emoji_tab(app, ui, &palette),
                            PickerTab::Gifs => gif_tab(app, ui, &palette),
                            PickerTab::Stickers => sticker_tab(app, ui, &palette),
                        },
                    );
                    tabs(app, ui, &palette, tab);
                });
        });
    // A click anywhere else closes it, unless it is the button that opens
    // it, which toggles on its own.
    let rect = area.response.rect;
    let clicked_outside = ctx.input(|input| {
        input.pointer.any_pressed()
            && input
                .pointer
                .interact_pos()
                .is_some_and(|pos| !rect.contains(pos) && !anchor.contains(pos))
    });
    if clicked_outside {
        app.actions.push(Action::ClosePicker);
    }
}

fn tabs(app: &mut App, ui: &mut egui::Ui, palette: &Palette, current: PickerTab) {
    ui.horizontal(|ui| {
        let entries = [
            (PickerTab::Emoji, Icon::Smile, "Emoji"),
            (PickerTab::Gifs, Icon::Gif, "GIF"),
            (PickerTab::Stickers, Icon::Sticker, "Stickers"),
        ];
        let total = entries.len() as f32 * 100.0 + 12.0;
        ui.add_space((ui.available_width() - total).max(0.0) / 2.0);
        for (tab, icon, label) in entries {
            if theme::soft_button(ui, palette, Some(icon), label, tab == current).clicked()
                && tab != current
            {
                app.actions.push(Action::TogglePicker(tab));
            }
        }
    });
}

fn search_box(
    ui: &mut egui::Ui,
    palette: &Palette,
    id: &str,
    text: &mut String,
    hint: &str,
) -> egui::Response {
    let width = ui.available_width();
    widgets::search_field(ui, palette, egui::Id::new(id), text, hint, width)
}

// --- emoji ---------------------------------------------------------------

fn group_name(group: emojis::Group) -> &'static str {
    match group {
        emojis::Group::SmileysAndEmotion => "Smileys & Emotion",
        emojis::Group::PeopleAndBody => "People & Body",
        emojis::Group::AnimalsAndNature => "Animals & Nature",
        emojis::Group::FoodAndDrink => "Food & Drink",
        emojis::Group::TravelAndPlaces => "Travel & Places",
        emojis::Group::Activities => "Activities",
        emojis::Group::Objects => "Objects",
        emojis::Group::Symbols => "Symbols",
        emojis::Group::Flags => "Flags",
    }
}

fn rows_for(app: &App, columns: usize) -> Vec<Row> {
    let query = app.picker_search.trim().to_lowercase();
    let mut rows = Vec::new();
    let chunk = |rows: &mut Vec<Row>, list: Vec<&'static str>| {
        for part in list.chunks(columns) {
            rows.push(Row::Emoji(part.to_vec()));
        }
    };
    if !query.is_empty() {
        let found: Vec<&'static str> = emojis::iter()
            .filter(|emoji| {
                emoji.name().to_lowercase().contains(&query)
                    || emoji
                        .shortcodes()
                        .any(|code| code.to_lowercase().contains(&query))
            })
            .map(|emoji| emoji.as_str())
            .collect();
        if found.is_empty() {
            rows.push(Row::Header("Nothing matches"));
        } else {
            chunk(&mut rows, found);
        }
        return rows;
    }
    if !app.settings.recent_emoji.is_empty() {
        rows.push(Row::Header("Recent"));
        let recent: Vec<&'static str> = app
            .settings
            .recent_emoji
            .iter()
            .filter_map(|emoji| emojis::get(emoji).map(|emoji| emoji.as_str()))
            .collect();
        chunk(&mut rows, recent);
    }
    for group in emojis::Group::iter() {
        rows.push(Row::Header(group_name(group)));
        chunk(
            &mut rows,
            group.emojis().map(|emoji| emoji.as_str()).collect(),
        );
    }
    rows
}

fn emoji_tab(app: &mut App, ui: &mut egui::Ui, palette: &Palette) {
    let mut search = app.picker_search.clone();
    let response = search_box(ui, palette, "emoji-search", &mut search, "Search emoji");
    if search != app.picker_search {
        app.picker_search = search;
    }
    if app.picker_search.is_empty() && !response.has_focus() && ui.input(|i| i.time) < 0.0 {
        response.request_focus();
    }
    // Leave the scroll bar its edge, then fill the rest with whole
    // columns.
    let width = ui.available_width() - 6.0;
    let columns = ((width / CELL).floor() as usize).max(1);
    let cell = width / columns as f32;
    let rows = rows_for(app, columns);
    let row_height = CELL;
    let mut picked = None;
    egui::ScrollArea::vertical()
        .id_salt("emoji-grid")
        .auto_shrink([false, false])
        .show_rows(ui, row_height, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            for row in &rows[range] {
                match row {
                    Row::Header(label) => {
                        let (rect, _) = ui.allocate_exact_size(
                            vec2(ui.available_width(), row_height),
                            Sense::hover(),
                        );
                        ui.painter().text(
                            pos2(rect.left() + 4.0, rect.bottom() - 8.0),
                            Align2::LEFT_BOTTOM,
                            *label,
                            theme::semibold(12.5),
                            palette.secondary,
                        );
                    }
                    Row::Emoji(list) => {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::ZERO;
                            for emoji in list {
                                let (rect, response) =
                                    ui.allocate_exact_size(vec2(cell, row_height), Sense::click());
                                if ui.is_rect_visible(rect) {
                                    if response.hovered() {
                                        ui.painter().rect_filled(
                                            rect.shrink(2.0),
                                            6.0,
                                            palette.surface_hover,
                                        );
                                    }
                                    let line = widgets::line(
                                        ui,
                                        emoji,
                                        theme::regular(24.0),
                                        palette.text,
                                        cell,
                                        1,
                                    );
                                    line.paint(ui, rect.center() - line.size() / 2.0, palette.text);
                                }
                                if response
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .clicked()
                                {
                                    picked = Some((*emoji).to_owned());
                                }
                            }
                        });
                    }
                }
            }
        });
    if let Some(emoji) = picked {
        app.actions.push(Action::InsertEmoji(emoji));
    }
}

// --- GIFs ---------------------------------------------------------------

fn gif_tab(app: &mut App, ui: &mut egui::Ui, palette: &Palette) {
    if app.settings.effective_giphy_key().is_none() {
        ui.add_space(8.0);
        theme::paragraph(
            ui,
            "GIF search needs a free GIPHY API key. Create one at developers.giphy.com and paste it here; it is kept in your settings file.",
            theme::regular(13.0),
            palette.text,
        );
        ui.add_space(6.0);
        if theme::link(
            ui,
            "developers.giphy.com",
            theme::medium(13.0),
            palette.link,
        )
        .clicked()
        {
            app.actions
                .push(Action::OpenUrl("https://developers.giphy.com/".to_owned()));
        }
        ui.add_space(6.0);
        Frame::new()
            .fill(palette.surface)
            .corner_radius(CornerRadius::same(theme::RADIUS))
            .inner_margin(Margin::symmetric(10, 6))
            .show(ui, |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut app.settings.giphy_key)
                        .hint_text(egui::RichText::new("GIPHY API key").color(palette.dim))
                        .font(theme::regular(13.5))
                        .text_color(palette.text)
                        .frame(Frame::NONE)
                        .desired_width(f32::INFINITY),
                );
                if response.changed() {
                    app.actions.push(Action::SettingsChanged);
                }
                if response.lost_focus() && !app.settings.giphy_key.trim().is_empty() {
                    app.actions.push(Action::SearchGifs(String::new()));
                }
            });
        return;
    }
    let mut query = app.picker_search.clone();
    let submit = ui.memory(|memory| memory.has_focus(egui::Id::new("gif-search")))
        && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter));
    search_box(
        ui,
        palette,
        "gif-search",
        &mut query,
        "Search GIFs via GIPHY",
    );
    if query != app.picker_search {
        app.picker_search = query.clone();
    }
    if submit && query.trim() != app.gif_query.trim() {
        app.actions.push(Action::SearchGifs(query));
    }
    if app.gif_pending {
        ui.horizontal(|ui| {
            theme::spinner(ui, 16.0, palette.accent);
            theme::text(ui, "Looking…", theme::regular(12.5), palette.secondary);
        });
    } else if let Some(error) = &app.gif_error {
        theme::paragraph(ui, error, theme::regular(13.0), palette.danger);
    }
    let results = app.gif_results.clone();
    let columns = 3;
    let gap = 6.0;
    let tile_width = (ui.available_width() - gap * (columns as f32 - 1.0)) / columns as f32;
    let mut picked = None;
    egui::ScrollArea::vertical()
        .id_salt("gif-grid")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(gap, gap);
            for row in results.chunks(columns) {
                ui.horizontal(|ui| {
                    for gif in row {
                        let ratio =
                            (gif.height.max(1) as f32 / gif.width.max(1) as f32).clamp(0.5, 1.4);
                        let size = vec2(tile_width, (tile_width * ratio).min(150.0));
                        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
                        if ui.is_rect_visible(rect) {
                            ui.painter().rect_filled(rect, 6.0, palette.surface);
                            if let Some(still) = &gif.still {
                                egui::Image::new(format!("file://{}", still.display()))
                                    .fit_to_exact_size(size)
                                    .corner_radius(6.0)
                                    .paint_at(ui, rect);
                            }
                            if response.hovered() {
                                ui.painter().rect_stroke(
                                    rect,
                                    6.0,
                                    Stroke::new(2.0, palette.accent),
                                    egui::StrokeKind::Inside,
                                );
                            }
                        }
                        if response
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            picked = Some(gif.clone());
                        }
                    }
                });
            }
            if results.is_empty() && !app.gif_pending && app.gif_error.is_none() {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| {
                    theme::text(
                        ui,
                        "Type to search, or wait for what is trending.",
                        theme::regular(13.0),
                        palette.secondary,
                    );
                });
            }
        });
    if let Some(gif) = picked {
        app.actions.push(Action::SendGif(gif));
    }
}

// --- stickers -----------------------------------------------------------

fn sticker_tab(app: &mut App, ui: &mut egui::Ui, palette: &Palette) {
    ui.add_space(4.0);
    theme::text(ui, "Recent", theme::semibold(12.5), palette.secondary);
    if app.stickers.is_empty() {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            theme::paragraph(
                ui,
                if app.stickers_pending {
                    "Fetching your stickers…"
                } else {
                    "The stickers you have sent lately, and those you receive, show up here."
                },
                theme::regular(13.0),
                palette.secondary,
            );
        });
        return;
    }
    let stickers = app.stickers.clone();
    let columns = 5;
    let gap = 6.0;
    let cell = (ui.available_width() - gap * (columns as f32 - 1.0)) / columns as f32;
    let mut picked: Option<std::path::PathBuf> = None;
    egui::ScrollArea::vertical()
        .id_salt("sticker-grid")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(gap, gap);
            for row in stickers.chunks(columns) {
                ui.horizontal(|ui| {
                    for path in row {
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::splat(cell), Sense::click());
                        if ui.is_rect_visible(rect) {
                            if response.hovered() {
                                ui.painter().rect_filled(rect, 8.0, palette.surface_hover);
                            }
                            sticker_picture(ui, path, rect.shrink(4.0));
                        }
                        if response
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            picked = Some(path.clone());
                        }
                    }
                });
            }
        });
    if let Some(path) = picked {
        app.actions.push(Action::SendSticker(path));
    }
}

fn sticker_picture(ui: &egui::Ui, path: &Path, rect: Rect) {
    egui::Image::new(format!("file://{}", path.display()))
        .fit_to_exact_size(rect.size())
        .paint_at(ui, rect);
}
