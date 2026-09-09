//! The rules themselves, in ordinary Rust.
//!
//! Nothing here knows that JavaScript exists. That is the point: these are
//! testable on the host, where the `#[wasm_bindgen]` shell in `lib.rs` is not
//! — `JsValue` compiles off the wasm target and then ABORTS when called, so a
//! host test touching an error path kills the whole test binary rather than
//! failing.
//!
//! What lives here is translation and nothing else. The decisions themselves
//! belong to `cal-core`; this module reads the wire spelling of an enum, asks
//! `cal-core`, and hands the answer back in the same spelling.

use std::fmt;

use core::cmp::Ordering;

use cal_core::{compare_names, compare_titles, CollationLanguage, TaskPriority};

/// A value that crossed the boundary and is not one this app writes.
///
/// An error rather than a default, on purpose. Data arriving from JavaScript
/// is unvalidated by construction, and a silent fallback would answer
/// confidently for a value nobody wrote — the kind of wrongness that shows up
/// days later as "why is this task sorted there".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// Not `"low"`, `"medium"` or `"high"`.
    UnknownPriority(String),
    /// Not `"three"` or `"two"`.
    UnknownScale(String),
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WireError::UnknownPriority(value) => write!(
                f,
                "unknown priority {value:?} — expected \"low\", \"medium\" or \"high\""
            ),
            WireError::UnknownScale(value) => write!(
                f,
                "unknown priority scale {value:?} — expected \"three\" or \"two\""
            ),
        }
    }
}

impl std::error::Error for WireError {}

/// Parse the wire spelling of a priority — the same lowercase form
/// `#[serde(rename_all = "lowercase")]` produces.
fn priority_from_wire(value: &str) -> Result<TaskPriority, WireError> {
    match value {
        "low" => Ok(TaskPriority::Low),
        "medium" => Ok(TaskPriority::Medium),
        "high" => Ok(TaskPriority::High),
        other => Err(WireError::UnknownPriority(other.to_string())),
    }
}

/// The wire spelling of a priority.
fn priority_to_wire(priority: TaskPriority) -> &'static str {
    match priority {
        TaskPriority::Low => "low",
        TaskPriority::Medium => "medium",
        TaskPriority::High => "high",
    }
}

/// See [`crate::is_important_priority`].
pub fn is_important_priority(priority: &str) -> Result<bool, WireError> {
    Ok(priority_from_wire(priority)? == TaskPriority::High)
}

/// See [`crate::priority_rank`].
pub fn priority_rank(priority: &str, scale: &str) -> Result<u32, WireError> {
    let priority = priority_from_wire(priority)?;
    match scale {
        "two" => Ok(u32::from(priority != TaskPriority::High)),
        "three" => Ok(match priority {
            TaskPriority::High => 0,
            TaskPriority::Medium => 1,
            TaskPriority::Low => 2,
        }),
        other => Err(WireError::UnknownScale(other.to_string())),
    }
}

/// See [`crate::normal_priority`].
pub fn normal_priority(previous: Option<&str>) -> Result<String, WireError> {
    let kept = match previous {
        None | Some("") => None,
        Some(value) => Some(priority_from_wire(value)?),
    };
    let result = match kept {
        Some(p) if p != TaskPriority::High => p,
        _ => TaskPriority::Medium,
    };
    Ok(priority_to_wire(result).to_string())
}

/// A comparison as JavaScript wants it: negative, zero, positive.
///
/// `Array.prototype.sort` only reads the sign, so the exact numbers do not
/// matter — but returning them rather than an `Ordering` keeps the boundary
/// free of a type JavaScript has no notion of.
fn to_js_ordering(ordering: Ordering) -> i32 {
    match ordering {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// See [`crate::compare_names`].
pub fn names(a: &str, b: &str, language_tag: &str) -> i32 {
    to_js_ordering(compare_names(
        a,
        b,
        CollationLanguage::from_tag(language_tag),
    ))
}

/// See [`crate::compare_titles`].
pub fn titles(a: &str, b: &str, language_tag: &str) -> i32 {
    to_js_ordering(compare_titles(
        a,
        b,
        CollationLanguage::from_tag(language_tag),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // These run on the HOST, and can, because nothing here touches `JsValue`.
    // What they cannot check is the JavaScript side of the boundary — that is
    // `src/wasm/coreRules.parity.test.ts`, which runs the real module and
    // compares it against the TypeScript for every input there is.

    #[test]
    fn the_rank_matches_what_typescript_answers_today() {
        assert_eq!(priority_rank("high", "three").unwrap(), 0);
        assert_eq!(priority_rank("medium", "three").unwrap(), 1);
        assert_eq!(priority_rank("low", "three").unwrap(), 2);
        // Two-level collapses everything below the top into one bucket.
        assert_eq!(priority_rank("high", "two").unwrap(), 0);
        assert_eq!(priority_rank("medium", "two").unwrap(), 1);
        assert_eq!(priority_rank("low", "two").unwrap(), 1);
    }

    #[test]
    fn clearing_important_keeps_a_low_task_low() {
        // The whole point of the rule: it must not quietly promote `low`.
        assert_eq!(normal_priority(Some("low")).unwrap(), "low");
        assert_eq!(normal_priority(Some("medium")).unwrap(), "medium");
        assert_eq!(normal_priority(Some("high")).unwrap(), "medium");
        assert_eq!(normal_priority(None).unwrap(), "medium");
        // The frontend sends an empty string where a task carried nothing.
        assert_eq!(normal_priority(Some("")).unwrap(), "medium");
    }

    #[test]
    fn importance_is_the_top_level_and_only_that() {
        assert!(is_important_priority("high").unwrap());
        assert!(!is_important_priority("medium").unwrap());
        assert!(!is_important_priority("low").unwrap());
    }

    #[test]
    fn an_unknown_value_is_refused_rather_than_guessed() {
        assert_eq!(
            priority_rank("urgent", "three"),
            Err(WireError::UnknownPriority("urgent".into()))
        );
        assert_eq!(
            priority_rank("high", "four"),
            Err(WireError::UnknownScale("four".into()))
        );
        assert!(is_important_priority("").is_err());
        assert!(normal_priority(Some("urgent")).is_err());
    }

    #[test]
    fn the_message_names_the_value_and_what_was_expected() {
        // It reaches the user's console through the shell, so it has to say
        // enough to act on.
        let message = WireError::UnknownPriority("urgent".into()).to_string();
        assert!(message.contains("urgent"), "{message}");
        assert!(message.contains("high"), "{message}");
    }

    #[test]
    fn the_collation_functions_answer_a_sign_and_read_the_tag() {
        // What this layer owns is the translation: an `Ordering` becomes the
        // negative/zero/positive `Array.prototype.sort` reads, and the language
        // tag becomes a `CollationLanguage`. The ordering rules themselves are
        // `cal_core::collation`'s and are tested there.
        assert!(names("arbeit", "Zebra", "de") < 0);
        assert_eq!(names("Arbeit", "arbeit", "de"), 0);
        assert!(titles("Kapitel 2", "Kapitel 10", "de") < 0);
        assert!(titles("Kapitel 10", "Kapitel 2", "de") > 0);
        // An unreadable tag falls back rather than failing — a comparator has
        // nowhere to report to.
        assert!(titles("Kapitel 2", "Kapitel 10", "") < 0);
    }
}
