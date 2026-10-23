//! Free/busy, the rule both shells follow.
//!
//! The editors check whether the attendees are free and say "All attendees
//! are free" for an empty answer. So an answer that is not one must not be
//! empty where the user could act on it: a busy provider says so (decision
//! 270), and the check can be tried again later.

use cal_core::{CalendarFeature, DateRange, FreeBusy};

/// Free/busy for `emails` from the calendar's `adapter`.
///
/// - No adapter (a local calendar): nobody's schedule is known, so the
///   answer is empty, as before.
/// - A busy provider: the error, so the editor says the check could not be
///   made instead of "All attendees are free".
/// - Any other failure: logged, and an empty answer, as before (TODO.md: the
///   editors should say every failed check).
pub async fn query_free_busy(
    adapter: Option<&dyn CalendarFeature>,
    emails: &[&str],
    range: DateRange,
) -> cal_core::Result<Vec<FreeBusy>> {
    let Some(adapter) = adapter else {
        return Ok(Vec::new());
    };
    match adapter.get_free_busy(emails, range).await {
        Ok(answer) => Ok(answer),
        Err(err) if err.is_provider_busy() => Err(err),
        Err(err) => {
            tracing::warn!(target: "aperio::free_busy", ?err, "free/busy query failed; returning empty");
            Ok(Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use cal_core::{Adapter, AuthToken, Calendar, Capability, Credentials, Event, NewEvent};
    use chrono::{TimeZone, Utc};

    /// A calendar adapter whose free/busy answers `answer`.
    struct Answers(fn() -> cal_core::Result<Vec<FreeBusy>>);

    #[async_trait]
    impl Adapter for Answers {
        async fn authenticate(&self, _: Credentials) -> cal_core::Result<AuthToken> {
            unreachable!()
        }
        fn capabilities(&self) -> &[Capability] {
            &[]
        }
    }

    #[async_trait]
    impl CalendarFeature for Answers {
        async fn list_calendars(&self) -> cal_core::Result<Vec<Calendar>> {
            unreachable!()
        }
        async fn get_events(&self, _: &str, _: DateRange) -> cal_core::Result<Vec<Event>> {
            unreachable!()
        }
        async fn create_event(&self, _: &str, _: NewEvent) -> cal_core::Result<Event> {
            unreachable!()
        }
        async fn update_event(&self, _: Event) -> cal_core::Result<Event> {
            unreachable!()
        }
        async fn delete_event(&self, _: &str, _: bool) -> cal_core::Result<()> {
            unreachable!()
        }
        async fn get_free_busy(&self, _: &[&str], _: DateRange) -> cal_core::Result<Vec<FreeBusy>> {
            (self.0)()
        }
        fn calendar_color(&self, _: &str) -> Option<cal_core::ContainerColor> {
            None
        }
    }

    fn range() -> DateRange {
        DateRange::new(
            Utc.with_ymd_and_hms(2026, 11, 2, 9, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 11, 2, 10, 0, 0).unwrap(),
        )
    }

    async fn ask(
        answer: fn() -> cal_core::Result<Vec<FreeBusy>>,
    ) -> cal_core::Result<Vec<FreeBusy>> {
        let adapter = Answers(answer);
        query_free_busy(Some(&adapter), &["a@example.com"], range()).await
    }

    #[tokio::test]
    async fn an_answer_passes_through() {
        let answer = ask(|| {
            Ok(vec![FreeBusy {
                email: "a@example.com".into(),
                slots: Vec::new(),
            }])
        })
        .await
        .unwrap();
        assert_eq!(answer.len(), 1);
    }

    /// A busy provider is said, never read as nobody being busy (270).
    #[tokio::test]
    async fn a_busy_provider_is_said() {
        let err = ask(|| {
            Err(cal_core::Error::provider_busy(
                cal_core::BusyDetail::TooManyRequests,
            ))
        })
        .await
        .unwrap_err();
        assert!(err.is_provider_busy(), "{err:?}");
    }

    #[tokio::test]
    async fn another_failure_stays_an_empty_answer() {
        let answer = ask(|| Err(cal_core::Error::Network("down".into())))
            .await
            .unwrap();
        assert!(answer.is_empty());
    }

    #[tokio::test]
    async fn a_calendar_without_an_adapter_answers_empty() {
        let answer = query_free_busy(None, &["a@example.com"], range())
            .await
            .unwrap();
        assert!(answer.is_empty());
    }
}
