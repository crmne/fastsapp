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
                && let Some(annotated) = annotate(text, &rows)
            {
                *text = annotated;
            }
        }
    }
}

/// One message body as drawn: the header to write before it, and the text
/// selection sees.
#[derive(Clone, Debug)]
pub struct Row {
    pub header: String,
    pub body: String,
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
        let line = format!("{}{}", row.header, part);
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
/// one that uses the rest up. Messages with nothing to copy sit between and
/// are skipped.
fn follow(copied: &str, rows: &[Row]) -> Option<Vec<String>> {
    let remaining = copied
        .strip_prefix("\n\n")
        .or_else(|| copied.strip_prefix('\n'))?;
    if remaining.is_empty() {
        return None;
    }
    for (skipped, row) in rows.iter().enumerate() {
        if row.body.is_empty() {
            continue;
        }
        // The rest of the copy is the head of this body: the last message.
        if row.body.starts_with(remaining) {
            return Some(vec![format!("{}{}", row.header, remaining)]);
        }
        // The whole body, with more after it.
        if let Some(rest) = remaining.strip_prefix(row.body.as_str())
            && let Some(mut tail) = follow(rest, &rows[skipped + 1..])
        {
            tail.insert(0, format!("{}{}", row.header, row.body));
            return Some(tail);
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
    fn a_message_with_no_text_sits_silently_between() {
        let with_gap = vec![
            row("[9:00, 1/2/2026] Ada: ", "before the picture"),
            row("[9:01, 1/2/2026] Ada: ", ""),
            row("[9:02, 1/2/2026] Ada: ", "after the picture"),
        ];
        let copied = "the picture\n\nafter the picture";
        assert_eq!(
            annotate(copied, &with_gap).as_deref(),
            Some("[9:00, 1/2/2026] Ada: the picture\n[9:02, 1/2/2026] Ada: after the picture")
        );
    }
}
