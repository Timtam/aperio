import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import {
  carryOnto,
  occurrenceCarryRow,
  planCarry,
  type CarryableFields,
  type CarryScope,
  type EventGroup,
} from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import { FocusableNote } from '../a11y/FocusableNote';
import {
  addEventExdate,
  createEvent,
  getEventById,
  isCommandError,
  updateEvent,
} from '../api/client';
import type { CalendarEvent } from '../api/types';
import { useCalendarStore } from '../state/calendarStoreContext';
import { Modal } from './Modal';

/**
 * "Carry this change to the other copies?" (`DESIGN-event-groups.md`, Stufe 2).
 *
 * Asked AFTER the edit is saved, never before. The user's change is safe
 * whatever they answer here, and a dialog that can only add work is a dialog
 * they may cancel without losing anything — which is also why it is a plain
 * question rather than a scope built into Save.
 *
 * It says what it will do before doing it, and what it did afterwards,
 * including the copies it could not touch. A colleague's calendar is read-only,
 * and skipping it quietly is how a group ends up meaning two different times.
 */
/** One copy the carry may write, as `planCarry` describes it. */
type CarryTarget = ReturnType<typeof planCarry>['targets'][number];

export interface EventGroupCarryDialogProps {
  isOpen: boolean;
  onClose: () => void;
  group: EventGroup;
  /** The copy that was edited — already saved, and left alone here. */
  anchor: { calendar_id: string; event_id: string };
  before: CarryableFields;
  after: CarryableFields;
  /** Which occurrences this carry is about.
   *
   *  `series` updates each copy's row. `occurrence` does to each copy what the
   *  edit did to the anchor: EXDATE the series at that instant and put a
   *  standalone event in its place — anything else would move every occurrence
   *  of a copy because one of them was edited. */
  scope?: CarryScope;
  /** The occurrence's original instant, for `scope: 'occurrence'`. */
  occurrence?: string | null;
  onChanged?: () => void;
}

export function EventGroupCarryDialog({
  isOpen,
  onClose,
  group,
  anchor,
  before,
  after,
  scope = 'series',
  occurrence,
  onChanged,
}: EventGroupCarryDialogProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { calendars } = useCalendarStore();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /**
   * What the last attempt achieved, once some of it failed.
   *
   * A partial carry is the one outcome the user MUST act on — the group now
   * means two different things — and it used to be reported by an
   * announcement while the dialog closed underneath it. A screen reader
   * speaks that once, into a view that has just changed; a sighted user gets
   * nothing at all. So the dialog stays open and says it, and the button
   * turns into a retry over exactly what is left.
   */
  const [outcome, setOutcome] = useState<{ done: number; failed: CarryTarget[] } | null>(
    null,
  );
  const [pending, setPending] = useState<CarryTarget[] | null>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const outcomeRef = useRef<HTMLParagraphElement>(null);
  const messageId = useId();

  const calendarName = useCallback(
    (id: string) => calendars.find((c) => c.id === id)?.name ?? id,
    [calendars],
  );

  const plan = useMemo(
    () =>
      planCarry(
        group,
        anchor,
        before,
        after,
        (id) => {
          const cal = calendars.find((c) => c.id === id);
          // Unknown means a calendar this device no longer holds — treat it as
          // unwritable rather than trying and failing halfway through.
          return cal != null && !cal.read_only;
        },
        (calendarId, eventId) =>
          group.members.find(
            (m) => m.calendar_id === calendarId && m.event_id === eventId,
          )?.title ?? eventId,
      ),
    [group, anchor, before, after, calendars],
  );

  useEffect(() => {
    if (!isOpen) return;
    setError(null);
    setOutcome(null);
    setPending(null);
    queueMicrotask(() => closeRef.current?.focus());
  }, [isOpen]);

  // After a partial carry, only what is still outstanding.
  const targets = pending ?? plan.targets;

  const carry = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    const failed: CarryTarget[] = [];
    let done = outcome?.done ?? 0;
    for (const target of targets) {
      try {
        const current = await getEventById(target.event_id, target.calendar_id);
        if (current == null) {
          // The copy is gone from under us. Reported, not silently counted.
          failed.push(target);
          continue;
        }
        if (scope === 'occurrence' && occurrence) {
          // What the edit did to the anchor, done to this copy: carve the
          // occurrence out of its series and put a standalone event there.
          // Updating the row instead would move EVERY occurrence of the copy
          // because one of them was edited — the outcome the scope prompt
          // exists to prevent.
          const row = occurrenceCarryRow(
            current as CalendarEvent & CarryableFields,
            occurrence,
            after,
            plan.changed,
          );
          await addEventExdate(target.event_id, occurrence, target.calendar_id);
          await createEvent({
            calendar_id: target.calendar_id,
            title: row.title,
            description: row.description,
            location: row.location,
            start: row.start,
            end: row.end,
            all_day: row.all_day,
            recurrence: null,
            // The copy keeps its own: what travels is what the appointment IS.
            color_label: current.color_label,
            reminders: current.reminders,
            sound: null,
            attendees: current.attendees,
            send_invitations: false,
          });
        } else {
          const next = carryOnto(
            current as CalendarEvent & CarryableFields,
            after,
            plan.changed,
          );
          await updateEvent(next, target.calendar_id);
        }
        done += 1;
      } catch (err) {
        failed.push(target);
        if (isCommandError(err)) setError(err.message);
      }
    }
    setBusy(false);
    onChanged?.();
    // The whole point of the dialog: say what actually happened, including
    // what did not.
    if (failed.length > 0) {
      // Named by CALENDAR, not by title: every copy of an appointment carries
      // the same title — that is what made them a group — so a list of titles
      // told the user nothing about which copy is now out of step.
      const names = failed.map((target) => calendarName(target.calendar_id));
      setOutcome({ done, failed });
      setPending(failed);
      announce(
        t('dialogs.eventGroupCarry.partly', { done, failed: names.join(', ') }),
      );
      // Stays open. A half-carried group is exactly the state this feature
      // exists to prevent, so the user has to see it and can retry the rest.
      queueMicrotask(() => outcomeRef.current?.focus());
      return;
    }
    announce(t('dialogs.eventGroupCarry.done', { count: done }));
    onClose();
  };

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.eventGroupCarry.title')}
      describedById={messageId}
    >
      {outcome ? (
        <FocusableNote
          ref={outcomeRef}
          id={messageId}
          className="form__hint form__hint--warning"
        >
          {t('dialogs.eventGroupCarry.partly', {
            done: outcome.done,
            failed: outcome.failed
              .map((target) => calendarName(target.calendar_id))
              .join(', '),
          })}
        </FocusableNote>
      ) : (
        <FocusableNote id={messageId} className="form__message">
          {t('dialogs.eventGroupCarry.message', {
            count: plan.targets.length,
            fields: plan.changed
              .map((field) => t(`dialogs.eventGroupCarry.field.${field}`))
              .join(', '),
          })}
        </FocusableNote>
      )}

      {/* Named by their calendar. The title is the same on every copy — that
          is what made them a group — and the stored one is the title the copy
          had when it JOINED, which an edit since then has already outdated. */}
      {targets.map((target) => (
        <FocusableNote
          key={`${target.calendar_id} ${target.event_id}`}
          className="form__message"
        >
          {t('dialogs.eventGroupCarry.target', {
            calendar: calendarName(target.calendar_id),
          })}
        </FocusableNote>
      ))}

      {/* The copies it may not write. Said BEFORE the user decides, because
          "carry to all" that silently means "to some" is the contradiction
          this feature exists to prevent. */}
      {plan.skipped.map((target) => (
        <FocusableNote
          key={`skip ${target.calendar_id} ${target.event_id}`}
          className="form__hint form__hint--warning"
        >
          {t('dialogs.eventGroupCarry.skipped', {
            calendar: calendarName(target.calendar_id),
          })}
        </FocusableNote>
      ))}

      {error && (
        <p className="form__error" role="alert">
          {error}
        </p>
      )}

      <div className="form__actions">
        <button
          ref={closeRef}
          type="button"
          onClick={onClose}
          className="form__action"
        >
          {t(
            outcome
              ? 'dialogs.eventGroupCarry.dismiss'
              : 'dialogs.eventGroupCarry.keep',
          )}
        </button>
        <button
          type="button"
          onClick={() => void carry()}
          className="form__action form__action--primary"
          aria-disabled={busy || undefined}
        >
          {t(
            outcome
              ? 'dialogs.eventGroupCarry.retry'
              : 'dialogs.eventGroupCarry.carry',
          )}
        </button>
      </div>
    </Modal>
  );
}
