//! The signature block of an event or task description.
//!
//! Was `shared/signatures.ts`, run by both editors on both surfaces. A
//! signature is a block at the end of a description, opened by a line that is
//! exactly `-- ` (dash dash space, borrowed from mail: RFC 3676 §4.3), the
//! join details of a permanent conference room, a lecturer's consultation
//! hours, a standing "bring your own laptop". Two devices inserting the same
//! signature must write the same bytes, and neither may double what the other
//! inserted; a frontend that is not JavaScript has to write the same block.
//! So the three answers live here, once: where the block starts, what it
//! says, and the description with a body applied — replacing, never stacking.
//!
//! # Plain text, and only plain text
//!
//! RFC 5545 §3.8.1.5 gives DESCRIPTION the TEXT value type. A URL on a line
//! of its own and blank lines for structure: that is the whole formatting
//! vocabulary, and it survives everywhere.
//!
//! # The marker
//!
//! The LAST line whose content is `--` after trailing whitespace is removed
//! (a forwarded invitation can carry somebody else's block above; ours is the
//! one at the end, the rule mail clients apply). The trailing space is part of
//! the convention and is written; it is not required for recognition, because
//! a provider round trip may trim line ends and a block that comes back as
//! `--` is still ours. Leading whitespace is not stripped: an indented `-- `
//! is no marker.
//!
//! # JavaScript's whitespace, not Rust's
//!
//! Which characters count as trailing whitespace on the marker line, and what
//! makes a description "nothing but whitespace", follow JavaScript's `trim`
//! (WhiteSpace + LineTerminator): U+FEFF and U+00A0 count, U+0085 does NOT.
//! Rust's `char::is_whitespace` differs on exactly those (DESIGN §4.5 c), and
//! the fixture pins the difference, so the set is spelled out here rather than
//! borrowed. Lines are split on LF alone; a CR stays part of its line and is
//! trailing whitespace on the marker line.
//!
//! Pinned by `tests/fixtures/signatures.json`, measured from the TypeScript
//! this replaces; the `contract` module below reads it, and so does the
//! TypeScript contract test on the other side of the boundary.

use serde::Deserialize;

/// The separator that opens a signature block: dash dash space. The trailing
/// space is part of the convention and is deliberately written.
pub const SIGNATURE_MARKER: &str = "-- ";

/// What JavaScript's `String.prototype.trim` removes: the WhiteSpace
/// production (TAB, VT, FF, SP, NBSP, ZWNBSP/BOM, and every Zs character) and
/// the LineTerminator production (LF, CR, LS, PS). Not U+0085 NEXT LINE, not
/// U+001C..U+001F — those are whitespace to Rust and not here.
fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{FEFF}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{000A}'
                | '\u{000D}'
                | '\u{2028}'
                | '\u{2029}'
    )
}

fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

fn js_trim_end(s: &str) -> &str {
    s.trim_end_matches(is_js_whitespace)
}

/// The index of the line that opens the signature block, or `None`. The LAST
/// such line wins.
fn marker_line(lines: &[&str]) -> Option<usize> {
    let marker = js_trim_end(SIGNATURE_MARKER);
    lines.iter().rposition(|line| js_trim_end(line) == marker)
}

/// The description without its signature block, and without the blank lines
/// that separated them (so removing and re-adding a signature does not grow a
/// gap each time). A description without a block comes back as it is.
pub fn strip_signature(description: &str) -> String {
    let lines: Vec<&str> = description.split('\n').collect();
    let Some(at) = marker_line(&lines) else {
        return description.to_string();
    };
    let mut end = at;
    while end > 0 && js_trim(lines[end - 1]).is_empty() {
        end -= 1;
    }
    lines[..end].join("\n")
}

/// What the description's signature block says, or `None` when it has none.
/// A block with nothing after the marker is a block: `Some("")`.
pub fn signature_in(description: &str) -> Option<String> {
    let lines: Vec<&str> = description.split('\n').collect();
    let at = marker_line(&lines)?;
    Some(lines[at + 1..].join("\n"))
}

/// The description with `body` as its signature block.
///
/// Replaces an existing block rather than appending a second one, so applying
/// twice — or switching from one signature to another — leaves exactly one.
/// An empty (whitespace-only) body removes the block. A description of
/// nothing but whitespace is a description of nothing: the block opens the
/// text rather than a line of stray spaces. The appointment's own text is
/// never touched — not even its trailing newlines, which keep their place and
/// get the separating gap on top.
pub fn apply_signature(description: &str, body: &str) -> String {
    let stripped = strip_signature(description);
    let base = if js_trim(&stripped).is_empty() {
        ""
    } else {
        stripped.as_str()
    };
    let trimmed = js_trim(body);
    if trimmed.is_empty() {
        return base.to_string();
    }
    let separator = if base.is_empty() { "" } else { "\n\n" };
    format!("{base}{separator}{SIGNATURE_MARKER}\n{trimmed}")
}

/// The question [`apply_signature`] answers, over the wire.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ApplySignatureInput {
    pub description: String,
    pub body: String,
}

/// The question [`signature_in`] and [`strip_signature`] answer, over the
/// wire.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct SignatureTextInput {
    pub description: String,
}

/// [`signature_in`] over the wire: the block's text, or `null`.
pub fn signature_in_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: SignatureTextInput = serde_json::from_str(input_json)?;
    serde_json::to_string(&signature_in(&input.description))
}

/// [`strip_signature`] over the wire.
pub fn strip_signature_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: SignatureTextInput = serde_json::from_str(input_json)?;
    serde_json::to_string(&strip_signature(&input.description))
}

/// [`apply_signature`] over the wire.
pub fn apply_signature_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: ApplySignatureInput = serde_json::from_str(input_json)?;
    serde_json::to_string(&apply_signature(&input.description, &input.body))
}

/// The contract with the TypeScript this replaced.
///
/// Its other half is `src/state/signatures.contract.test.ts`, reading this
/// same file through the doors. The fixture was written by running the
/// TypeScript BEFORE the port, so the port is measured against what was —
/// byte for byte.
#[cfg(test)]
mod contract {
    use super::*;
    use serde_json::{json, Value};

    const CONTRACT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/signatures.json"
    ));

    fn doc() -> Value {
        serde_json::from_str(CONTRACT).expect("the contract parses")
    }

    fn has_row(cases: &[Value], name: &str) {
        assert!(
            cases.iter().any(|c| c["name"] == name),
            "the contract lost `{name}`"
        );
    }

    #[test]
    fn the_marker_is_the_fixtures() {
        assert_eq!(doc()["shape"]["marker"], SIGNATURE_MARKER);
    }

    #[test]
    fn every_text_is_read_and_stripped_alike() {
        let doc = doc();
        let cases = doc["texts"].as_array().expect("texts is an array");
        for needed in [
            "the-last-marker-wins",
            "a-marker-whose-trailing-space-a-provider-ate",
            "an-indented-marker-is-no-marker",
            "several-blank-lines-before-the-block-all-go",
            "crlf-line-endings",
            "trailing-nel-on-the-marker-line",
            "trailing-bom-on-the-marker-line",
        ] {
            has_row(cases, needed);
        }
        for case in cases {
            let description = case["input"]["description"]
                .as_str()
                .expect("a description");
            assert_eq!(
                json!(signature_in(description)),
                case["expect"]["read"],
                "read {}: {}",
                case["name"],
                case["note"]
            );
            assert_eq!(
                json!(strip_signature(description)),
                case["expect"]["strip"],
                "strip {}: {}",
                case["name"],
                case["note"]
            );
        }
    }

    #[test]
    fn every_application_holds() {
        let doc = doc();
        let cases = doc["apply"].as_array().expect("apply is an array");
        for needed in [
            "applying-twice-leaves-one-block",
            "replaces-the-block-when-the-signature-changes",
            "a-whitespace-description-is-nothing",
            "an-empty-body-removes-the-block",
            "a-bom-only-description-is-nothing",
            "a-nel-only-description-is-text",
        ] {
            has_row(cases, needed);
        }
        for case in cases {
            let i = &case["input"];
            let got = apply_signature(
                i["description"].as_str().expect("a description"),
                i["body"].as_str().expect("a body"),
            );
            assert_eq!(
                json!(got),
                case["expect"],
                "{}: {}",
                case["name"],
                case["note"]
            );
        }
    }

    #[test]
    fn the_wire_round_trips() {
        assert_eq!(
            apply_signature_json(r#"{"description": "Agenda.", "body": "Raum 42"}"#)
                .expect("valid"),
            r#""Agenda.\n\n-- \nRaum 42""#
        );
        assert_eq!(
            signature_in_json(r#"{"description": "Nur Text."}"#).expect("valid"),
            "null"
        );
        assert_eq!(
            strip_signature_json(r#"{"description": "Agenda.\n\n-- \nRaum 42"}"#).expect("valid"),
            r#""Agenda.""#
        );
    }
}
