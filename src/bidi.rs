//! Paragraph-level run order for RTL scripts.
//!
//! egui 0.36 shapes each font run with harfrust (within-run RTL shaping is
//! already correct) and then places those runs left-to-right. Inter has no
//! Hebrew/Arabic coverage, so spaces stay on Inter while letters fall back to
//! another face. Each word is its own RTL run. Laid out LTR, the first logical
//! word sits on the left; a Hebrew reader starting from the right reads the
//! last word first (`הכלב הגדול קפץ` reads as `קפץ הגדול הכלב`).
//!
//! After line breaking, reverse the order of runs on an RTL paragraph and
//! leave glyphs inside each run alone. `LayoutJob` text and glyph vec order
//! stay as laid out, so selection and copy are unchanged.

use std::ops::Range;
use std::sync::Arc;

use egui::epaint::text::{Galley, Glyph};
use egui::epaint::{Mesh, Vec2};

/// Lays out `job` and reorders RTL paragraph runs for visual word order.
pub fn layout_job(ui: &egui::Ui, job: egui::text::LayoutJob) -> std::sync::Arc<Galley> {
    let mut galley = ui.painter().layout_job(job);
    reorder_rtl_runs(std::sync::Arc::make_mut(&mut galley));
    galley
}

/// Places RTL-paragraph runs in visual order without touching within-run shaping.
pub fn reorder_rtl_runs(galley: &mut Galley) {
    if !paragraph_rtl(galley.text()) {
        return;
    }
    for placed in &mut galley.rows {
        let row = Arc::make_mut(&mut placed.row);
        reorder_row(&mut row.glyphs, &mut row.visuals.mesh);
        row.visuals.mesh_bounds = row.visuals.mesh.calc_bounds();
    }
}

fn reorder_row(glyphs: &mut [Glyph], mesh: &mut Mesh) {
    let runs = split_runs(glyphs);
    if runs.len() < 2 || !rtl_runs_placed_ltr(glyphs, &runs) {
        return;
    }

    let packed: Vec<(f32, Vec<egui::Pos2>)> = runs
        .iter()
        .map(|run| {
            let slice = &glyphs[run.clone()];
            let origin = min_x(slice);
            let width = slice
                .iter()
                .map(Glyph::max_x)
                .fold(f32::NEG_INFINITY, f32::max)
                - origin;
            let rel = slice
                .iter()
                .map(|glyph| egui::Pos2::new(glyph.pos.x - origin, glyph.pos.y))
                .collect();
            (width.max(0.0), rel)
        })
        .collect();

    let mut x = min_x(glyphs);
    for (run, (width, rel)) in runs.iter().rev().zip(packed.iter().rev()) {
        for (glyph, rel_pos) in glyphs[run.clone()].iter_mut().zip(rel) {
            let new_pos = egui::Pos2::new(x + rel_pos.x, rel_pos.y);
            let delta = new_pos.to_vec2() - glyph.pos.to_vec2();
            glyph.pos = new_pos;
            shift_glyph_mesh(mesh, glyph, delta);
        }
        x += width;
    }
}

/// Font-split RTL words are concatenated in logical (LTR) x order. A single
/// harfrust RTL line is already visual order; reversing that again would undo it.
/// Glyph vec order stays LTR after we move positions, so this is also idempotent.
fn rtl_runs_placed_ltr(glyphs: &[Glyph], runs: &[Range<usize>]) -> bool {
    let mut xs = Vec::new();
    for run in runs {
        if !is_rtl_item(&glyphs[run.start]) {
            continue;
        }
        let slice = &glyphs[run.clone()];
        if !slice.iter().any(is_rtl_letter) {
            continue;
        }
        xs.push(min_x(slice));
    }
    xs.len() >= 2 && xs.windows(2).all(|pair| pair[0] < pair[1])
}

fn min_x(glyphs: &[Glyph]) -> f32 {
    glyphs
        .iter()
        .map(|glyph| glyph.pos.x)
        .fold(f32::INFINITY, f32::min)
}

fn split_runs(glyphs: &[Glyph]) -> Vec<Range<usize>> {
    let mut runs = Vec::new();
    let mut index = 0;
    while index < glyphs.len() {
        let rtl = is_rtl_item(&glyphs[index]);
        let start = index;
        index += 1;
        while index < glyphs.len() && is_rtl_item(&glyphs[index]) == rtl {
            index += 1;
        }
        runs.push(start..index);
    }
    runs
}

/// Script run membership. Diacritics (often ~0 advance) stay with the letter;
/// never early-out on `advance_width`.
fn is_rtl_item(glyph: &Glyph) -> bool {
    is_rtl(glyph.chr) || is_nonspacing_mark(glyph.chr)
}

fn is_rtl_letter(glyph: &Glyph) -> bool {
    glyph.advance_width > 0.01 && is_rtl(glyph.chr) && !is_nonspacing_mark(glyph.chr)
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

fn paragraph_rtl(text: &str) -> bool {
    text.chars().find_map(strong_rtl).unwrap_or(false)
}

fn strong_rtl(c: char) -> Option<bool> {
    if is_rtl(c) && !is_nonspacing_mark(c) {
        Some(true)
    } else if is_ltr(c) {
        Some(false)
    } else {
        None
    }
}

fn is_ltr(c: char) -> bool {
    c.is_ascii_alphabetic() || c.is_ascii_digit() || ('\u{00C0}'..='\u{024F}').contains(&c)
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

    fn layout_raw(text: &str) -> Galley {
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
            .expect(
                "install a Hebrew/Arabic-capable sans (DejaVu, Liberation, Arial) for RTL layout tests",
            );
        let ctx = egui::Context::default();
        let mut fonts = FontDefinitions::default();
        let inter = include_bytes!("../assets/fonts/InterVariable.ttf");
        fonts
            .font_data
            .insert("inter".into(), Arc::new(FontData::from_static(inter)));
        let face = std::fs::read(path).unwrap_or_else(|error| panic!("read {path}: {error}"));
        fonts
            .font_data
            .insert("rtl-fallback".into(), Arc::new(FontData::from_owned(face)));
        fonts.families.insert(
            FontFamily::Proportional,
            vec!["inter".into(), "rtl-fallback".into()],
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
        let galley = galley.into_inner().expect("galley");
        Arc::try_unwrap(galley).unwrap_or_else(|arc| (*arc).clone())
    }

    fn layout_fixed(text: &str) -> Galley {
        let mut galley = layout_raw(text);
        reorder_rtl_runs(&mut galley);
        galley
    }

    /// Visible RTL letter groups in left-to-right x order (word/run metric).
    fn rtl_words_ltr(galley: &Galley) -> Vec<Vec<char>> {
        let glyphs = &galley.rows[0].glyphs;
        let mut words = Vec::new();
        for run in split_runs(glyphs) {
            if !is_rtl_item(&glyphs[run.start]) {
                continue;
            }
            let mut letters = Vec::new();
            let mut x = f32::INFINITY;
            for glyph in &glyphs[run] {
                if is_rtl_letter(glyph) {
                    x = x.min(glyph.pos.x);
                    letters.push(glyph.chr);
                }
            }
            if !letters.is_empty() {
                words.push((x, letters));
            }
        }
        words.sort_by(|a, b| a.0.total_cmp(&b.0));
        words.into_iter().map(|(_, letters)| letters).collect()
    }

    fn sorted(letters: &[char]) -> Vec<char> {
        let mut letters = letters.to_vec();
        letters.sort_unstable();
        letters
    }

    fn charset(word: &str) -> Vec<char> {
        sorted(
            &word
                .chars()
                .filter(|c| is_rtl(*c) && !is_nonspacing_mark(*c))
                .collect::<Vec<_>>(),
        )
    }

    fn visible_by_x(galley: &Galley) -> Vec<char> {
        let mut glyphs: Vec<_> = galley.rows[0]
            .glyphs
            .iter()
            .filter(|glyph| glyph.advance_width > 0.01)
            .cloned()
            .collect();
        glyphs.sort_by(|a, b| a.pos.x.total_cmp(&b.pos.x));
        glyphs.into_iter().map(|glyph| glyph.chr).collect()
    }

    #[test]
    fn hebrew_word_order_is_ltr_before_reorder() {
        let logical = "הכלב הגדול קפץ";
        let galley = layout_raw(logical);
        let words = rtl_words_ltr(&galley);
        assert_eq!(
            words.len(),
            3,
            "Inter fallback should split on spaces: {words:?}"
        );
        assert_eq!(
            sorted(&words[0]),
            charset("הכלב"),
            "failure mode: first logical word is leftmost, so reading RTL hits the last word first"
        );
        assert_eq!(sorted(&words[2]), charset("קפץ"));
        assert_eq!(galley.text(), logical);
    }

    #[test]
    fn hebrew_words_read_rtl_after_run_reorder() {
        let logical = "הכלב הגדול קפץ";
        let before = layout_raw(logical);
        let before_words = rtl_words_ltr(&before);
        let mut galley = before.clone();
        reorder_rtl_runs(&mut galley);
        let after = rtl_words_ltr(&galley);
        assert_eq!(sorted(&after[0]), charset("קפץ"), "{after:?}");
        assert_eq!(sorted(&after[1]), charset("הגדול"));
        assert_eq!(sorted(&after[2]), charset("הכלב"));
        let dog = before_words
            .iter()
            .find(|word| sorted(word) == charset("הכלב"))
            .expect("dog");
        let dog_after = after
            .iter()
            .find(|word| sorted(word) == charset("הכלב"))
            .expect("dog after");
        assert_eq!(
            dog, dog_after,
            "within-run shaping must stay (do not reverse letters inside a word)"
        );
        assert_eq!(galley.text(), logical);
        reorder_rtl_runs(&mut galley);
        assert_eq!(
            rtl_words_ltr(&galley),
            after,
            "run reorder must be idempotent"
        );
    }

    #[test]
    fn mixed_ltr_paragraph_keeps_latin_on_the_left() {
        let text = "OK הכלב end";
        let galley = layout_fixed(text);
        let visible = visible_by_x(&galley);
        assert_eq!(visible.first().copied(), Some('O'));
        assert_eq!(visible.last().copied(), Some('d'));
        let words = rtl_words_ltr(&galley);
        assert_eq!(words.len(), 1);
        assert_eq!(sorted(&words[0]), charset("הכלב"));
        assert_eq!(galley.text(), text);
    }

    #[test]
    fn arabic_words_read_rtl_after_run_reorder() {
        let logical = "مرحبا بالعالم";
        let galley = layout_fixed(logical);
        let words = rtl_words_ltr(&galley);
        assert!(
            words.len() >= 2,
            "expected space-split Arabic runs, got {words:?}"
        );
        assert_eq!(sorted(words.first().unwrap()), charset("بالعالم"));
        assert_eq!(sorted(words.last().unwrap()), charset("مرحبا"));
        assert_eq!(galley.text(), logical);
    }

    #[test]
    fn hebrew_niqqud_stays_in_the_letter_run() {
        let logical = "שָׁלוֹם עוֹלָם";
        let galley = layout_fixed(logical);
        let glyphs = &galley.rows[0].glyphs;
        let mark_runs = split_runs(glyphs).into_iter().filter(|run| {
            glyphs[run.clone()]
                .iter()
                .any(|glyph| is_nonspacing_mark(glyph.chr))
        });
        for run in mark_runs {
            assert!(
                glyphs[run].iter().any(is_rtl_letter),
                "diacritics must stay with their letter run (no advance_width early-out)"
            );
        }
        let words = rtl_words_ltr(&galley);
        assert_eq!(words.len(), 2, "{words:?}");
        assert_eq!(sorted(&words[0]), charset("עוֹלָם"));
        assert_eq!(sorted(&words[1]), charset("שָׁלוֹם"));
    }

    #[test]
    fn rtl_detection_covers_hebrew_and_arabic() {
        assert!(is_rtl('א'));
        assert!(is_rtl('ب'));
        assert!(!is_rtl('A'));
        assert!(!is_rtl(' '));
        assert!(paragraph_rtl("הכלב הגדול קפץ"));
        assert!(!paragraph_rtl("OK הכלב end"));
        assert!(is_nonspacing_mark('\u{05B8}'));
        assert!(!is_rtl(' '));
    }
}
