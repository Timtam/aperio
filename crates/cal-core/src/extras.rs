//! "Aperio Extras" — a forward-compatible bag of task fields that have no
//! native home on an external provider (DESIGN.md §9.12).
//!
//! On a SHARED list the only channel Aperio shares with a collaborator is
//! the provider itself, so these fields ride along inside a provider field.
//! This module implements the **visible managed-block** channel used for
//! plain-text description fields (Vikunja / Todoist / Google); the same
//! JSON payload also feeds the invisible custom-property channels (CalDAV
//! `X-`property, EWS extended property, Microsoft Graph open extension).
//!
//! The block is appended to the description with a bilingual "don't edit"
//! warning and a base64 payload — base64 keeps the JSON safe from a
//! rich-text editor's HTML escaping (Vikunja). On read the block is
//! stripped back out so the user-facing description stays clean; a missing
//! or malformed block degrades to "no extras", never an error.
//!
//! `embed` always strips any existing block before appending, so it's
//! idempotent AND is the "defensive merge": pass the freshly-read current
//! description and it replaces only Aperio's block, preserving the rest.

use base64::Engine;
use serde::{Deserialize, Serialize};

/// Current on-wire format version — the payload line is `aperio:<v>:<b64>`.
const FORMAT_VERSION: u32 = 1;
/// Human warning shown above the payload in a visible block.
const WARNING_LINE: &str = "— ⚙ Aperio · bitte nicht bearbeiten / please don't edit —";

/// Forward-compatible bag of Aperio-only task fields. Keys are stable
/// identifiers (e.g. `recurrence`, `resurface_date`, `series_id`); values
/// are arbitrary JSON so the schema can grow without a format break. An
/// empty bag is never written as a block.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AperioExtras {
    fields: serde_json::Map<String, serde_json::Value>,
}

impl AperioExtras {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.fields.get(key)
    }

    /// Set a key. The bag's order is deterministic (serde_json's default
    /// `Map` is sorted), so the encoded payload is stable for the same
    /// content — callers can compare two encodings to decide whether a
    /// write is even needed.
    pub fn insert(&mut self, key: impl Into<String>, value: serde_json::Value) {
        self.fields.insert(key.into(), value);
    }

    pub fn remove(&mut self, key: &str) -> Option<serde_json::Value> {
        self.fields.remove(key)
    }
}

/// Versioned wire envelope: `{ "v": 1, "d": { …fields… } }`.
#[derive(Serialize, Deserialize)]
struct Envelope {
    v: u32,
    #[serde(default)]
    d: serde_json::Map<String, serde_json::Value>,
}

/// `aperio:<v>:<base64(json envelope)>`.
fn to_payload(extras: &AperioExtras) -> String {
    let env = Envelope {
        v: FORMAT_VERSION,
        d: extras.fields.clone(),
    };
    // Serializing a JSON map never fails.
    let json = serde_json::to_vec(&env).unwrap_or_default();
    let b64 = base64::engine::general_purpose::STANDARD.encode(json);
    format!("aperio:{FORMAT_VERSION}:{b64}")
}

/// Decode a payload line back into the bag. Accepts any numeric version
/// (the envelope is generic), so a newer writer degrades to "readable
/// fields" rather than an error. `None` on anything malformed.
fn parse_payload(line: &str) -> Option<AperioExtras> {
    let rest = line.trim().strip_prefix("aperio:")?;
    let (ver, b64) = rest.split_once(':')?;
    if ver.is_empty() || !ver.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .ok()?;
    let env: Envelope = serde_json::from_slice(&bytes).ok()?;
    Some(AperioExtras { fields: env.d })
}

/// Does this line look like an Aperio payload line (`aperio:<digits>:…`)?
fn is_payload_line(line: &str) -> bool {
    line.trim()
        .strip_prefix("aperio:")
        .and_then(|rest| rest.split_once(':'))
        .is_some_and(|(ver, _)| !ver.is_empty() && ver.bytes().all(|b| b.is_ascii_digit()))
}

/// Loose match for the warning line, tolerant of minor manual edits — we
/// only use it to strip the cosmetic line above the payload.
fn is_warning_line(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    l.contains("aperio") && (l.contains("bearbeiten") || l.contains("edit"))
}

/// Split a provider description into `(clean_description, extras)`. A
/// missing or malformed block yields `(description, None)` untouched.
pub fn extract(description: Option<&str>) -> (Option<String>, Option<AperioExtras>) {
    let Some(desc) = description else {
        return (None, None);
    };
    let lines: Vec<&str> = desc.lines().collect();
    let Some(payload_idx) = lines.iter().rposition(|l| is_payload_line(l)) else {
        return (Some(desc.to_string()), None);
    };
    let Some(extras) = parse_payload(lines[payload_idx]) else {
        // Looks like a payload but doesn't decode — leave the text alone.
        return (Some(desc.to_string()), None);
    };

    // Block start: the payload line, plus a recognizable warning line above
    // it, plus any blank separator lines before that.
    let mut start = payload_idx;
    if start >= 1 && is_warning_line(lines[start - 1]) {
        start -= 1;
    }
    while start >= 1 && lines[start - 1].trim().is_empty() {
        start -= 1;
    }

    // Keep everything before the block and anything after the payload line
    // (a user might have typed below it).
    let mut kept: Vec<&str> = Vec::new();
    kept.extend_from_slice(&lines[..start]);
    kept.extend_from_slice(&lines[payload_idx + 1..]);
    let clean = kept.join("\n");
    let clean = clean.trim();
    let clean = if clean.is_empty() {
        None
    } else {
        Some(clean.to_string())
    };
    (clean, Some(extras))
}

/// Produce the provider description that carries `extras` as a visible
/// block. Strips any existing block first (idempotent + defensive merge),
/// so the caller passes the current provider description and gets back the
/// description with only Aperio's block replaced. Empty extras ⇒ just the
/// cleaned description (no block).
pub fn embed(description: Option<&str>, extras: &AperioExtras) -> Option<String> {
    let (clean, _) = extract(description);
    if extras.is_empty() {
        return clean;
    }
    let block = format!("{WARNING_LINE}\n{}", to_payload(extras));
    match clean {
        Some(c) if !c.is_empty() => Some(format!("{c}\n\n{block}")),
        _ => Some(block),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> AperioExtras {
        let mut e = AperioExtras::new();
        e.insert("series_id", json!("abc-123"));
        e.insert("resurface_date", json!("2026-10-01"));
        e.insert(
            "recurrence",
            json!({ "placement": "backlog", "anchor": "from_completion" }),
        );
        e
    }

    #[test]
    fn round_trips_extras_and_keeps_a_clean_description() {
        let extras = sample();
        let stored = embed(Some("Geschirrspüler einräumen"), &extras).unwrap();
        // The user text survives; the payload is HTML-safe base64.
        assert!(stored.starts_with("Geschirrspüler einräumen"));
        assert!(stored.contains("aperio:1:"));
        assert!(!stored.contains('{') && !stored.contains('<'));

        let (clean, back) = extract(Some(&stored));
        assert_eq!(clean.as_deref(), Some("Geschirrspüler einräumen"));
        assert_eq!(back, Some(extras));
    }

    #[test]
    fn empty_extras_writes_no_block() {
        assert_eq!(
            embed(Some("just text"), &AperioExtras::new()).as_deref(),
            Some("just text")
        );
        assert_eq!(embed(None, &AperioExtras::new()), None);
    }

    #[test]
    fn description_without_a_block_is_untouched() {
        let (clean, extras) = extract(Some("ordinary description"));
        assert_eq!(clean.as_deref(), Some("ordinary description"));
        assert!(extras.is_none());
    }

    #[test]
    fn embed_is_idempotent() {
        let extras = sample();
        let once = embed(Some("text"), &extras).unwrap();
        let twice = embed(Some(&once), &extras).unwrap();
        assert_eq!(once, twice, "re-embedding strips the old block first");
        assert_eq!(once.matches("aperio:1:").count(), 1, "no duplicate blocks");
    }

    #[test]
    fn defensive_merge_preserves_a_concurrent_text_edit() {
        // A collaborator changed the description text while the block was
        // present; we re-embed (possibly different) extras and must keep
        // their new text.
        let with_block = embed(Some("old text"), &sample()).unwrap();
        let edited = with_block.replace("old text", "their new text");
        let mut updated = sample();
        updated.insert("resurface_date", json!("2027-04-01"));
        let result = embed(Some(&edited), &updated).unwrap();
        let (clean, back) = extract(Some(&result));
        assert_eq!(clean.as_deref(), Some("their new text"));
        assert_eq!(back, Some(updated));
        assert_eq!(result.matches("aperio:1:").count(), 1);
    }

    #[test]
    fn malformed_payload_degrades_to_no_extras() {
        let broken = "real text\n\n— ⚙ Aperio · please don't edit —\naperio:1:!!!not-base64!!!";
        let (clean, extras) = extract(Some(broken));
        // Can't decode → leave the whole thing as the description, no data loss.
        assert!(extras.is_none());
        assert_eq!(clean.as_deref(), Some(broken));
    }

    #[test]
    fn block_only_when_description_is_empty() {
        let stored = embed(None, &sample()).unwrap();
        assert!(stored.starts_with(WARNING_LINE));
        let (clean, back) = extract(Some(&stored));
        assert_eq!(clean, None);
        assert_eq!(back, Some(sample()));
    }
}
