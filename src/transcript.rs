//! The form WhatsApp gives copied conversation: when a selection runs
//! across several messages, each line carries the clock, the date, and who
//! wrote it, like `[22:41, 8/18/2026] Ada: the analyzer is crazy good`.
//!
//! egui's selection copies the raw galley text. Every message body drawn in
//! a frame is remembered with its header, and when the copied text turns
//! out to walk across more than one of those bodies, it is rebuilt with the
//! headers in.

/// The plugin that does the rebuilding. egui's selection queues the copied
/// text from its own end-of-pass hook, after the app's frame code has run,
/// so the rewrite happens in `output_hook`, the last look at the output
/// before the backend takes it.
pub struct CopyAnnotator {
    /// Filled by the frame with every message body drawn, in order.
    pub rows: std::sync::Arc<std::sync::Mutex<Vec<Row>>>,
}

impl egui::plugin::Plugin for CopyAnnotator {
    fn debug_name(&self) -> &'static str {
        "fastsapp-transcript"
    }

    fn output_hook(&mut self, _ctx: &egui::Context, output: &mut egui::FullOutput) {
        let rows = self.rows.lock().unwrap_or_else(|p| p.into_inner());
        if rows.is_empty() {
            return;
        }
        for command in &mut output.platform_output.commands {
            if let egui::OutputCommand::CopyText(text) = command
                && let Some(refined) = refine(text, &rows)
            {
                *text = refined;
            }
        }
    }
}

/// One message body as drawn: the header to write before it, the text
/// selection sees, and the emoji behind the body's placeholder glyphs (the
/// galley holds `emoji::PLACEHOLDER` where a colour emoji is painted, and a
/// copy must put the emoji themselves back).
#[derive(Clone, Debug, Default)]
pub struct Row {
    pub header: String,
    pub body: String,
    pub placements: Vec<String>,
    /// What kind of message this is, when it is not plain text:
    /// "\[photo\]", "\[video\]", "\[document: notes.pdf\]", and so on.
    pub marker: Option<String>,
    /// The reactions on the message, preformatted: " (❤️ Roberta)".
    pub reactions: String,
    /// What the message replied to, preformatted:
    /// `(replying to Roberta: "…")`.
    pub quote: Option<String>,
}

impl Row {
    /// One transcript line: the header, the reply context, the kind, the
    /// selected text, and the reactions.
    fn line(&self, body: &str) -> String {
        let mut line = self.header.clone();
        if let Some(quote) = &self.quote {
            line.push_str(quote);
            line.push(' ');
        }
        if let Some(marker) = &self.marker {
            line.push_str(marker);
            if !body.is_empty() {
                line.push(' ');
            }
        }
        line.push_str(body);
        line.push_str(&self.reactions);
        line
    }

    /// The stretch of this row's body from `start`, with every placeholder
    /// replaced by the emoji it stood for.
    fn restored(&self, start: usize, segment: &str) -> String {
        if self.placements.is_empty() {
            return segment.to_owned();
        }
        let mut next = self.body[..start]
            .chars()
            .filter(|&c| c == crate::emoji::PLACEHOLDER)
            .count();
        let mut out = String::with_capacity(segment.len());
        for character in segment.chars() {
            if character == crate::emoji::PLACEHOLDER {
                match self.placements.get(next) {
                    Some(emoji) => out.push_str(emoji),
                    None => out.push(character),
                }
                next += 1;
            } else {
                out.push(character);
            }
        }
        out
    }
}

/// What a copy needs done: headers put on when it spans messages, and the
/// emoji restored either way. `None` when the text is not from the
/// conversation (a field's copy, say) or needs nothing.
pub fn refine(copied: &str, rows: &[Row]) -> Option<String> {
    if let Some(annotated) = annotate(copied, rows) {
        return Some(annotated);
    }
    // Within one message: no header, but the emoji come back.
    if !copied.contains(crate::emoji::PLACEHOLDER) {
        return None;
    }
    for row in rows {
        if let Some(start) = row.body.find(copied) {
            return Some(row.restored(start, copied));
        }
    }
    None
}

/// Rebuilds copied text with a header per message, when it spans more than
/// one of `rows`; anything else is left as it is (`None`).
///
/// A selection across messages takes a tail of the first body, whole bodies
/// in between (messages without text contribute nothing), and a head of the
/// last; egui joins bodies with one blank line or none.
pub fn annotate(copied: &str, rows: &[Row]) -> Option<String> {
    for start in 0..rows.len() {
        if let Some(lines) = walk(copied, &rows[start..])
            && lines.len() >= 2
        {
            return Some(lines.join("\n"));
        }
    }
    None
}

/// Tries to read `copied` as a tail of the first row's body followed by
/// later rows; answers with one annotated line per row touched.
fn walk(copied: &str, rows: &[Row]) -> Option<Vec<String>> {
    let row = rows.first()?;
    // The longest tail first: a selection that starts mid-word still ends
    // at this body's end, since it carried on into the next message.
    for cut in 0..row.body.len() {
        if !row.body.is_char_boundary(cut) {
            continue;
        }
        let part = &row.body[cut..];
        let Some(rest) = copied.strip_prefix(part) else {
            continue;
        };
        let line = row.line(&row.restored(cut, part));
        if rest.is_empty() {
            return Some(vec![line]);
        }
        if let Some(mut tail) = follow(rest, &rows[1..]) {
            tail.insert(0, line);
            return Some(tail);
        }
    }
    None
}

/// After a separator, the next contribution is a whole body, or a head of
/// one that uses the rest up. A message with no text between two that
/// contribute was swept over too: its kind is said on a line of its own.
fn follow(copied: &str, rows: &[Row]) -> Option<Vec<String>> {
    let remaining = copied
        .strip_prefix("\n\n")
        .or_else(|| copied.strip_prefix('\n'))?;
    if remaining.is_empty() {
        return None;
    }
    let mut passed: Vec<String> = Vec::new();
    for (skipped, row) in rows.iter().enumerate() {
        if row.body.is_empty() {
            if row.marker.is_some() {
                passed.push(row.line(""));
            }
            continue;
        }
        // The rest of the copy is the head of this body: the last message.
        if row.body.starts_with(remaining) {
            passed.push(row.line(&row.restored(0, remaining)));
            return Some(passed);
        }
        // The whole body, with more after it.
        if let Some(rest) = remaining.strip_prefix(row.body.as_str())
            && let Some(tail) = follow(rest, &rows[skipped + 1..])
        {
            passed.push(row.line(&row.restored(0, &row.body)));
            passed.extend(tail);
            return Some(passed);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(header: &str, body: &str) -> Row {
        Row {
            header: header.to_owned(),
            body: body.to_owned(),
            placements: body
                .chars()
                .filter(|&c| c == crate::emoji::PLACEHOLDER)
                .map(|_| "🎉".to_owned())
                .collect(),
            ..Default::default()
        }
    }

    fn rows() -> Vec<Row> {
        vec![
            row("[22:41, 8/18/2026] Carmine: ", "the analyzer is crazy good"),
            row(
                "[18:21, 8/30/2026] Ada: ",
                "Hello from France!\nSee you soon",
            ),
            row("[18:27, 8/30/2026] Carmine: ", "Sure I'll take a look"),
        ]
    }

    #[test]
    fn a_span_of_messages_gets_a_header_each() {
        let copied = "crazy good\nHello from France!\nSee you soon\nSure";
        assert_eq!(
            annotate(copied, &rows()).as_deref(),
            Some(
                "[22:41, 8/18/2026] Carmine: crazy good\n\
                 [18:21, 8/30/2026] Ada: Hello from France!\nSee you soon\n\
                 [18:27, 8/30/2026] Carmine: Sure"
            )
        );
        // A blank line between bubbles reads the same.
        let gapped = "good\n\nHello from France!\nSee you soon";
        assert_eq!(
            annotate(gapped, &rows()).as_deref(),
            Some(
                "[22:41, 8/18/2026] Carmine: good\n\
                 [18:21, 8/30/2026] Ada: Hello from France!\nSee you soon"
            )
        );
    }

    #[test]
    fn within_one_message_nothing_changes() {
        assert_eq!(annotate("analyzer is crazy", &rows()), None);
        assert_eq!(annotate("the analyzer is crazy good", &rows()), None);
        assert_eq!(annotate("something else entirely", &rows()), None);
        assert_eq!(annotate("", &rows()), None);
        assert_eq!(annotate("anything", &[]), None);
    }

    #[test]
    fn emoji_come_back_out_of_their_placeholders() {
        let placeholder = crate::emoji::PLACEHOLDER;
        let rows = vec![Row {
            header: "[9:00, 1/2/2026] Ada: ".to_owned(),
            body: format!("well {placeholder} done {placeholder}"),
            placements: vec!["🎂".to_owned(), "🎈".to_owned()],
            ..Default::default()
        }];
        // Within the one message: emoji restored, no header.
        assert_eq!(
            refine(&format!("{placeholder} done {placeholder}"), &rows).as_deref(),
            Some("🎂 done 🎈")
        );
        assert_eq!(
            refine(&format!("done {placeholder}"), &rows).as_deref(),
            Some("done 🎈"),
            "the offset counts placeholders before the selection"
        );
        assert_eq!(refine("well", &rows), None, "nothing to do");
        // Across messages the headers carry restored bodies too.
        let mut both = rows.clone();
        both.push(Row {
            header: "[9:01, 1/2/2026] Ada: ".to_owned(),
            body: "and again".to_owned(),
            placements: Vec::new(),
            ..Default::default()
        });
        assert_eq!(
            refine(&format!("done {placeholder}\nand again"), &both).as_deref(),
            Some("[9:00, 1/2/2026] Ada: done 🎈\n[9:01, 1/2/2026] Ada: and again")
        );
    }

    #[test]
    fn a_swept_over_picture_is_named_and_an_unknown_stays_silent() {
        let mut photo = row("[9:01, 1/2/2026] Ada: ", "");
        photo.marker = Some("[photo]".to_owned());
        let with_gap = vec![
            row("[9:00, 1/2/2026] Ada: ", "before the picture"),
            photo,
            row("[9:02, 1/2/2026] Ada: ", "after the picture"),
        ];
        let copied = "the picture\n\nafter the picture";
        assert_eq!(
            annotate(copied, &with_gap).as_deref(),
            Some(
                "[9:00, 1/2/2026] Ada: the picture\n[9:01, 1/2/2026] Ada: [photo]\n[9:02, 1/2/2026] Ada: after the picture"
            )
        );
        let silent = vec![
            row("[9:00, 1/2/2026] Ada: ", "before the picture"),
            row("[9:01, 1/2/2026] Ada: ", ""),
            row("[9:02, 1/2/2026] Ada: ", "after the picture"),
        ];
        assert_eq!(
            annotate(copied, &silent).as_deref(),
            Some("[9:00, 1/2/2026] Ada: the picture\n[9:02, 1/2/2026] Ada: after the picture")
        );
    }

    #[test]
    fn reactions_quotes_and_kinds_ride_along() {
        let mut first = row("[9:00, 1/2/2026] Ada: ", "look at this");
        first.marker = Some("[photo]".to_owned());
        first.reactions = " (❤️ You)".to_owned();
        let mut second = row("[9:05, 1/2/2026] You: ", "lovely");
        second.quote = Some("(replying to Ada: \"look at this\")".to_owned());
        let copied = "at this\nlovely";
        assert_eq!(
            annotate(copied, &[first, second]).as_deref(),
            Some(
                "[9:00, 1/2/2026] Ada: [photo] at this (❤️ You)\n[9:05, 1/2/2026] You: (replying to Ada: \"look at this\") lovely"
            )
        );
    }
}
