//! Work around egui's missing bidirectional layout for RTL scripts.
//!
//! egui 0.36 shapes each font run through harfrust with a guessed direction.
//! Hebrew and Arabic runs are shaped RTL, which reverses glyph order within
//! the run, but egui still places runs left-to-right and does not run the
//! Unicode Bidirectional Algorithm. Inter has no Hebrew/Arabic coverage, so
//! spaces stay on Inter while letters fall back to a script face: each word
//! becomes its own RTL run and appears with its letters reversed.
//!
//! After layout, reverse each visible strong-RTL glyph run (and reattach its
//! mesh) so characters paint in logical order. `LayoutJob` text is unchanged,
//! so selection and copy keep the original string. Full paragraph-level bidi
//! (mixed embeddings, mirroring, Arabic joining in visual order) still needs
//! egui support; this only undoes the per-run letter reversal.

use std::sync::Arc;

use egui::epaint::text::{Galley, Glyph};
use egui::epaint::{Mesh, Vec2};

/// Lays out `job` and restores logical order for RTL script runs.
pub fn layout_job(ui: &egui::Ui, job: egui::text::LayoutJob) -> std::sync::Arc<Galley> {
    let mut galley = ui.painter().layout_job(job);
    restore_logical_order(std::sync::Arc::make_mut(&mut galley));
    galley
}

/// Restores logical character order in a laid-out galley.
pub fn restore_logical_order(galley: &mut Galley) {
    for placed in &mut galley.rows {
        let row = Arc::make_mut(&mut placed.row);
        restore_row(&mut row.glyphs, &mut row.visuals.mesh);
        row.visuals.mesh_bounds = row.visuals.mesh.calc_bounds();
    }
}

fn restore_row(glyphs: &mut [Glyph], mesh: &mut Mesh) {
    let mut index = 0;
    while index < glyphs.len() {
        if is_visible_rtl(glyphs[index]) {
            let start = index;
            index += 1;
            while index < glyphs.len() && continues_rtl_run(glyphs[index]) {
                index += 1;
            }
            reverse_visible_run(&mut glyphs[start..index], mesh);
        } else {
            index += 1;
        }
    }
}

fn is_visible_rtl(glyph: Glyph) -> bool {
    glyph.advance_width > 0.01 && is_rtl(glyph.chr)
}

fn continues_rtl_run(glyph: Glyph) -> bool {
    if glyph.advance_width <= 0.01 {
        return false;
    }
    is_rtl(glyph.chr) || is_nonspacing_mark(glyph.chr)
}

fn reverse_visible_run(glyphs: &mut [Glyph], mesh: &mut Mesh) {
    if glyphs.len() < 2 {
        return;
    }
    let positions: Vec<egui::Pos2> = glyphs.iter().map(|glyph| glyph.pos).collect();
    glyphs.reverse();
    for (glyph, &pos) in glyphs.iter_mut().zip(&positions) {
        let delta = pos.to_vec2() - glyph.pos.to_vec2();
        glyph.pos = pos;
        shift_glyph_mesh(mesh, glyph, delta);
    }
}

fn shift_glyph_mesh(mesh: &mut Mesh, glyph: &Glyph, delta: Vec2) {
    if glyph.uv_rect.is_nothing() || delta == Vec2::ZERO {
        return;
    }
    let start = glyph.first_vertex as usize;
    let end = (start + 4).min(mesh.vertices.len());
    for vertex in &mut mesh.vertices[start..end] {
        vertex.pos += delta;
    }
}

/// Strong right-to-left letters (Unicode bidi classes R and AL).
pub fn is_rtl(c: char) -> bool {
    matches!(
        c,
        '\u{0590}'..='\u{05FF}' // Hebrew
            | '\u{0600}'..='\u{06FF}' // Arabic
            | '\u{0700}'..='\u{074F}' // Syriac
            | '\u{0750}'..='\u{077F}' // Arabic Supplement
            | '\u{0780}'..='\u{07BF}' // Thaana
            | '\u{07C0}'..='\u{07FF}' // NKo
            | '\u{08A0}'..='\u{08FF}' // Arabic Extended-A
            | '\u{FB1D}'..='\u{FB4F}' // Hebrew presentation forms
            | '\u{FB50}'..='\u{FDFF}' // Arabic presentation forms-A
            | '\u{FE70}'..='\u{FEFF}' // Arabic presentation forms-B
    )
}

fn is_nonspacing_mark(c: char) -> bool {
    matches!(
        c,
        '\u{0591}'..='\u{05C7}' | '\u{064B}'..='\u{065F}' | '\u{0670}' | '\u{06D6}'..='\u{06ED}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::text::{FontData, FontDefinitions, FontFamily, LayoutJob, TextFormat};
    use egui::{Color32, FontId, Pos2, vec2};
    use std::sync::Arc;

    /// Inter plus a face that covers Hebrew/Arabic, matching the app's fallback
    /// split (spaces stay on Inter; letters go to another font).
    fn layout_like_app(text: &str) -> Galley {
        const CANDIDATES: &[&str] = &[
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/truetype/noto/NotoSansHebrew-Regular.ttf",
            "/usr/share/fonts/truetype/noto/NotoSansArabic-Regular.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/Library/Fonts/Arial Unicode.ttf",
            r"C:\Windows\Fonts\arial.ttf",
            r"C:\Windows\Fonts\tahoma.ttf",
        ];
        let path = CANDIDATES
            .iter()
            .copied()
            .find(|path| std::path::Path::new(path).is_file())
            .expect("install a Hebrew/Arabic-capable sans (DejaVu, Liberation, Arial) for RTL layout tests");
        layout_with_fallback(text, path, "rtl-fallback")
    }

    fn layout_with_fallback(text: &str, fallback_path: &str, fallback_name: &str) -> Galley {
        let ctx = egui::Context::default();
        let mut fonts = FontDefinitions::default();
        let inter = include_bytes!("../assets/fonts/InterVariable.ttf");
        fonts
            .font_data
            .insert("inter".into(), Arc::new(FontData::from_static(inter)));
        let face = std::fs::read(fallback_path)
            .unwrap_or_else(|error| panic!("read {fallback_path}: {error}"));
        fonts
            .font_data
            .insert(fallback_name.into(), Arc::new(FontData::from_owned(face)));
        fonts.families.insert(
            FontFamily::Proportional,
            vec!["inter".into(), fallback_name.into()],
        );
        fonts
            .families
            .insert(FontFamily::Monospace, vec!["inter".into()]);
        ctx.set_fonts(fonts);

        let galley = std::cell::RefCell::new(None);
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(400.0, 120.0))),
                ..Default::default()
            },
            |ui| {
                let mut job = LayoutJob::default();
                job.append(
                    text,
                    0.0,
                    TextFormat::simple(FontId::proportional(14.0), Color32::WHITE),
                );
                *galley.borrow_mut() = Some(ui.painter().layout_job(job));
            },
        );
        output.textures_delta.clear();
        let mut galley = galley.into_inner().expect("galley");
        // Without the restore, Inter+script fallback reverses letters in each word.
        let broken: String = galley.rows[0]
            .glyphs
            .iter()
            .filter(|glyph| glyph.advance_width > 0.01)
            .map(|glyph| glyph.chr)
            .collect();
        assert_ne!(
            broken, text,
            "egui RTL shaping should reverse letters before our restore"
        );
        if text.chars().any(is_rtl) {
            assert_ne!(
                broken, text,
                "egui RTL shaping should reverse letters before our restore"
            );
        }
        restore_logical_order(Arc::make_mut(&mut galley));
        Arc::try_unwrap(galley).unwrap_or_else(|arc| (*arc).clone())
    }

    fn visible_text(galley: &Galley) -> String {
        galley.rows[0]
            .glyphs
            .iter()
            .filter(|glyph| glyph.advance_width > 0.01)
            .map(|glyph| glyph.chr)
            .collect()
    }

    #[test]
    fn hebrew_words_keep_logical_letter_order() {
        let logical = "הכלב הגדול קפץ";
        let galley = layout_like_app(logical);
        assert_eq!(visible_text(&galley), logical);
        assert_eq!(galley.text(), logical);
    }

    #[test]
    fn mixed_ltr_and_hebrew_keeps_each_side_readable() {
        let text = "OK הכלב end";
        let galley = layout_like_app(text);
        assert_eq!(visible_text(&galley), text);
        assert_eq!(galley.text(), text);
    }

    #[test]
    fn arabic_letters_keep_logical_order() {
        let logical = "مرحبا بالعالم";
        let galley = layout_like_app(logical);
        assert_eq!(visible_text(&galley), logical);
        assert_eq!(galley.text(), logical);
    }

    #[test]
    fn rtl_detection_covers_hebrew_and_arabic() {
        assert!(is_rtl('א'));
        assert!(is_rtl('ب'));
        assert!(!is_rtl('A'));
        assert!(!is_rtl(' '));
    }
}
