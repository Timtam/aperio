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
