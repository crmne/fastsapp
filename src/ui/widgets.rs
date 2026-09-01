//! Building blocks the views share: avatars, fields, menus, badges.

use std::path::Path;

use egui::{
    Align, Color32, CornerRadius, Layout, Rect, Sense, Stroke, Ui, UiBuilder, Vec2, pos2, vec2,
};

use crate::emoji;
use crate::model::Delivery;
use crate::theme::{self, Icon, Palette};

/// A laid-out line (or few lines) of text with its emoji, ready to paint.
pub struct Line {
    pub galley: std::sync::Arc<egui::Galley>,
    placements: Vec<String>,
}

impl Line {
    pub fn size(&self) -> Vec2 {
        self.galley.size()
    }

    pub fn paint(&self, ui: &Ui, pos: egui::Pos2, fallback: Color32) {
        ui.painter().galley(pos, self.galley.clone(), fallback);
        emoji::paint(ui, &self.galley, pos, &self.placements);
    }
}

/// Text cut to `max_rows` lines within `width`, ending in an ellipsis, with
/// emoji drawn in colour.
pub fn line(
    ui: &Ui,
    text: &str,
    font: egui::FontId,
    color: Color32,
    width: f32,
    max_rows: usize,
) -> Line {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = width;
    job.wrap.max_rows = max_rows;
    // A single line fills its row to the ellipsis; several lines break at
    // words, or a card description ends in a split word.
    job.wrap.break_anywhere = max_rows == 1;
    job.wrap.overflow_character = Some('…');
    let mut placements = Vec::new();
    let format = egui::TextFormat::simple(font, color);
    let single = text.lines().next().unwrap_or_default();
    emoji::append(
        &mut job,
        &mut placements,
        if max_rows == 1 { single } else { text },
        &format,
    );
    Line {
        galley: ui.painter().layout_job(job),
        placements,
    }
}

/// A single line of text with emoji, allocated in the layout and truncated
/// to the available width.
pub fn rich_text(ui: &mut Ui, text: &str, font: egui::FontId, color: Color32) -> egui::Response {
    let width = ui.available_width().max(1.0);
    let line = line(ui, text, font, color, width, 1);
    let (rect, response) = ui.allocate_exact_size(line.size(), Sense::hover());
    if ui.is_rect_visible(rect) {
        line.paint(ui, rect.min, color);
    }
    response
}

/// Like [`rich_text`], and the text can be swept and copied.
pub fn selectable_rich_text(
    ui: &mut Ui,
    text: &str,
    font: egui::FontId,
    color: Color32,
) -> egui::Response {
    let width = ui.available_width().max(1.0);
    let line = line(ui, text, font, color, width, 1);
    let (rect, response) = ui.allocate_exact_size(line.size(), Sense::click_and_drag());
    // Registered with the copy rewriter, so emoji in a copied name come
    // out as themselves rather than as placeholder glyphs.
    if let Some(rows) = ui.ctx().data(|data| {
        data.get_temp::<std::sync::Arc<std::sync::Mutex<Vec<crate::transcript::Row>>>>(
            egui::Id::new("copy-rows"),
        )
    }) {
        rows.lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(crate::transcript::Row {
                header: String::new(),
                body: line.galley.text().to_owned(),
                placements: line.placements.clone(),
                ..Default::default()
            });
    }
    if ui.is_rect_visible(rect) {
        egui::text_selection::LabelSelectionState::label_text_selection(
            ui,
            &response,
            rect.min,
            line.galley.clone(),
            color,
            egui::Stroke::NONE,
        );
        crate::emoji::paint(ui, &line.galley, rect.min, &line.placements);
    }
    response
}

/// A round picture, or initials on a colour derived from the id when there
/// is none. The picture is loaded by egui from its file.
pub fn avatar(
    ui: &mut Ui,
    palette: &Palette,
    name: &str,
    id: &str,
    size: f32,
    picture: Option<&Path>,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if ui.is_rect_visible(rect) {
        paint_avatar(ui, palette, rect, name, id, picture);
    }
    response
}

pub fn paint_avatar(
    ui: &Ui,
    palette: &Palette,
    rect: Rect,
    name: &str,
    id: &str,
    picture: Option<&Path>,
) {
    let size = rect.width();
    let mut painted = false;
    if let Some(picture) = picture {
        let uri = format!("file://{}", picture.display());
        let image = egui::Image::new(uri)
            .fit_to_exact_size(Vec2::splat(size))
            .corner_radius(size / 2.0);
        if let Ok(egui::load::TexturePoll::Ready { .. }) =
            image.load_for_size(ui.ctx(), Vec2::splat(size))
        {
            image.paint_at(ui, rect);
            painted = true;
        }
    }
    if !painted {
        let fill = palette.avatar(crate::util::hue(id));
        ui.painter().circle_filled(rect.center(), size / 2.0, fill);
        if crate::model::ChatKind::from_id(id) == crate::model::ChatKind::Group {
            theme::paint_icon(ui, Icon::Users, rect, size * 0.5, Color32::WHITE);
        } else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                crate::util::initials(name),
                theme::semibold(size * 0.38),
                Color32::WHITE,
            );
        }
    }
}

/// The check marks under our own messages.
pub fn ticks(ui: &Ui, palette: &Palette, rect: Rect, status: Delivery) {
    let (icon, color) = match status {
        Delivery::None => return,
        Delivery::Pending => (Icon::Clock, palette.secondary),
        Delivery::Sent => (Icon::Check, palette.secondary),
        Delivery::Delivered => (Icon::CheckCheck, palette.secondary),
        Delivery::Read | Delivery::Played => (Icon::CheckCheck, palette.read),
        Delivery::Failed => (Icon::CircleAlert, palette.danger),
    };
    theme::paint_icon(ui, icon, rect, rect.height(), color);
}

/// The unread count on a chat row.
pub fn badge(ui: &Ui, palette: &Palette, at: egui::Pos2, count: u32, muted: bool) -> f32 {
    let label = if count > 99 {
        "99+".to_owned()
    } else {
        count.to_string()
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label, theme::semibold(11.0), palette.on_accent);
    let width = (galley.size().x + 12.0).max(20.0);
    let rect = Rect::from_center_size(at, vec2(width, 20.0));
    let fill = if muted { palette.dim } else { palette.accent };
    ui.painter().rect_filled(rect, 10.0, fill);
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        palette.on_accent,
    );
    width
}

/// How wide a menu must be to fit its entries, so it is no wider.
pub fn menu_width(ui: &Ui, labels: &[&str], icons: bool) -> f32 {
    let widest = labels
        .iter()
        .map(|label| {
            ui.painter()
                .layout_no_wrap(label.to_string(), theme::regular(13.5), Color32::WHITE)
                .size()
                .x
        })
        .fold(0.0, f32::max);
    widest + if icons { 26.0 } else { 0.0 } + 20.0 + 12.0
}

pub fn menu_item(ui: &mut Ui, palette: &Palette, icon: Option<Icon>, label: &str) -> bool {
    menu_item_enabled(ui, palette, icon, label, true)
}

pub fn menu_item_enabled(
    ui: &mut Ui,
    palette: &Palette,
    icon: Option<Icon>,
    label: &str,
    enabled: bool,
) -> bool {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(
        vec2(width, 28.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    if ui.is_rect_visible(rect) {
        if response.hovered() && enabled {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(6), palette.surface_hover);
        }
        let color = if enabled { palette.text } else { palette.dim };
        let mut x = rect.left() + 10.0;
        if let Some(icon) = icon {
            let icon_rect =
                Rect::from_center_size(pos2(x + 8.0, rect.center().y), Vec2::splat(16.0));
            icon.image(
                if enabled {
                    palette.secondary
                } else {
                    palette.dim
                },
                16.0,
            )
            .paint_at(ui, icon_rect);
            x += 26.0;
        }
        let mut job = egui::text::LayoutJob::simple_singleline(
            label.to_string(),
            theme::regular(13.5),
            color,
        );
        job.wrap = egui::text::TextWrapping {
            max_width: (rect.right() - 10.0 - x).max(0.0),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('\u{2026}'),
        };
        let galley = ui.painter().layout_job(job);
        ui.painter().galley(
            pos2(x, rect.center().y - galley.size().y / 2.0),
            galley,
            color,
        );
    }
    let clicked = enabled && response.clicked();
    if clicked {
        ui.close();
    }
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    clicked
}

pub fn menu_separator(ui: &mut Ui, palette: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::hover());
    ui.painter().hline(
        rect.x_range().shrink(6.0),
        rect.center().y,
        Stroke::new(1.0, palette.outline),
    );
}

/// The frame every popup menu uses.
pub fn menu_frame(palette: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(palette.overlay)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(egui::Margin::same(6))
        .shadow(egui::epaint::Shadow {
            offset: [0, 6],
            blur: 20,
            spread: 0,
            color: palette.shadow,
        })
}

pub fn empty_state(ui: &mut Ui, palette: &Palette, icon: Icon, title: &str, body: &str) {
    ui.add_space(48.0);
    ui.vertical_centered(|ui| {
        theme::icon(ui, icon, 40.0, palette.dim);
        ui.add_space(8.0);
        theme::text(ui, title, theme::semibold(16.0), palette.text);
        ui.add_space(2.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(body)
                    .font(theme::regular(13.5))
                    .color(palette.secondary),
            )
            .wrap()
            .selectable(false),
        );
    });
}

/// A rounded search box with a magnifier and a clear button.
pub fn search_field(
    ui: &mut Ui,
    palette: &Palette,
    id: egui::Id,
    text: &mut String,
    hint: &str,
    width: f32,
) -> egui::Response {
    let height = 34.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let has_focus = ui.memory(|memory| memory.has_focus(id));
    let fill = if has_focus {
        palette.surface_hover
    } else {
        palette.surface
    };
    ui.painter().rect_filled(rect, height / 2.0, fill);
    if has_focus {
        ui.painter().rect_stroke(
            rect,
            height / 2.0,
            Stroke::new(1.5, palette.accent),
            egui::StrokeKind::Inside,
        );
    }
    let icon_rect =
        Rect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), Vec2::splat(16.0));
    Icon::Search
        .image(palette.secondary, 16.0)
        .paint_at(ui, icon_rect);
    let field_rect = Rect::from_min_max(
        pos2(rect.left() + 34.0, rect.top() + 1.0),
        pos2(rect.right() - 30.0, rect.bottom() - 1.0),
    );
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(field_rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    let response = child.add(
        egui::TextEdit::singleline(text)
            .id(id)
            .hint_text(
                egui::RichText::new(hint)
                    .color(palette.dim)
                    .font(theme::regular(14.0)),
            )
            .font(theme::regular(14.0))
            .text_color(palette.text)
            .frame(egui::Frame::NONE)
            .desired_width(field_rect.width())
            .vertical_align(Align::Center),
    );
    if !text.is_empty() {
        let clear_rect = Rect::from_center_size(
            pos2(rect.right() - 17.0, rect.center().y),
            Vec2::splat(24.0),
        );
        let mut clear = ui.new_child(
            UiBuilder::new()
                .max_rect(clear_rect)
                .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
        );
        if theme::icon_button(
            &mut clear,
            Icon::X,
            15.0,
            palette.secondary,
            palette.text,
            "Clear",
        )
        .clicked()
        {
            text.clear();
            ui.memory_mut(|memory| memory.request_focus(id));
        }
    }
    response
}

/// A toggle drawn as a switch.
pub fn switch(ui: &mut Ui, palette: &Palette, on: &mut bool) -> egui::Response {
    let size = vec2(40.0, 22.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool(response.id, *on);
        let fill = egui::lerp(
            egui::Rgba::from(palette.surface_active)..=egui::Rgba::from(palette.accent),
            t,
        );
        ui.painter()
            .rect_filled(rect, rect.height() / 2.0, Color32::from(fill));
        let knob_x = egui::lerp(rect.left() + 11.0..=rect.right() - 11.0, t);
        ui.painter()
            .circle_filled(pos2(knob_x, rect.center().y), 8.0, Color32::WHITE);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A labelled row in a settings section.
pub fn setting_row(
    ui: &mut Ui,
    palette: &Palette,
    label: &str,
    description: &str,
    control: impl FnOnce(&mut Ui),
) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() - 260.0).max(120.0));
            theme::text(ui, label, theme::medium(14.0), palette.text);
            if !description.is_empty() {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(description)
                            .font(theme::regular(12.5))
                            .color(palette.secondary),
                    )
                    .wrap()
                    .selectable(false),
                );
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), control);
    });
    ui.add_space(10.0);
}

pub fn paint_vertical_gradient(ui: &Ui, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    ui.painter().add(egui::Shape::mesh(mesh));
}

/// A pill with a short label, for "Today" separators and pinned marks.
pub fn chip(ui: &mut Ui, palette: &Palette, label: &str) -> egui::Response {
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), theme::medium(12.0), palette.secondary);
    let size = galley.size() + vec2(20.0, 10.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter()
            .rect_filled(rect, rect.height() / 2.0, palette.panel);
        ui.painter().galley(
            rect.center() - galley.size() / 2.0,
            galley,
            palette.secondary,
        );
    }
    response
}
