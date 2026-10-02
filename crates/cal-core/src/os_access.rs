//! Whether the operating system lets Aperio at the device's own calendars and
//! reminders, and when Aperio asks for that by itself.
//!
//! The device's calendars are not an account Aperio signs in to: the OS
//! grants or withholds them, per kind of entry, and only the OS can ask the
//! user. Aperio asked exactly once, when the "this device" account was added.
//! Moving to a new phone brings the account and its cache along but not the
//! grant, and nothing ever asked again: the account failed on every read, its
//! cache froze, and nothing said why.
//!
//! So the rule lives here, once, for every frontend (decision 166): when a
//! device account exists and the OS has never asked about an entity, Aperio
//! asks at start, for exactly the entities the OS has never asked about. Every
//! other state is the user's answer or a restriction, and asking again cannot
//! change it — those get a way into the system settings instead, never a
//! prompt by surprise.

use serde::{Deserialize, Serialize};

/// One entity's access, as the operating system states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum OsAccess {
    /// Read and write.
    Full,
    /// New entries may be added, nothing may be read (iOS "Add events
    /// only"). Counts as no access (decision 171): Aperio has to read.
    WriteOnly,
    /// The OS has never asked the user on this device.
    NotAsked,
    /// The user said no.
    Denied,
    /// A policy (Screen Time, a device profile) forbids it; the user cannot
    /// grant it from here.
    Restricted,
    /// The platform cannot tell "never asked" from "refused" (Android), or
    /// sent a state this build does not know.
    Undetermined,
}

/// Which entities to ask the OS about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct AskFor {
    pub events: bool,
    pub reminders: bool,
}

/// What a frontend reads at start: the access per entity, the device accounts
/// it concerns, and whether to ask now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct OsAccessReport {
    /// The names of the device accounts, for what is said afterwards.
    pub account_names: Vec<String>,
    /// Access to the calendars.
    pub calendar: OsAccess,
    /// Access to the reminders; `None` where the platform has none.
    pub tasks: Option<OsAccess>,
    /// `Some` exactly when Aperio should ask now ([`ask_on_start`]).
    pub ask_now: Option<AskFor>,
}

/// Whether to ask the OS at start, and about what.
///
/// Only with a device account, and only about an entity the OS has never
/// asked about. Each entity is decided on its own: a grant for the calendars
/// does not stand in for the reminders, and the prompt for one the user
/// already answered is not repeated.
pub fn ask_on_start(
    calendar: OsAccess,
    tasks: Option<OsAccess>,
    has_account: bool,
) -> Option<AskFor> {
    if !has_account {
        return None;
    }
    let ask = AskFor {
        events: calendar == OsAccess::NotAsked,
        reminders: tasks == Some(OsAccess::NotAsked),
    };
    (ask.events || ask.reminders).then_some(ask)
}

#[cfg(test)]
mod tests {
    use super::*;
    use OsAccess::*;

    const BOTH: AskFor = AskFor {
        events: true,
        reminders: true,
    };
    const EVENTS: AskFor = AskFor {
        events: true,
        reminders: false,
    };
    const REMINDERS: AskFor = AskFor {
        events: false,
        reminders: true,
    };

    #[test]
    fn asks_about_exactly_what_was_never_asked() {
        assert_eq!(ask_on_start(NotAsked, Some(NotAsked), true), Some(BOTH));
        assert_eq!(ask_on_start(Full, Some(NotAsked), true), Some(REMINDERS));
        assert_eq!(ask_on_start(NotAsked, Some(Full), true), Some(EVENTS));
        // A platform without reminders (Android).
        assert_eq!(ask_on_start(NotAsked, None, true), Some(EVENTS));
    }

    #[test]
    fn never_asks_about_an_answer_or_a_restriction() {
        for answered in [Full, WriteOnly, Denied, Restricted, Undetermined] {
            assert_eq!(
                ask_on_start(answered, Some(answered), true),
                None,
                "{answered:?}"
            );
            assert_eq!(ask_on_start(answered, None, true), None, "{answered:?}");
        }
    }

    #[test]
    fn never_asks_without_a_device_account() {
        for state in [NotAsked, Full, WriteOnly, Denied, Restricted, Undetermined] {
            assert_eq!(
                ask_on_start(state, Some(NotAsked), false),
                None,
                "{state:?}"
            );
            assert_eq!(
                ask_on_start(NotAsked, Some(state), false),
                None,
                "{state:?}"
            );
        }
    }

    #[test]
    fn speaks_snake_case_on_the_wire() {
        let report = OsAccessReport {
            account_names: vec!["Dieses Gerät".into()],
            calendar: NotAsked,
            tasks: Some(WriteOnly),
            ask_now: Some(EVENTS),
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains(r#""calendar":"not_asked""#), "{json}");
        assert!(json.contains(r#""tasks":"write_only""#), "{json}");
        assert_eq!(
            serde_json::from_str::<OsAccessReport>(&json).unwrap(),
            report
        );
    }
}
