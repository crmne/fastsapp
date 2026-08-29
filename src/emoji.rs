//! Colour emoji.
//!
//! epaint draws text from monochrome outlines, so an emoji comes out as a
//! grey silhouette from the face egui bundles. Every desktop carries a
//! colour emoji font as bitmaps (Noto Color Emoji on Linux, Apple Color
//! Emoji on macOS), and this module borrows it: text is laid out with each
//! emoji sequence replaced by a placeholder drawn in transparent ink, and
//! the font's picture is painted over the placeholder afterwards. Flags,
//! skin tones, and joined sequences resolve through the font's own ligature
//! table, the same way a browser finds them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use egui::text::LayoutJob;
use egui::{
    Color32, ColorImage, Pos2, Rect, Stroke, TextFormat, TextureHandle, TextureOptions, vec2,
};
use skrifa::bitmap::BitmapData;
use skrifa::instance::Size;
use skrifa::raw::TableProvider;
use skrifa::raw::tables::gsub::{ExtensionSubtable, LigatureSubstFormat1, SubstitutionLookup};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use unicode_segmentation::UnicodeSegmentation;

/// The character that stands in for an emoji during layout. It comes from
/// the monochrome emoji face egui bundles, where it is a full square, so a
/// sequence of any length takes exactly one emoji's room.
pub const PLACEHOLDER: char = '\u{2B1B}';

/// Pictures are kept this wide; text never asks for more than about 40px.
const TEXTURE_WIDTH: u32 = 72;

struct Font {
    bytes: Vec<u8>,
    index: u32,
    /// A glyph sequence to the glyph the font draws for it.
    ligatures: HashMap<Vec<u32>, u32>,
}

static FONT: OnceLock<Option<Font>> = OnceLock::new();

/// Whether a colour emoji font was found. Without one, text keeps the
/// monochrome glyphs.
pub fn available() -> bool {
    font().is_some()
}

/// Loads the font now rather than at the first emoji, which would stall a
/// frame.
pub fn warm_up() {
    let _ = font();
}

fn font() -> Option<&'static Font> {
    FONT.get_or_init(load).as_ref()
}

fn load() -> Option<Font> {
    let (path, index) = find()?;
    let bytes = std::fs::read(&path).ok()?;
    let font = FontRef::from_index(&bytes, index).ok()?;
    if font.bitmap_strikes().is_empty() {
        log::info!(
            "{} has no bitmap emoji; keeping monochrome glyphs",
            path.display()
        );
        return None;
    }
    let ligatures = read_ligatures(&font);
    log::info!(
        "colour emoji from {} ({} sequences)",
        path.display(),
        ligatures.len()
    );
    Some(Font {
        bytes,
        index,
        ligatures,
    })
}

/// The colour emoji font this desktop carries, and the face inside it.
fn find() -> Option<(PathBuf, u32)> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        candidates.push("/System/Library/Fonts/Apple Color Emoji.ttc".into());
    }
    for dir in [
        "/usr/share/fonts/noto",
        "/usr/share/fonts/truetype/noto",
        "/usr/share/fonts/google-noto-emoji",
        "/usr/share/fonts/noto-emoji",
        "/usr/share/fonts/TTF",
        "/usr/share/fonts",
        "/usr/local/share/fonts",
    ] {
        candidates.push(PathBuf::from(dir).join("NotoColorEmoji.ttf"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.push(home.join(".local/share/fonts/NotoColorEmoji.ttf"));
        candidates.push(home.join(".fonts/NotoColorEmoji.ttf"));
    }
    if let Some(path) = candidates.into_iter().find(|path| path.is_file()) {
        return Some((path, 0));
    }
    // A distribution may file it anywhere under the font root.
    for root in ["/usr/share/fonts", "/usr/local/share/fonts"] {
        if let Some(path) = search(&PathBuf::from(root), 0) {
            return Some((path, 0));
        }
    }
    None
}

fn search(dir: &std::path::Path, depth: usize) -> Option<PathBuf> {
    if depth > 4 {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if let Some(found) = search(&path, depth + 1) {
                return Some(found);
            }
        } else if name.eq_ignore_ascii_case("NotoColorEmoji.ttf") {
            return Some(path);
        }
    }
    None
}

/// Every ligature the font defines, keyed by the glyphs it replaces.
fn read_ligatures(font: &FontRef<'_>) -> HashMap<Vec<u32>, u32> {
    let mut map = HashMap::new();
    let Ok(gsub) = font.gsub() else {
        return map;
    };
    let Ok(lookups) = gsub.lookup_list() else {
        return map;
    };
    for lookup in lookups.lookups().iter().flatten() {
        match lookup {
            SubstitutionLookup::Ligature(lookup) => {
                for subtable in lookup.subtables().iter().flatten() {
                    add_ligatures(&mut map, &subtable);
                }
            }
            SubstitutionLookup::Extension(lookup) => {
                for subtable in lookup.subtables().iter().flatten() {
                    if let ExtensionSubtable::Ligature(extension) = subtable
                        && let Ok(subtable) = extension.extension()
                    {
                        add_ligatures(&mut map, &subtable);
                    }
                }
            }
            _ => {}
        }
    }
    map
}

fn add_ligatures(map: &mut HashMap<Vec<u32>, u32>, subtable: &LigatureSubstFormat1<'_>) {
    let Ok(coverage) = subtable.coverage() else {
        return;
    };
    let firsts: Vec<u32> = coverage
        .iter()
        .map(|glyph| u32::from(glyph.to_u16()))
        .collect();
    for (index, set) in subtable.ligature_sets().iter().enumerate() {
        let (Ok(set), Some(first)) = (set, firsts.get(index)) else {
            continue;
        };
        for ligature in set.ligatures().iter().flatten() {
            let mut key = vec![*first];
            key.extend(
                ligature
                    .component_glyph_ids()
                    .iter()
                    .map(|component| u32::from(component.get().to_u16())),
            );
            map.insert(key, u32::from(ligature.ligature_glyph().to_u16()));
        }
    }
}

impl Font {
    fn font_ref(&self) -> Option<FontRef<'_>> {
        FontRef::from_index(&self.bytes, self.index).ok()
    }

    /// The glyph the font draws for a sequence, resolving joined sequences,
    /// flags, and skin tones through its ligatures.
    fn glyph(&self, font: &FontRef<'_>, cluster: &[char]) -> Option<GlyphId> {
        let charmap = font.charmap();
        let sequence = |chars: &[char]| -> Option<u32> {
            let glyphs: Vec<u32> = chars
                .iter()
                .map(|character| charmap.map(*character as u32).map(|glyph| glyph.to_u32()))
                .collect::<Option<_>>()?;
            match glyphs.as_slice() {
                [single] => Some(*single),
                _ => self.ligatures.get(&glyphs).copied(),
            }
        };
        if let Some(glyph) = sequence(cluster) {
            return Some(GlyphId::new(glyph));
        }
        let mut stripped: Vec<char> = cluster
            .iter()
            .copied()
            .filter(|character| *character != '\u{FE0F}')
            .collect();
        if stripped.len() != cluster.len()
            && let Some(glyph) = sequence(&stripped)
        {
            return Some(GlyphId::new(glyph));
        }
        // An unknown joined sequence: show as much of it as the font knows.
        while stripped.len() > 1 {
            stripped.pop();
            while stripped.last().is_some_and(|c| *c == '\u{200D}') {
                stripped.pop();
            }
            if let Some(glyph) = sequence(&stripped) {
                return Some(GlyphId::new(glyph));
            }
        }
        None
    }

    fn image(&self, font: &FontRef<'_>, glyph: GlyphId) -> Option<ColorImage> {
        let strikes = font.bitmap_strikes();
        let bitmap = strikes.glyph_for_size(Size::new(TEXTURE_WIDTH as f32), glyph)?;
        let rgba = match bitmap.data {
            BitmapData::Png(png) => {
                image::load_from_memory_with_format(png, image::ImageFormat::Png)
                    .ok()?
                    .to_rgba8()
            }
            BitmapData::Bgra(bgra) => {
                let mut buffer =
                    image::RgbaImage::from_raw(bitmap.width, bitmap.height, bgra.to_vec())?;
                for pixel in buffer.pixels_mut() {
                    pixel.0.swap(0, 2);
                }
                buffer
            }
            BitmapData::Mask(_) => return None,
        };
        if rgba.width() == 0 || rgba.height() == 0 {
            return None;
        }
        let height = (TEXTURE_WIDTH * rgba.height() / rgba.width()).max(1);
        let resized = image::imageops::resize(
            &rgba,
            TEXTURE_WIDTH,
            height,
            image::imageops::FilterType::Triangle,
        );
        Some(ColorImage::from_rgba_unmultiplied(
            [TEXTURE_WIDTH as usize, height as usize],
            resized.as_raw(),
        ))
    }
}

/// Textures already uploaded, per egui context.
#[derive(Clone, Default)]
struct Cache(Arc<Mutex<HashMap<String, Option<TextureHandle>>>>);

fn texture(ctx: &egui::Context, cluster: &str) -> Option<TextureHandle> {
    let cache: Cache = ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Cache>(egui::Id::new("emoji-cache"))
            .clone()
    });
    let mut map = cache
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(known) = map.get(cluster) {
        return known.clone();
    }
    let handle = font().and_then(|font| {
        let font_ref = font.font_ref()?;
        let chars: Vec<char> = cluster.chars().collect();
        let glyph = font.glyph(&font_ref, &chars)?;
        let image = font.image(&font_ref, glyph)?;
        Some(ctx.load_texture(format!("emoji-{cluster}"), image, TextureOptions::LINEAR))
    });
    map.insert(cluster.to_owned(), handle.clone());
    handle
}

/// A run of text, or one emoji sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    Text(&'a str),
    Emoji(&'a str),
}

/// Splits text into plain runs and emoji sequences.
pub fn pieces(text: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let mut run_start = 0;
    let mut offset = 0;
    for cluster in text.graphemes(true) {
        if is_emoji(cluster) {
            if run_start < offset {
                pieces.push(Piece::Text(&text[run_start..offset]));
            }
            pieces.push(Piece::Emoji(&text[offset..offset + cluster.len()]));
            run_start = offset + cluster.len();
        }
        offset += cluster.len();
    }
    if run_start < text.len() {
        pieces.push(Piece::Text(&text[run_start..]));
    }
    pieces
}

/// Whether a grapheme cluster is drawn as a picture: a character with
/// emoji presentation, or anything asked for one with a variation
/// selector, keycap, or joiner.
pub fn is_emoji(cluster: &str) -> bool {
    let Some(first) = cluster.chars().next() else {
        return false;
    };
    if (first as u32) < 0xA9 {
        // A digit, '#' or '*' is only an emoji as a keycap.
        return cluster.contains('\u{20E3}');
    }
    if is_presentation(first) {
        return true;
    }
    // A character that is text by default becomes a picture when the
    // sequence asks (a variation selector, joiner, or skin tone), but only
    // if it is one of the symbols that can be pictures: the letters of
    // scripts that use joiners in ordinary words (Devanagari, Bengali,
    // Sinhala, Malayalam) stay text.
    let capable = matches!(first, '\u{A9}' | '\u{AE}')
        || ('\u{2000}'..='\u{33FF}').contains(&first)
        || ('\u{1F000}'..='\u{1FAFF}').contains(&first);
    capable
        && cluster
            .chars()
            .any(|c| matches!(c, '\u{FE0F}' | '\u{20E3}' | '\u{200D}') || is_skin_tone(c))
}

fn is_skin_tone(c: char) -> bool {
    ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
}

/// Characters that are pictures by default, from Unicode's emoji data.
fn is_presentation(c: char) -> bool {
    let code = c as u32;
    if (0x1F000..=0x1FAFF).contains(&code) {
        return !matches!(
            code,
            0x1F170 | 0x1F171 | 0x1F17E | 0x1F17F | 0x1F202 | 0x1F237
        );
    }
    matches!(
        code,
        0x231A | 0x231B | 0x23E9..=0x23EC | 0x23F0 | 0x23F3 | 0x25FD | 0x25FE | 0x2614 | 0x2615
            | 0x2648..=0x2653 | 0x267F | 0x2693 | 0x26A1 | 0x26AA | 0x26AB | 0x26BD | 0x26BE
            | 0x26C4 | 0x26C5 | 0x26CE | 0x26D4 | 0x26EA | 0x26F2 | 0x26F3 | 0x26F5 | 0x26FA
            | 0x26FD | 0x2705 | 0x270A | 0x270B | 0x2728 | 0x274C | 0x274E | 0x2753..=0x2755
            | 0x2757 | 0x2795..=0x2797 | 0x27B0 | 0x27BF | 0x2B1B | 0x2B1C | 0x2B50 | 0x2B55
    )
}

/// How many emoji a text is, when it is nothing but emoji (and spaces).
pub fn only_emoji(text: &str) -> Option<usize> {
    let mut count = 0;
    for piece in pieces(text) {
        match piece {
            Piece::Emoji(_) => count += 1,
            Piece::Text(run) if run.trim().is_empty() => {}
            Piece::Text(_) => return None,
        }
    }
    (count > 0).then_some(count)
}

/// Appends text to a layout job, routing each emoji through a placeholder
/// and remembering which sequence it stands for.
pub fn append(
    job: &mut LayoutJob,
    placements: &mut Vec<String>,
    text: &str,
    format: &TextFormat,
) -> usize {
    let start = job.text.len();
    if !available() {
        job.append(text, 0.0, format.clone());
        return job.text[start..].chars().count();
    }
    for piece in pieces(text) {
        match piece {
            Piece::Text(run) => job.append(run, 0.0, format.clone()),
            Piece::Emoji(cluster) => {
                let mut hidden = format.clone();
                hidden.color = Color32::TRANSPARENT;
                hidden.underline = Stroke::NONE;
                hidden.strikethrough = Stroke::NONE;
                job.append(&PLACEHOLDER.to_string(), 0.0, hidden);
                placements.push(cluster.to_owned());
            }
        }
    }
    job.text[start..].chars().count()
}

/// Paints the pictures over a laid-out galley's placeholders.
pub fn paint(ui: &egui::Ui, galley: &egui::Galley, origin: Pos2, placements: &[String]) {
    if placements.is_empty() {
        return;
    }
    let painter = ui.painter();
    let mut next = 0;
    for row in &galley.rows {
        for glyph in &row.row.glyphs {
            if glyph.chr != PLACEHOLDER {
                continue;
            }
            let Some(cluster) = placements.get(next) else {
                return;
            };
            next += 1;
            let rect = glyph
                .logical_rect()
                .translate(origin.to_vec2() + row.pos.to_vec2());
            match texture(ui.ctx(), cluster) {
                Some(texture) => {
                    let side = rect.height() * 1.08;
                    let size = texture.size_vec2();
                    let scale = (side / size.x).min(side / size.y);
                    let image_rect = Rect::from_center_size(
                        rect.center() + vec2(0.0, rect.height() * 0.02),
                        size * scale,
                    );
                    painter.image(
                        texture.id(),
                        image_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                None => {
                    // The font has no picture for it: draw the sequence in
                    // the monochrome face after all.
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        cluster,
                        egui::FontId::proportional(rect.height() * 0.8),
                        ui.visuals().text_color(),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_split_emoji_out_of_text() {
        assert_eq!(
            pieces("hi 😀 there"),
            vec![
                Piece::Text("hi "),
                Piece::Emoji("😀"),
                Piece::Text(" there")
            ]
        );
        assert_eq!(pieces("plain"), vec![Piece::Text("plain")]);
        // A flag is two regional indicators; a family is joined with ZWJ.
        assert_eq!(pieces("🇩🇪"), vec![Piece::Emoji("🇩🇪")]);
        assert_eq!(pieces("👨‍👩‍👧"), vec![Piece::Emoji("👨‍👩‍👧")]);
        assert_eq!(pieces("👍🏽!"), vec![Piece::Emoji("👍🏽"), Piece::Text("!")]);
    }

    #[test]
    fn presentation_follows_unicode() {
        assert!(is_emoji("❤️"));
        assert!(!is_emoji("❤"));
        assert!(is_emoji("⭐"));
        assert!(is_emoji("1️⃣"));
        assert!(!is_emoji("1"));
        assert!(!is_emoji("©"));
        assert!(is_emoji("©️"));
        assert!(!is_emoji("a"));
    }

    #[test]
    fn emoji_only_messages_are_counted() {
        assert_eq!(only_emoji("😂"), Some(1));
        assert_eq!(only_emoji("😂 🙏"), Some(2));
        assert_eq!(only_emoji("ok 😂"), None);
        assert_eq!(only_emoji(""), None);
    }

    #[test]
    fn placeholders_line_up_with_placements() {
        let mut job = LayoutJob::default();
        let mut placements = Vec::new();
        append(&mut job, &mut placements, "a 😀 b", &TextFormat::default());
        if available() {
            assert_eq!(placements, vec!["😀".to_owned()]);
            assert_eq!(job.text.matches(PLACEHOLDER).count(), 1);
        } else {
            assert!(placements.is_empty());
            assert_eq!(job.text, "a 😀 b");
        }
    }

    #[test]
    fn the_system_font_resolves_sequences_when_present() {
        let Some(font) = font() else {
            return;
        };
        let font_ref = font.font_ref().expect("font parses");
        assert!(font.glyph(&font_ref, &['😀']).is_some());
        let flag: Vec<char> = "🇩🇪".chars().collect();
        assert!(
            font.glyph(&font_ref, &flag).is_some(),
            "flags are ligatures"
        );
        let thumbs: Vec<char> = "👍🏽".chars().collect();
        assert!(
            font.glyph(&font_ref, &thumbs).is_some(),
            "skin tones are ligatures"
        );
        let glyph = font.glyph(&font_ref, &['😀']).expect("glyph");
        let image = font.image(&font_ref, glyph).expect("picture");
        assert_eq!(image.size[0], TEXTURE_WIDTH as usize);
    }
}

#[cfg(test)]
mod script_tests {
    use super::*;

    #[test]
    fn joiners_in_ordinary_words_are_not_emoji() {
        // Sinhala "Sri", Bengali "rya", Malayalam chillu: all carry a ZWJ.
        for word in [
            "\u{0DC1}\u{0DCA}\u{200D}\u{0DBB}\u{0DD3}",
            "\u{09B0}\u{200D}\u{09CD}\u{09AF}",
            "\u{0D28}\u{0D4D}\u{200D}",
        ] {
            assert!(!is_emoji(word), "{word:?} is text");
            assert_eq!(only_emoji(word), None);
        }
    }

    #[test]
    fn sequences_that_ask_to_be_pictures_still_are() {
        for picture in [
            "\u{2764}\u{FE0F}",                    // red heart
            "\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}",  // rainbow flag
            "\u{1F9D1}\u{1F3FD}\u{200D}\u{1F4BB}", // technologist, medium skin
            "\u{23}\u{FE0F}\u{20E3}",              // keycap #
            "\u{1F1EE}\u{1F1F9}",                  // flag of Italy
            "\u{A9}\u{FE0F}",                      // copyright as emoji
        ] {
            assert!(is_emoji(picture), "{picture:?} is a picture");
        }
        assert!(!is_emoji("\u{A9}"), "a bare copyright sign is text");
        assert!(!is_emoji("a"));
    }
}
