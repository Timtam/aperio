//! The rows of ONE series, read by its id, and how far they can be trusted.
//!
//! Splitting a series ("this and all following") has to know every occurrence
//! the provider keeps as a row of its own, `{series}::rid::{slot}`: a changed
//! one stands in for its slot, and a cancelled one is an occurrence the user
//! deleted, which the new series must not bring back (decision 134). A date
//! range cannot find them all. A row counts by the slot it names, its own
//! times may lie anywhere, and the series may run for years. So the read goes
//! by id, on the table's primary key, and never warms, heals or filters
//! (decision 135): it is a look at what the cache holds, nothing more.
//!
//! What the cache holds is not always the whole calendar. Exchange and CalDAV
//! keep every row; Google keeps the rows of a window around today. So the read
//! says how far its rows reach (decision 139), and the frontend can ask before
//! it trusts rows that may be missing. What a window vouches for is where a row
//! stands NOW, not the slot it names: see [`SeriesReach::Window`].

use serde::Serialize;

use cal_core::{Event, OVERRIDE_ID_MARKER};

use super::{unbounded_window, CacheStore, SyncScope};
use crate::birthdays::is_birthday_calendar_id;
use crate::registry::{AdapterRegistry, LOCAL_ID};

/// How far a calendar's cached rows reach: where a series' rows can be missing
/// because the cache never held them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SeriesReach {
    /// Every row the calendar has is here: the provider hands over the whole
    /// calendar (Exchange, CalDAV), or the calendar keeps no such rows at all
    /// (local and birthday calendars).
    Complete,
    /// Only the rows whose event now lies inside this stretch are sure to be
    /// here. A provider that fills its cache for a window (Google) filters
    /// its rows by the time they are shown, not by the slot they name, on the
    /// full read and on every delta. So a cancelled row, which stands at its
    /// slot, is missing only when that slot lies outside; a changed one moved
    /// out of the stretch is missing whatever its slot.
    Window {
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
    },
    /// Nothing is sure: the calendar was never read, or was written to since
    /// it was last read in full. Every write clears the window until the next
    /// refresh, because the write may have changed rows the cache still shows
    /// as they were.
    Unknown,
}

/// The rows of one series besides its master, and how far they reach.
///
/// Only serialized: the frontends declare it themselves (`shared/`), generic
/// over their own event type, because `Event` has no generated declaration.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeriesRows {
    /// Changed and cancelled occurrences alike, by the time they are shown.
    pub rows: Vec<Event>,
    pub reach: SeriesReach,
}

impl SeriesRows {
    /// A calendar that keeps no occurrence rows: nothing, and nothing missing.
    fn none() -> Self {
        Self {
            rows: Vec::new(),
            reach: SeriesReach::Complete,
        }
    }
}

/// The reach a recorded window stands for. The folder-complete window is the
/// cache's own sentinel, so it is recognised by value.
pub(super) fn reach_of(
    window: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
) -> SeriesReach {
    match window {
        None => SeriesReach::Unknown,
        Some((start, end)) => {
            let unbounded = unbounded_window();
            if start <= unbounded.start && end >= unbounded.end {
                SeriesReach::Complete
            } else {
                SeriesReach::Window { start, end }
            }
        }
    }
}

/// The first id past every id that starts with `prefix`, in the database's
/// binary order: the prefix with its last character raised by one. Rows whose
/// id starts with the prefix are exactly those in `[prefix, prefix_end)`.
pub(super) fn prefix_end(prefix: &str) -> String {
    let mut end = prefix.to_string();
    let last = end.pop().expect("the override marker is never empty");
    // The marker ends the prefix, and it is ASCII, so the next character is too.
    end.push(char::from_u32(u32::from(last) + 1).expect("an ASCII character has a successor"));
    end
}

/// The rows of `series_id` in `calendar_id`, and how far they reach.
///
/// A local calendar keeps no occurrence rows, nor does a birthday calendar, so
/// both answer with none and nothing missing. An external calendar without an
/// adapter is an error, as it is for `get_events`: the frontend must not plan a
/// split on rows it could not read.
pub fn series_rows(
    registry: &AdapterRegistry,
    cache: &CacheStore,
    calendar_id: &str,
    series_id: &str,
) -> cal_core::Result<SeriesRows> {
    if series_id.is_empty() {
        return Err(cal_core::Error::invalid_input(
            "a series read needs a series id",
        ));
    }
    // An occurrence's id would read as a series whose id holds the marker, and
    // find nothing: a split planned on that would miss every row.
    if series_id.contains(OVERRIDE_ID_MARKER) {
        return Err(cal_core::Error::invalid_input(format!(
            "'{series_id}' names an occurrence, not a series"
        )));
    }
    if is_birthday_calendar_id(calendar_id) {
        return Ok(SeriesRows::none());
    }
    let account = registry
        .account_for_calendar(calendar_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        return Ok(SeriesRows::none());
    }
    if registry.calendar_adapter(&account).is_none() {
        return Err(cal_core::Error::NotFound(format!(
            "calendar '{calendar_id}' is not routable"
        )));
    }
    cache
        .read_series_rows(&account, calendar_id, series_id)
        .map_err(|err| cal_core::Error::internal(err.to_string()))
}

impl CacheStore {
    /// The cached rows of one series besides its master, whatever their dates
    /// and cancelled ones included, with the reach of the calendar's cache.
    /// Both are read in one transaction: a refresh that lands between them
    /// would pair its rows with the window before it, or the other way round.
    pub fn read_series_rows(
        &self,
        account: &str,
        calendar: &str,
        series_id: &str,
    ) -> crate::db::DbResult<SeriesRows> {
        let from = format!("{series_id}{OVERRIDE_ID_MARKER}");
        let until = prefix_end(&from);
        self.db.with_read_conn(|c| {
            let tx = c.unchecked_transaction()?;
            let window = read_event_window(&tx, account, calendar)?;
            let rows = {
                let mut stmt = tx.prepare(
                    "SELECT payload FROM cache_events
                     WHERE account_id = ?1 AND calendar_id = ?2
                       AND id >= ?3 AND id < ?4
                     ORDER BY start_utc",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![account, calendar, from, until], |r| {
                        r.get::<_, String>(0)
                    })?;
                super::rows_to_structs(rows, "cache_events")?
            };
            Ok(SeriesRows {
                rows,
                reach: reach_of(window),
            })
        })
    }
}

/// The event window recorded for `calendar`, read on the caller's connection
/// so it shares the caller's transaction.
fn read_event_window(
    c: &rusqlite::Connection,
    account: &str,
    calendar: &str,
) -> crate::db::DbResult<Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>> {
    use rusqlite::OptionalExtension;
    let window = c
        .query_row(
            "SELECT window_start, window_end FROM cache_sync_state
              WHERE account_id = ?1 AND scope = ?2 AND container_id = ?3",
            rusqlite::params![account, SyncScope::Events.as_str(), calendar],
            |r| {
                Ok((
                    super::opt_ts(r.get::<_, Option<String>>(0)?)?,
                    super::opt_ts(r.get::<_, Option<String>>(1)?)?,
                ))
            },
        )
        .optional()?;
    Ok(match window {
        Some((Some(start), Some(end))) => Some((start, end)),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::{TimeZone, Utc};
    use rusqlite::params;

    use super::*;
    use crate::db::DbHandle;
    use cal_core::DateRange;

    const ACC: &str = "acc-1";
    const CAL: &str = "cal-1";

    fn setup() -> CacheStore {
        let db = DbHandle::open_in_memory().unwrap();
        let store = CacheStore::new(db);
        store
            .db
            .with_conn(|c| {
                c.execute(
                    "INSERT INTO accounts (id, adapter_kind, display_name, config_json, created_at, updated_at)
                     VALUES (?1, 'google', 'Work', '{}', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                    params![ACC],
                )
            })
            .unwrap();
        store
    }

    fn row(id: &str, year: i32, cancelled: bool) -> Event {
        let start = Utc.with_ymd_and_hms(year, 6, 1, 9, 0, 0).unwrap();
        Event {
            keep_attendees: false,
            keep_fields: Vec::new(),
            clear_attendees: false,
            organized_elsewhere: false,
            id: id.into(),
            calendar_id: CAL.into(),
            title: id.into(),
            description: None,
            location: None,
            start,
            end: if cancelled {
                start
            } else {
                start + chrono::Duration::hours(1)
            },
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            truncate_tail_overrides: false,
            created_at: start,
            updated_at: start,
            etag: None,
            organizer: None,
            attendee_responses: Vec::new(),
            cancelled,
            scheduling_silenced: false,
        }
    }

    fn window(from: i32, to: i32) -> DateRange {
        DateRange::new(
            Utc.with_ymd_and_hms(from, 1, 1, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(to, 1, 1, 0, 0, 0).unwrap(),
        )
    }

    fn ids(rows: &SeriesRows) -> Vec<&str> {
        rows.rows.iter().map(|r| r.id.as_str()).collect()
    }

    #[test]
    fn every_row_of_the_series_whatever_its_date() {
        let store = setup();
        let cached = [
            row("ev-1", 2026, false),
            row("ev-1::rid::2026-06-01T09:00:00Z", 2026, false),
            // Deleted, and years past any window a range read would ask for.
            row("ev-1::rid::2031-06-01T09:00:00Z", 2031, true),
            // A row whose id only STARTS like the series': another series.
            row("ev-10::rid::2026-06-01T09:00:00Z", 2026, true),
            row("ev-1x", 2026, false),
            row("ev-2::rid::2026-06-01T09:00:00Z", 2026, true),
        ];
        store
            .replace_calendar_events(ACC, CAL, window(2026, 2027), &cached)
            .unwrap();

        let got = store.read_series_rows(ACC, CAL, "ev-1").unwrap();
        assert_eq!(
            ids(&got),
            vec![
                "ev-1::rid::2026-06-01T09:00:00Z",
                "ev-1::rid::2031-06-01T09:00:00Z",
            ],
            "the series' rows, in the order they are shown, and nothing else",
        );
        assert!(
            got.rows[1].cancelled,
            "a cancelled row is read like any other"
        );
    }

    #[test]
    fn another_calendar_keeps_its_rows() {
        let store = setup();
        store
            .replace_calendar_events(
                ACC,
                "cal-2",
                window(2026, 2027),
                &[row("ev-1::rid::2026-06-01T09:00:00Z", 2026, false)],
            )
            .unwrap();
        let got = store.read_series_rows(ACC, CAL, "ev-1").unwrap();
        assert!(got.rows.is_empty());
    }

    #[test]
    fn the_reach_is_the_window_the_cache_was_filled_for() {
        let store = setup();
        store
            .replace_calendar_events(ACC, CAL, window(2026, 2027), &[])
            .unwrap();
        assert_eq!(
            store.read_series_rows(ACC, CAL, "ev-1").unwrap().reach,
            SeriesReach::Window {
                start: window(2026, 2027).start,
                end: window(2026, 2027).end,
            },
        );
    }

    #[test]
    fn a_calendar_held_whole_reaches_everywhere() {
        let store = setup();
        store
            .replace_calendar_events(ACC, CAL, unbounded_window(), &[])
            .unwrap();
        assert_eq!(
            store.read_series_rows(ACC, CAL, "ev-1").unwrap().reach,
            SeriesReach::Complete,
        );
    }

    #[test]
    fn a_calendar_never_read_or_written_since_reaches_nowhere() {
        let store = setup();
        assert_eq!(
            store.read_series_rows(ACC, CAL, "ev-1").unwrap().reach,
            SeriesReach::Unknown,
            "never read",
        );
        store
            .replace_calendar_events(
                ACC,
                CAL,
                unbounded_window(),
                &[row("ev-1::rid::2026-06-01T09:00:00Z", 2026, true)],
            )
            .unwrap();
        store.invalidate(ACC, SyncScope::Events, CAL).unwrap();
        let got = store.read_series_rows(ACC, CAL, "ev-1").unwrap();
        assert_eq!(got.reach, SeriesReach::Unknown, "written to since");
        assert_eq!(
            got.rows.len(),
            1,
            "the rows it still holds are read all the same"
        );
    }

    #[test]
    fn the_prefix_ends_right_after_every_id_that_starts_with_it() {
        let from = format!("ev-1{OVERRIDE_ID_MARKER}");
        let until = prefix_end(&from);
        assert_eq!(until, "ev-1::rid:;");
        assert!(format!("{from}9999-12-31T23:59:59Z").as_str() < until.as_str());
        assert!(from.as_str() < until.as_str());
    }

    fn registry() -> AdapterRegistry {
        AdapterRegistry::new(
            Arc::new(plugin_core::PluginManager::new("0.1.0")),
            Arc::new(sync_engine::test_support::FakeSecrets::default()),
        )
    }

    #[test]
    fn a_local_or_birthday_calendar_keeps_no_rows_and_misses_none() {
        let registry = registry();
        let cache = setup();
        for calendar in ["local-cal", "aperio-birthdays:list-1"] {
            let got = series_rows(&registry, &cache, calendar, "ev-1").unwrap();
            assert_eq!(got, SeriesRows::none(), "{calendar}");
        }
    }

    /// An external calendar's adapter, which a series read must never call: it
    /// only looks at the cache.
    struct NeverAsked;

    #[async_trait::async_trait]
    impl cal_core::Adapter for NeverAsked {
        async fn authenticate(
            &self,
            _credentials: cal_core::Credentials,
        ) -> cal_core::Result<cal_core::AuthToken> {
            unreachable!("a series read never asks the provider")
        }
        fn capabilities(&self) -> &[cal_core::Capability] {
            &[]
        }
    }

    #[async_trait::async_trait]
    impl cal_core::CalendarFeature for NeverAsked {
        async fn list_calendars(&self) -> cal_core::Result<Vec<cal_core::Calendar>> {
            unreachable!("a series read never asks the provider")
        }
        async fn get_events(
            &self,
            _calendar: &str,
            _range: DateRange,
        ) -> cal_core::Result<Vec<Event>> {
            unreachable!("a series read never asks the provider")
        }
        async fn create_event(
            &self,
            _calendar: &str,
            _event: cal_core::NewEvent,
        ) -> cal_core::Result<Event> {
            unreachable!("a series read never asks the provider")
        }
        async fn update_event(&self, _event: Event) -> cal_core::Result<Event> {
            unreachable!("a series read never asks the provider")
        }
        async fn delete_event(&self, _id: &str, _notify: bool) -> cal_core::Result<()> {
            unreachable!("a series read never asks the provider")
        }
        async fn get_free_busy(
            &self,
            _emails: &[&str],
            _range: DateRange,
        ) -> cal_core::Result<Vec<cal_core::FreeBusy>> {
            unreachable!("a series read never asks the provider")
        }
        fn calendar_color(&self, _calendar: &str) -> Option<cal_core::ContainerColor> {
            None
        }
    }

    #[test]
    fn an_external_calendar_is_read_from_its_accounts_cache() {
        let registry = registry();
        registry.register_host_adapter(ACC, Some(Arc::new(NeverAsked)), None);
        registry.note_calendar_route(CAL, ACC);
        let cache = setup();
        cache
            .replace_calendar_events(
                ACC,
                CAL,
                window(2026, 2027),
                &[
                    row("ev-1::rid::2031-06-01T09:00:00Z", 2031, true),
                    row("ev-2::rid::2026-06-01T09:00:00Z", 2026, false),
                ],
            )
            .unwrap();
        let got = series_rows(&registry, &cache, CAL, "ev-1").unwrap();
        assert_eq!(ids(&got), vec!["ev-1::rid::2031-06-01T09:00:00Z"]);
        assert_eq!(
            got.reach,
            SeriesReach::Window {
                start: window(2026, 2027).start,
                end: window(2026, 2027).end,
            },
        );
    }

    #[test]
    fn a_calendar_with_no_adapter_is_not_read_as_empty() {
        let registry = registry();
        registry.note_calendar_route(CAL, ACC);
        let err = series_rows(&registry, &setup(), CAL, "ev-1").unwrap_err();
        assert!(matches!(err, cal_core::Error::NotFound(_)), "{err:?}");
    }

    #[test]
    fn an_occurrence_is_not_a_series() {
        let registry = registry();
        let cache = setup();
        for id in ["", "ev-1::rid::2026-06-01T09:00:00Z"] {
            let err = series_rows(&registry, &cache, "local-cal", id).unwrap_err();
            assert!(
                matches!(err, cal_core::Error::InvalidInput(_)),
                "{id}: {err:?}"
            );
        }
    }
}
