import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';

import { useAnnouncer } from '../a11y/Announcer';
import { isCommandError } from '../api/client';
import type { Task } from '../api/types';
import { Modal } from './Modal';

/**
 * Plan-task dialog (DESIGN.md §9.3 — "Einplanen aus dem Backlog").
 *
 * Triggered by Shift+D on a focused task. Lets the user pick a
 * scheduled date with three flavours:
 *
 *   - **Quick presets** (Heute / Morgen / Nächste Woche) — radio-button
 *     style buttons, the dominant interaction for backlog grooming
 *   - **Custom date** — native `<input type="date">` for any other
 *     date; tab-accessible, screen reader friendly via aria-label
 *   - **Back to backlog** — clears `scheduled_date` *and*
 *     `deadline_date`; reopens the task as pure backlog. Status flips
 *     to `Open` if it was anything else.
 *
 * Save dispatches `update_task` against the backend; the dialog
 * doesn't reach into useTasks/useEvents directly — `invalidateData()`
 * bumps the global counter and both hooks refetch.
 *
 * Auto-focus lands on the first quick-preset (Today) — the most
 * frequently-used action. Esc cancels without writing.
 */
export interface PlanTaskDialogProps {
  isOpen: boolean;
  onClose: () => void;
  task: Task | null;
  onPlanned: () => void;
}

type Choice =
  | { kind: 'date'; iso: string }
  | { kind: 'backlog' };

export function PlanTaskDialog({
  isOpen,
  onClose,
  task,
  onPlanned,
}: PlanTaskDialogProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const titleId = useId();
  const hintId = useId();
  const todayRef = useRef<HTMLButtonElement>(null);

  // Local custom-date input — preserved across re-renders while the
  // dialog is open so the user can type a partial date, click Today
  // by mistake, click custom again, and find their text.
  const [customDate, setCustomDate] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Reset on open: preload the custom input with the task's current
  // scheduled date (if any) so editing-after-planning is one keypress
  // less work.
  useEffect(() => {
    if (!isOpen) return;
    setCustomDate(task?.scheduled_date ?? '');
    setError(null);
    queueMicrotask(() => todayRef.current?.focus());
  }, [isOpen, task]);

  const commit = useCallback(
    async (choice: Choice) => {
      if (!task) return;
      setSubmitting(true);
      setError(null);
      try {
        const updated: Task = {
          ...task,
          scheduled_date: choice.kind === 'date' ? choice.iso : null,
          // "Back to backlog" also clears the deadline so the task is
          // truly unscheduled — otherwise a `By` deadline alone would
          // keep pulling it into upcoming views.
          deadline_date:
            choice.kind === 'backlog' ? null : task.deadline_date,
          deadline_type:
            choice.kind === 'backlog' ? null : task.deadline_type,
          deadline_time:
            choice.kind === 'backlog' ? null : task.deadline_time,
          status:
            choice.kind === 'backlog' && task.status === 'completed'
              ? 'open'
              : task.status,
          // Bump the timestamp so sync engines pick up the change.
          updated_at: new Date().toISOString(),
        };
        await invoke<Task>('update_task', { task: updated });
        const announcement =
          choice.kind === 'date'
            ? t('dialogs.plan.plannedAnnouncement', {
                title: task.title,
                date: formatIsoDate(choice.iso),
              })
            : t('dialogs.plan.backloggedAnnouncement', {
                title: task.title,
              });
        announce(announcement);
        onPlanned();
        onClose();
      } catch (err) {
        if (isCommandError(err)) {
          setError(`${err.code}: ${err.message}`);
        } else {
          setError(String(err));
        }
      } finally {
        setSubmitting(false);
      }
    },
    [task, t, announce, onPlanned, onClose],
  );

  if (!task) return null;

  const todayIso = isoToday();
  const tomorrowIso = isoTomorrow();
  const nextWeekIso = isoNextMonday();

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.plan.title', { title: task.title })}
      className="modal--form modal--plan"
    >
      <p id={hintId} className="form__hint">
        {t('dialogs.plan.hint')}
      </p>
      {error && (
        <p role="alert" className="form__error">
          {error}
        </p>
      )}
      <div
        className="plan-dialog"
        role="group"
        aria-labelledby={titleId}
        aria-describedby={hintId}
      >
        <h3 id={titleId} className="form__label">
          {t('dialogs.plan.quickPresets')}
        </h3>
        <div className="plan-dialog__presets">
          <button
            ref={todayRef}
            type="button"
            className="form__action"
            onClick={() => void commit({ kind: 'date', iso: todayIso })}
            aria-disabled={submitting || undefined}
          >
            {t('dialogs.plan.today')}
          </button>
          <button
            type="button"
            className="form__action"
            onClick={() => void commit({ kind: 'date', iso: tomorrowIso })}
            aria-disabled={submitting || undefined}
          >
            {t('dialogs.plan.tomorrow')}
          </button>
          <button
            type="button"
            className="form__action"
            onClick={() => void commit({ kind: 'date', iso: nextWeekIso })}
            aria-disabled={submitting || undefined}
          >
            {t('dialogs.plan.nextWeek')}
          </button>
        </div>

        <label className="form__field plan-dialog__custom">
          <span className="form__label">{t('dialogs.plan.customDate')}</span>
          <input
            type="date"
            value={customDate}
            onChange={(e) => setCustomDate(e.target.value)}
            aria-describedby={hintId}
          />
          <button
            type="button"
            className="form__action"
            onClick={() => {
              if (!customDate) {
                setError(t('dialogs.plan.customDateRequired'));
                return;
              }
              void commit({ kind: 'date', iso: customDate });
            }}
            aria-disabled={submitting || undefined}
          >
            {t('dialogs.plan.applyCustom')}
          </button>
        </label>

        <div className="plan-dialog__separator" role="presentation" />

        <button
          type="button"
          className="form__action form__action--secondary"
          onClick={() => void commit({ kind: 'backlog' })}
          aria-disabled={submitting || undefined}
        >
          {t('dialogs.plan.backToBacklog')}
        </button>
      </div>

      <div className="form__actions">
        <button
          type="button"
          onClick={onClose}
          className="form__action"
          aria-disabled={submitting || undefined}
        >
          {t('dialogs.plan.cancel')}
        </button>
      </div>
    </Modal>
  );
}

// ── Date helpers ────────────────────────────────────────────────────────
// Tasks store dates as bare `YYYY-MM-DD` ISO strings (no time, no zone)
// — matching cal-core's `NaiveDate` on the wire. The helpers below
// produce strings in the user's local calendar day, which is what the
// "Heute"/"Morgen" semantics implicitly mean.

function isoToday(): string {
  return toIso(new Date());
}

function isoTomorrow(): string {
  const d = new Date();
  d.setDate(d.getDate() + 1);
  return toIso(d);
}

/**
 * "Next Monday" — matches Outlook / Apple Calendar's interpretation
 * of "next week" as "the upcoming Monday". When today *is* Monday,
 * lands one week out (next-next isn't useful as a quick preset).
 */
function isoNextMonday(): string {
  const d = new Date();
  const dow = d.getDay(); // 0=Sun..6=Sat
  // Days until Monday: Sun→1, Mon→7, Tue→6, …, Sat→2.
  const daysToMon = dow === 0 ? 1 : (8 - dow) % 7 || 7;
  d.setDate(d.getDate() + daysToMon);
  return toIso(d);
}

function toIso(d: Date): string {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${y}-${m}-${day}`;
}

function formatIsoDate(iso: string): string {
  // Lightweight human-readable variant for the SR announcement; the
  // dialog itself uses the locale's date picker which is already
  // localised by the browser.
  const d = new Date(`${iso}T00:00:00`);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleDateString();
}

/** Test-only re-export of the date helpers so unit tests can hit
 *  them without rendering the dialog. Pure functions, no side
 *  effects beyond `new Date()`. */
export const __test = {
  isoToday,
  isoTomorrow,
  isoNextMonday,
};
