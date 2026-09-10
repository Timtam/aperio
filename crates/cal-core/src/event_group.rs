//! Which events mean the same appointment.
//!
//! A group is Aperio's statement ABOUT foreign data: these events, in
//! different calendars and belonging to different providers, are one
//! commitment. No provider knows the concept; see `DESIGN-event-groups.md`.
//!
//! The types live here rather than in the host because they cross crates: the
//! host writes them, the sync applier reads them off the wire, and the local
//! adapter stores what arrives. One shape in one place, the same arrangement
//! [`crate::ColorLabel`] already has.

use serde::{Deserialize, Serialize};

/// Two titles that a person would call the same one.
///
/// Case, padding and the WIDTH of the gaps between words are all noise: a title
/// is typed by people and re-flowed by providers, and "Wochen  planung" with two
/// spaces is not a different appointment from "Wochen planung" with one.
///
/// # Why this is here and not five times over
///
/// It was written five times: three times in `shared/` — byte for byte
/// identical, in `groupSuggestions.ts`, `suggestGroupMate.ts` and
/// `healEventGroups.ts` — and twice in `host-core`, in
/// `event_anchor::plan_repairs` and in the reminder scheduler's own anchor
/// repair. The two languages spelled it differently: TypeScript collapsed inner
/// runs of whitespace, Rust only trimmed the ends.
///
/// Each copy was self-consistent — every one compares two titles that both went
/// through it — so no single comparison ever got two answers. What the
/// difference bought was a CHAIN across the two: TypeScript offers to group two
/// events because it reads their titles as the same, and if the title's inner
/// spacing later changes, Rust's repair no longer re-finds the member and it
/// drops out of the group in silence. Grouped by one reading of "same",
/// lost by another.
///
/// The generous reading wins, and deliberately: it errs towards keeping a member
/// in a group rather than towards losing one without a word.
///
/// # The two halves are spelled out, because the built-ins disagree
///
/// Neither "whitespace" nor "lowercase" is one answer across the two languages,
/// and taking whatever each one hands out is how the copies would drift again:
///
/// - A GAP is Unicode `White_Space`, which is what [`char::is_whitespace`]
///   means. JavaScript's `\s` is a different set — it counts U+FEFF and not
///   U+0085 — so the TypeScript half states the set rather than using `\s`.
/// - LOWERCASE is applied to the whole collapsed string, not character by
///   character, because a final Greek sigma depends on what follows it:
///   `"ΤΕΛΟΣ".to_lowercase()` is `"τελος"` with ς, while lowercasing each
///   `char` on its own gives σ. `String.prototype.toLowerCase` makes the same
///   distinction, so whole-string on both sides is what agrees.
pub fn normalized_title(title: &str) -> String {
    let mut collapsed = String::with_capacity(title.len());
    let mut in_gap = false;
    for ch in title.trim().chars() {
        if ch.is_whitespace() {
            in_gap = true;
            continue;
        }
        if in_gap {
            collapsed.push(' ');
            in_gap = false;
        }
        collapsed.push(ch);
    }
    collapsed.to_lowercase()
}

/// One event's membership in a group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct EventGroupMember {
    pub calendar_id: String,
    /// Series master id — a recurring appointment is grouped as a series.
    pub event_id: String,
    /// The title it had when it joined. Half of the SIGNATURE, and never
    /// shown: what a user reads comes from the event itself, which may since
    /// have been renamed.
    pub title: String,
    /// The start it had when it joined. The other half.
    pub starts_at: String,
    pub added_at: String,
}

/// A set of events that mean one appointment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct EventGroup {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    /// The whole membership, always. A group is small and only meaningful
    /// entire, so it travels and is stored as one value rather than as a
    /// stream of additions and removals that could interleave.
    pub members: Vec<EventGroupMember>,
}

/// The title rule, as a table both languages answer.
///
/// Its TypeScript half is `src/intl/eventTitle.test.ts`, reading this same
/// file. The two are independent implementations of one decision — which is
/// exactly the arrangement that let five copies drift — so the fixture is what
/// keeps them together until the TypeScript one goes.
#[cfg(test)]
mod contract {
    use super::*;
    use serde_json::Value;

    const CONTRACT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/normalizedTitle.json"
    ));

    #[test]
    fn every_case_in_the_contract_holds() {
        let doc: Value = serde_json::from_str(CONTRACT).expect("the contract parses");
        let cases = doc["cases"].as_array().expect("cases is an array");

        // Anti-silence: the file must carry the row the whole rule turns on.
        // Named rather than counted — a floor would be the number of rows there
        // are today, and adding one is the change it should survive.
        assert!(
            cases.iter().any(|c| c["title"] == "Wochen  planung"),
            "the contract lost the doubled-inner-space row, which is the one \
             case the two spellings disagreed on",
        );

        for case in cases {
            let title = case["title"].as_str().expect("every case has a title");
            let want = case["normalized"]
                .as_str()
                .expect("every case has an answer");
            assert_eq!(
                normalized_title(title),
                want,
                "{:?}: {}",
                title,
                case["note"].as_str().unwrap_or(""),
            );
        }
    }

    #[test]
    fn it_is_idempotent() {
        // Normalising an already-normalised title must not change it again.
        // Both anchor-repair sites compare a STORED signature against a live
        // title, and a rule that moved on a second pass would make that
        // comparison depend on how many times each side had been through it.
        let doc: Value = serde_json::from_str(CONTRACT).expect("the contract parses");
        for case in doc["cases"].as_array().expect("cases is an array") {
            let once = normalized_title(case["title"].as_str().unwrap_or(""));
            assert_eq!(normalized_title(&once), once, "not idempotent: {once:?}");
        }
    }
}
