//! What a write left behind that it could not take back, in a word both
//! surfaces can translate.
//!
//! The opposite of a [`crate::WriteRefusal`]: a refusal says nothing was
//! written, a leftover says something was and may still stand. It travels the
//! same way, as the token at the start of an error's message:
//!
//! ```text
//! created-may-stand: invited no-occurrence
//! ```
//!
//! but always as [`crate::Error::Network`], the one error that says "this may
//! have landed" and keeps its message through the plugin boundary, the
//! desktop's command error and the phone's native module. Never as a refusing
//! error: a surface that reads a write as never landed undoes the rest of it —
//! a split deletes its new part —, and that is wrong where the new part stands.

use serde::{Deserialize, Serialize};

/// What a failed write may have left behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum WriteLeftover {
    /// A new series was written and could not be completed — its deleted
    /// occurrences could not all be deleted again, or the answer to its
    /// creation was lost — nor deleted again, so it may stand in the calendar
    /// beside the old one (decisions 203, 210). The detail is
    /// [`crate::step_write::step_detail`], as for
    /// [`crate::WriteRefusal::DeletionsNotKept`].
    CreatedMayStand,
    /// A save of several requests was turned away by a busy provider after an
    /// earlier request of it had landed (decision 277): part of the change
    /// stands, the rest does not. It counts as landed — nothing is taken back
    /// or sent again —, and the surfaces say to check it and try the rest
    /// later. The detail is a machine word for the log: the step turned away.
    PartlySaved,
}

impl WriteLeftover {
    /// The token that starts the message.
    pub const fn token(self) -> &'static str {
        match self {
            Self::CreatedMayStand => "created-may-stand",
            Self::PartlySaved => "partly-saved",
        }
    }

    /// The message: the token, and the detail behind a colon when there is one.
    pub fn message(self, detail: &str) -> String {
        let detail = detail.trim();
        if detail.is_empty() {
            self.token().to_string()
        } else {
            format!("{}: {detail}", self.token())
        }
    }

    /// The leftover a message names, and its detail, read from the start of
    /// the message as [`crate::WriteRefusal::parse`] reads a refusal.
    pub fn parse(message: &str) -> Option<(Self, &str)> {
        let message = message.trim();
        for leftover in [Self::CreatedMayStand, Self::PartlySaved] {
            let Some(rest) = message.strip_prefix(leftover.token()) else {
                continue;
            };
            return match rest.strip_prefix(':') {
                Some(detail) => Some((leftover, detail.trim())),
                None if rest.is_empty() => Some((leftover, "")),
                None => continue,
            };
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::step_write::{step_detail_invited, step_detail_why};
    use crate::WriteRefusal;

    #[test]
    fn the_token_is_the_serialized_name_and_comes_back() {
        assert_eq!(
            serde_json::to_value(WriteLeftover::CreatedMayStand).unwrap(),
            serde_json::json!(WriteLeftover::CreatedMayStand.token()),
        );
        let msg = WriteLeftover::CreatedMayStand.message("invited time");
        assert_eq!(msg, "created-may-stand: invited time");
        assert_eq!(
            WriteLeftover::parse(&msg),
            Some((WriteLeftover::CreatedMayStand, "invited time"))
        );
        assert_eq!(
            WriteLeftover::CreatedMayStand.message(""),
            "created-may-stand"
        );
        assert_eq!(
            WriteLeftover::parse("created-may-stand"),
            Some((WriteLeftover::CreatedMayStand, ""))
        );
    }

    #[test]
    fn a_save_partly_made_comes_back_from_its_message() {
        let msg = WriteLeftover::PartlySaved.message("members");
        assert_eq!(msg, "partly-saved: members");
        assert_eq!(
            WriteLeftover::parse(&msg),
            Some((WriteLeftover::PartlySaved, "members"))
        );
        assert_eq!(WriteRefusal::parse(&msg), None);
        assert_eq!(
            serde_json::to_value(WriteLeftover::PartlySaved).unwrap(),
            serde_json::json!("partly-saved")
        );
    }

    #[test]
    fn a_leftover_is_never_a_refusal_nor_a_refusal_a_leftover() {
        assert_eq!(WriteRefusal::parse("created-may-stand: time"), None);
        assert_eq!(WriteLeftover::parse("deletions-not-kept: time"), None);
        assert_eq!(WriteLeftover::parse("created-may-stand-elsewhere: x"), None);
    }

    /// The rows both surfaces read the same messages by (`writeOutcome.json`;
    /// `src/state/eventWriteError.test.ts` reads it too).
    #[test]
    fn the_shared_rows_read_as_they_say() {
        let rows: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/writeOutcome.json"
        )))
        .unwrap();
        let rows = rows["rows"].as_array().unwrap();
        assert!(rows.len() >= 6, "the fixture lost its rows");
        for row in rows {
            let message = row["message"].as_str().unwrap();
            let refusal = WriteRefusal::parse(message);
            let leftover = WriteLeftover::parse(message);
            assert_eq!(
                refusal.map(|(r, _)| serde_json::to_value(r).unwrap()),
                (!row["refusal"].is_null()).then(|| row["refusal"].clone()),
                "{message}"
            );
            assert_eq!(
                leftover.map(|(l, _)| serde_json::to_value(l).unwrap()),
                (!row["leftover"].is_null()).then(|| row["leftover"].clone()),
                "{message}"
            );
            let detail = refusal.map(|(_, d)| d).or(leftover.map(|(_, d)| d));
            assert_eq!(
                detail.is_some_and(step_detail_invited),
                row["invited"].as_bool().unwrap(),
                "{message}"
            );
            assert_eq!(
                detail.map(step_detail_why),
                row["why"].as_str(),
                "{message}"
            );
        }
    }
}
