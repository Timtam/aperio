//! Raw iCalendar content lines, for the parts of a resource Aperio carries
//! through verbatim.
//!
//! A PUT replaces a whole resource, and the `icalendar` crate re-serialises
//! what it parsed: it reorders parameters, quotes a value only when it holds
//! a colon or a semicolon, and knows nothing of what Aperio does not model.
//! Properties that belong to someone else — the ORGANIZER and ATTENDEE lines
//! of a meeting above all — are therefore kept as the server's own text,
//! folding and line endings included, and spliced back in by position
//! (see `recurrence_id_lines` and `override_to_vevent` for the same rule).

use std::ops::Range;

/// One property of a component: where its text sits (the first physical line
/// to the end of its last continuation line, line ending included), its name,
/// its parameters and its value, unfolded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContentLine {
    pub range: Range<usize>,
    /// Upper-cased.
    pub name: String,
    /// Names upper-cased, values without their surrounding quotes.
    pub params: Vec<(String, String)>,
    pub value: String,
}

impl ContentLine {
    /// The value of the parameter `name` (case-insensitive), if present.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// The properties of the VEVENT (or other component) that `block` holds,
/// never those of a component nested inside it: an ATTENDEE inside a VALARM
/// is the recipient of an email alarm, not a guest. BEGIN and END lines are
/// not properties.
pub(crate) fn component_lines(block: &str) -> Vec<ContentLine> {
    let mut out = Vec::new();
    let mut depth: usize = 0;
    for (range, unfolded) in logical_lines(block) {
        let Some((name, params, value)) = split_line(&unfolded) else {
            continue;
        };
        match name.as_str() {
            "BEGIN" => {
                depth += 1;
                continue;
            }
            "END" => {
                depth = depth.saturating_sub(1);
                continue;
            }
            _ => {}
        }
        if depth == 1 {
            out.push(ContentLine {
                range,
                name,
                params,
                value,
            });
        }
    }
    out
}

/// Every logical line of `text`: the byte range of its physical lines and
/// its unfolded content without the line ending. A continuation line starts
/// with a space or a tab (RFC 5545 §3.1).
fn logical_lines(text: &str) -> Vec<(Range<usize>, String)> {
    let mut out: Vec<(Range<usize>, String)> = Vec::new();
    let mut offset = 0;
    for physical in text.split_inclusive('\n') {
        let start = offset;
        offset += physical.len();
        let content = physical.trim_end_matches(['\r', '\n']);
        let continued = physical.starts_with(' ') || physical.starts_with('\t');
        match out.last_mut() {
            Some((range, unfolded)) if continued => {
                range.end = offset;
                unfolded.push_str(&content[1..]);
            }
            _ => out.push((start..offset, content.to_string())),
        }
    }
    out
}

/// `NAME;PARAM=value;…:value` into its parts. The value starts after the
/// first colon outside double quotes; parameters split at semicolons outside
/// them. `None` for a line without a colon.
fn split_line(line: &str) -> Option<(String, Vec<(String, String)>, String)> {
    let mut in_quotes = false;
    let mut cuts = Vec::new();
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ';' if !in_quotes => cuts.push(i),
            ':' if !in_quotes => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let head = &line[..colon];
    let value = line[colon + 1..].to_string();
    let mut pieces = Vec::new();
    let mut from = 0;
    for cut in cuts {
        pieces.push(&head[from..cut]);
        from = cut + 1;
    }
    pieces.push(&head[from..]);
    let name = pieces[0].trim().to_ascii_uppercase();
    let params = pieces[1..]
        .iter()
        .filter_map(|p| p.split_once('='))
        .map(|(n, v)| {
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(v);
            (n.trim().to_ascii_uppercase(), v.to_string())
        })
        .collect();
    Some((name, params, value))
}

/// The line ending `block` uses: CRLF unless it has only bare LFs.
pub(crate) fn line_ending(block: &str) -> &'static str {
    if block.contains("\r\n") || !block.contains('\n') {
        "\r\n"
    } else {
        "\n"
    }
}

/// `line` (without an ending) as physical lines of at most 75 octets each,
/// every one ending in `ending`; a continuation starts with a space. Never
/// splits a UTF-8 character (RFC 5545 §3.1).
pub(crate) fn fold(line: &str, ending: &str) -> String {
    let mut out = String::with_capacity(line.len() + line.len() / 70 * 3 + 2);
    let mut width = 0;
    let mut first = true;
    for c in line.chars() {
        let limit = if first { 75 } else { 74 };
        if width + c.len_utf8() > limit {
            out.push_str(ending);
            out.push(' ');
            width = 0;
            first = false;
        }
        out.push(c);
        width += c.len_utf8();
    }
    out.push_str(ending);
    out
}

/// A parameter value as RFC 5545 §3.2 wants it: in double quotes when it
/// holds a colon, a semicolon or a comma. A double quote cannot be written
/// inside one, so it becomes a single quote.
pub(crate) fn param_value(value: &str) -> String {
    let cleaned = value.replace('"', "'");
    if cleaned.contains([':', ';', ',']) {
        format!("\"{cleaned}\"")
    } else {
        cleaned
    }
}

/// `block` with `lines` inserted right after its head: the BEGIN line and,
/// for an override, its RECURRENCE-ID property. Properties must come before
/// any nested component (RFC 5545 §3.6.1).
pub(crate) fn insert_after_head(block: &str, lines: &str) -> String {
    if lines.is_empty() {
        return block.to_string();
    }
    let mut at = 0;
    for (index, (range, unfolded)) in logical_lines(block).into_iter().enumerate() {
        let name = unfolded
            .split([';', ':'])
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        if index == 0 || name == "RECURRENCE-ID" {
            at = range.end;
            continue;
        }
        break;
    }
    format!("{}{}{}", &block[..at], lines, &block[at..])
}

/// `block` without the text in `drop`.
pub(crate) fn without_ranges(block: &str, drop: &[Range<usize>]) -> String {
    let mut sorted: Vec<&Range<usize>> = drop.iter().collect();
    sorted.sort_by_key(|r| r.start);
    let mut out = String::with_capacity(block.len());
    let mut from = 0;
    for range in sorted {
        if range.start >= from {
            out.push_str(&block[from..range.start]);
            from = range.end;
        }
    }
    out.push_str(&block[from..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: &str = "BEGIN:VEVENT\r\n\
ATTENDEE;CN=Toni Barth;CUTYPE=INDIVIDUAL;PARTSTAT=ACCEPTED;EMAIL=toni@examp\r\n le.org;ROLE=CHAIR:/aB1/principal/\r\n\
ORGANIZER;CN=\"Barth, Toni\";EMAIL=toni@example.org:/aB1/principal/\r\n\
SUMMARY:Sync\r\n\
BEGIN:VALARM\r\n\
ACTION:EMAIL\r\n\
ATTENDEE:mailto:alarm@example.org\r\n\
END:VALARM\r\n\
END:VEVENT\r\n";

    #[test]
    fn component_lines_unfold_and_skip_nested_components() {
        let lines = component_lines(BLOCK);
        let names: Vec<&str> = lines.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["ATTENDEE", "ORGANIZER", "SUMMARY"]);
        assert_eq!(lines[0].param("email"), Some("toni@example.org"));
        assert_eq!(lines[0].value, "/aB1/principal/");
        // The range covers both physical lines, endings included.
        assert_eq!(
            &BLOCK[lines[0].range.clone()],
            "ATTENDEE;CN=Toni Barth;CUTYPE=INDIVIDUAL;PARTSTAT=ACCEPTED;EMAIL=toni@examp\r\n le.org;ROLE=CHAIR:/aB1/principal/\r\n"
        );
        // A quoted parameter with a comma stays one parameter.
        assert_eq!(lines[1].param("CN"), Some("Barth, Toni"));
    }

    #[test]
    fn a_colon_inside_quotes_is_not_the_value() {
        let (_, params, value) =
            split_line("ATTENDEE;DELEGATED-FROM=\"mailto:a@x\";CN=A:mailto:b@x").unwrap();
        assert_eq!(params[0], ("DELEGATED-FROM".into(), "mailto:a@x".into()));
        assert_eq!(value, "mailto:b@x");
    }

    #[test]
    fn fold_keeps_75_octets_and_whole_characters() {
        let line = format!("ATTENDEE;CN={}:mailto:a@x", "ä".repeat(60));
        let folded = fold(&line, "\r\n");
        for physical in folded.split_inclusive("\r\n") {
            assert!(
                physical.trim_end_matches("\r\n").len() <= 75,
                "{physical:?}"
            );
        }
        assert_eq!(logical_lines(&folded)[0].1, line);
    }

    #[test]
    fn param_values_are_quoted_when_rfc_5545_says_so() {
        assert_eq!(param_value("Bob"), "Bob");
        assert_eq!(param_value("Doe, Jane"), "\"Doe, Jane\"");
        assert_eq!(param_value("say \"hi\""), "say 'hi'");
    }

    #[test]
    fn inserts_after_the_begin_line_and_a_recurrence_id() {
        let plain = "BEGIN:VEVENT\r\nSUMMARY:x\r\nEND:VEVENT\r\n";
        assert_eq!(
            insert_after_head(plain, "SEQUENCE:1\r\n"),
            "BEGIN:VEVENT\r\nSEQUENCE:1\r\nSUMMARY:x\r\nEND:VEVENT\r\n"
        );
        let over = "BEGIN:VEVENT\r\nRECURRENCE-ID;TZID=Europe/Berlin:2026\r\n 1109T160000\r\nSUMMARY:x\r\nEND:VEVENT\r\n";
        assert_eq!(
            insert_after_head(over, "SEQUENCE:1\r\n"),
            "BEGIN:VEVENT\r\nRECURRENCE-ID;TZID=Europe/Berlin:2026\r\n 1109T160000\r\nSEQUENCE:1\r\nSUMMARY:x\r\nEND:VEVENT\r\n"
        );
    }

    #[test]
    fn line_endings_and_removal() {
        assert_eq!(line_ending("A\r\nB\r\n"), "\r\n");
        assert_eq!(line_ending("A\nB\n"), "\n");
        let lines = component_lines(BLOCK);
        let kept = without_ranges(BLOCK, &[lines[0].range.clone()]);
        assert!(kept.starts_with("BEGIN:VEVENT\r\nORGANIZER;"), "{kept}");
    }
}
