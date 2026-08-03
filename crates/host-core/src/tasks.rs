//! Shared task orchestration that belongs to neither the Tauri command layer
//! nor the FFI host — so both can call the SAME logic (DESIGN.md §22).
//!
//! Today this is the post-completion recurrence reconciliation for EXTERNAL
//! tasks: [`record_external_recurrence_completion`]. It used to live as two
//! byte-identical `spawn_external_on_demand` copies (one in `src-tauri`, one in
//! `cal-ffi`); unifying it here is what guarantees desktop and mobile behave
//! the same when a recurring provider task is checked off.

use cal_core::{
    completion_record_for, next_recurrence_instance, recurrence_needs_extras, Task, TaskStatus,
    TasksFeature,
};

use crate::cache::CacheStore;

/// Reconcile a just-completed EXTERNAL recurring task with Aperio's recurrence
/// model (DESIGN §9.12). The provider write (`done=true`) has already run;
/// `completed` is the pre-write snapshot (status `Completed`). Two recurrence
/// flavours, mirror images of each other:
///
/// - **On-demand / backlog** (`recurrence_needs_extras` — the provider can't
///   express the rule, so the adapter cleared the provider's native repeat and
///   the original stays done): Aperio SPAWNS the next instance.
/// - **Native** (a plain scheduled rule the provider repeats itself — e.g.
///   Vikunja advancing the dates of the same task in place): the provider keeps
///   NO completion history, so Aperio creates a COMPLETION RECORD — a terminal
///   done copy of the just-completed turn — while the provider advances the
///   live task. Without this, checking off a native-recurring task only shifts
///   its date and leaves nothing under "Done".
///
/// Best-effort: a failure is logged, never surfaced — the user's task is
/// already done and the next sync reflects it. Idempotent against the cached
/// pre-write snapshot, so a re-run / a peer that already acted doesn't double up.
pub async fn record_external_recurrence_completion(
    ext: &dyn TasksFeature,
    cache: &CacheStore,
    account: &str,
    completed: &Task,
) {
    if completed.status != TaskStatus::Completed {
        return;
    }
    let Some(rec) = completed.recurrence.as_ref() else {
        return;
    };

    // The cached snapshot reflects the PRE-write state — our idempotency anchor.
    // Every guard below is a lookup in it, so an empty list from a FAILED read
    // would silently disarm all three and both branches would create a task on
    // the user's real provider account without ever checking whether this turn
    // was already handled. A duplicate there can only be cleaned up by hand;
    // not acting is recoverable (re-saving the task runs this again, and a peer
    // with a healthy cache still does it), so an unreadable anchor stops us.
    let cached = match cache.read_tasks(account, &completed.list_id) {
        Ok(tasks) => tasks,
        Err(err) => {
            tracing::warn!(
                account = %account,
                list = %completed.list_id,
                ?err,
                "couldn't read the cached pre-write snapshot; skipping recurrence \
                 reconciliation rather than risking a duplicate on the provider",
            );
            return;
        }
    };

    // Re-saving an already-completed task must not act again.
    if cached
        .iter()
        .any(|t| t.id == completed.id && t.status == TaskStatus::Completed)
    {
        return;
    }

    if recurrence_needs_extras(rec) {
        spawn_next_instance(ext, &cached, account, completed).await;
    } else {
        record_completion(ext, &cached, completed).await;
    }
}

/// On-demand / backlog: create the next instance (the original stays done).
async fn spawn_next_instance(
    ext: &dyn TasksFeature,
    cached: &[Task],
    account: &str,
    completed: &Task,
) {
    let is_open = |status| matches!(status, TaskStatus::Open | TaskStatus::InProgress);
    // A series spawns at most one open instance — if another client already
    // created the next turn (and it's synced into our cache), do nothing.
    if let Some(sid) = completed.series_id.as_deref() {
        if cached.iter().any(|t| {
            t.id != completed.id && t.series_id.as_deref() == Some(sid) && is_open(t.status)
        }) {
            return;
        }
    }

    // Anchor on the LOCAL completion day (the user's timezone): a "+1 day
    // backlog" task completed just after local midnight must resurface
    // TOMORROW. Deriving from the UTC `completed_at` lands it on today (UTC is
    // still yesterday at that hour) → it never leaves the active backlog.
    let completion_date = completed
        .completed_at
        .map(|dt| dt.with_timezone(&chrono::Local).date_naive())
        .unwrap_or_else(|| chrono::Local::now().date_naive());
    let Some(next) = next_recurrence_instance(completed, completion_date) else {
        return;
    };
    if let Err(err) = ext.create_task(&completed.list_id, next).await {
        tracing::warn!(
            account = %account,
            list = %completed.list_id,
            ?err,
            "external on-demand recurrence spawn failed",
        );
    }
}

/// Native recurrence: the provider advanced the live task itself; leave a
/// completion record behind so the just-completed turn survives under "Done".
async fn record_completion(ext: &dyn TasksFeature, cached: &[Task], completed: &Task) {
    let record = completion_record_for(completed);
    // Idempotency: skip when a matching completion record already exists in the
    // cached snapshot (a re-run, or a peer that already recorded this turn). A
    // record is a completed, non-recurring task with the same title + date(s).
    let already = cached.iter().any(|t| {
        t.status == TaskStatus::Completed
            && t.recurrence.is_none()
            && t.title == record.title
            && t.scheduled_date == record.scheduled_date
            && t.deadline_date == record.deadline_date
    });
    if already {
        return;
    }
    if let Err(err) = ext.create_task(&completed.list_id, record).await {
        tracing::warn!(
            list = %completed.list_id,
            ?err,
            "external recurrence completion-record create failed",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use async_trait::async_trait;
    use cal_core::{
        Adapter, AuthToken, Capability, Credentials, NewTask, RecurrenceFrequency, Section,
        TaskList, TaskPriority, TaskRecurrence,
    };
    use chrono::NaiveDate;
    use rusqlite::params;
    use std::sync::Mutex;

    const ACC: &str = "acc-1";
    const LIST: &str = "list-1";

    /// Records what the reconciler asks the provider to create — the whole
    /// question these tests answer is whether it asks at all.
    #[derive(Default)]
    struct RecordingAdapter {
        created: Mutex<Vec<NewTask>>,
    }

    #[async_trait]
    impl Adapter for RecordingAdapter {
        async fn authenticate(&self, _credentials: Credentials) -> cal_core::Result<AuthToken> {
            Err(cal_core::Error::Unsupported("test fake".into()))
        }
        fn capabilities(&self) -> &[Capability] {
            &[]
        }
    }

    #[async_trait]
    impl TasksFeature for RecordingAdapter {
        async fn list_task_lists(&self) -> cal_core::Result<Vec<TaskList>> {
            Ok(Vec::new())
        }
        async fn get_tasks(&self, _list_id: &str) -> cal_core::Result<Vec<Task>> {
            Ok(Vec::new())
        }
        async fn create_task(&self, _list_id: &str, task: NewTask) -> cal_core::Result<Task> {
            self.created.lock().unwrap().push(task.clone());
            Ok(task_from(&task))
        }
        async fn update_task(&self, task: Task) -> cal_core::Result<Task> {
            Ok(task)
        }
        async fn delete_task(&self, _task_id: &str) -> cal_core::Result<()> {
            Ok(())
        }
        async fn list_sections(&self, _list_id: &str) -> cal_core::Result<Vec<Section>> {
            Ok(Vec::new())
        }
    }

    fn task_from(new: &NewTask) -> Task {
        let mut task = base_task("created");
        task.title = new.title.clone();
        task.status = new.status;
        task.recurrence = new.recurrence.clone();
        task.scheduled_date = new.scheduled_date;
        task
    }

    fn base_task(id: &str) -> Task {
        Task {
            id: id.into(),
            list_id: LIST.into(),
            title: "Bins out".into(),
            description: None,
            status: TaskStatus::Open,
            priority: TaskPriority::Medium,
            effort: Default::default(),
            scheduled_date: NaiveDate::from_ymd_opt(2026, 8, 3),
            scheduled_time: None,
            deadline_date: None,
            deadline_time: None,
            deadline_reminder_days: None,
            recurrence: None,
            resurface_date: None,
            series_id: None,
            parent_id: None,
            section_id: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
            assignees: Vec::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            completed_at: None,
            etag: None,
        }
    }

    /// A plain weekly rule — exactly what a provider like Vikunja repeats
    /// itself, so `recurrence_needs_extras` is false and the NATIVE branch runs.
    fn native_weekly() -> TaskRecurrence {
        TaskRecurrence {
            frequency: RecurrenceFrequency::Weekly,
            interval: 1,
            day_of_week: None,
            day_of_month: None,
            fixed_dates: None,
            end: None,
            anchor: Default::default(),
            placement: Default::default(),
        }
    }

    fn cache_with(pre_write: &[Task]) -> CacheStore {
        let db = DbHandle::open_in_memory().unwrap();
        // The cache rows FK onto an account, so seed one through our own handle
        // — `CacheStore`'s is private to its module.
        let store = CacheStore::new(db.clone());
        db.with_conn(|c| {
            c.execute(
                "INSERT INTO accounts (id, adapter_kind, display_name, config_json, \
                     created_at, updated_at) VALUES (?1, 'vikunja', 'Work', '{}', \
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                params![ACC],
            )
        })
        .unwrap();
        store.replace_list_tasks(ACC, LIST, pre_write).unwrap();
        store
    }

    /// The behaviour the whole feature exists for: a provider that only
    /// advances a repeating task leaves nothing under "Done", so Aperio records
    /// the turn that was just finished.
    #[tokio::test]
    async fn a_native_recurring_completion_leaves_a_record() {
        let mut live = base_task("t1");
        live.recurrence = Some(native_weekly());
        let cache = cache_with(std::slice::from_ref(&live));

        let mut completed = live.clone();
        completed.status = TaskStatus::Completed;

        let ext = RecordingAdapter::default();
        record_external_recurrence_completion(&ext, &cache, ACC, &completed).await;

        let created = ext.created.lock().unwrap();
        assert_eq!(created.len(), 1, "expected exactly one completion record");
        let record = &created[0];
        assert_eq!(record.title, "Bins out");
        assert_eq!(record.status, TaskStatus::Completed);
        // Terminal: nothing may repeat it and nothing may spawn from it.
        assert!(record.recurrence.is_none());
        assert!(record.series_id.is_none());
        assert_eq!(record.scheduled_date, live.scheduled_date);
    }

    /// Re-saving an already-done task must not record the same turn twice.
    #[tokio::test]
    async fn an_already_completed_task_records_nothing() {
        let mut live = base_task("t1");
        live.recurrence = Some(native_weekly());
        live.status = TaskStatus::Completed;
        let cache = cache_with(std::slice::from_ref(&live));

        let ext = RecordingAdapter::default();
        record_external_recurrence_completion(&ext, &cache, ACC, &live).await;

        assert!(ext.created.lock().unwrap().is_empty());
    }

    /// A record for the same turn already in the snapshot — a re-run, or a peer
    /// that got there first.
    #[tokio::test]
    async fn an_existing_record_for_this_turn_is_not_duplicated() {
        let mut live = base_task("t1");
        live.recurrence = Some(native_weekly());
        let mut existing = base_task("t2");
        existing.status = TaskStatus::Completed;
        let cache = cache_with(&[live.clone(), existing]);

        let mut completed = live.clone();
        completed.status = TaskStatus::Completed;

        let ext = RecordingAdapter::default();
        record_external_recurrence_completion(&ext, &cache, ACC, &completed).await;

        assert!(ext.created.lock().unwrap().is_empty());
    }

    /// A task with no rule at all is nobody's business here.
    #[tokio::test]
    async fn a_non_recurring_completion_records_nothing() {
        let live = base_task("t1");
        let cache = cache_with(std::slice::from_ref(&live));

        let mut completed = live.clone();
        completed.status = TaskStatus::Completed;

        let ext = RecordingAdapter::default();
        record_external_recurrence_completion(&ext, &cache, ACC, &completed).await;

        assert!(ext.created.lock().unwrap().is_empty());
    }
}
