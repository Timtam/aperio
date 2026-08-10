import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import {
  carryOnto,
  firstOccurrenceFrom,
  futureCarryRow,
  occurrenceCarryRow,
  planCarry,
  planSeriesSplit,
  seriesLeftTruncated,
  writeSeriesSplit,
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
  groupEvents,
  isCommandError,
  ungroupEvent,
  updateEvent,
  type NewGroupMember,
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

/** How the last attempt ended, when it did not simply succeed. */
type CarryOutcome =
  /** Some copies could not be written. */
  | { kind: 'partly'; done: number; failed: CarryTarget[] }
  /** Every copy was written, but the new rows are not tied together yet. */
  | { kind: 'regroup'; done: number };

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
  /**
   * The row the anchor's edit left behind, when the edit made one.
   *
   * An occurrence edit carves a standalone event out of the series; a "this
   * and all following" edit cuts the series in two. Either way the anchor's
   * new row is OUTSIDE the group, and so is every copy this dialog creates —
   * from that point on the appointment would be read out four times again,
   * having just been made one. So the new rows are grouped with each other
   * once the carry is done, and this is the anchor's half of that.
   */
  successor?: NewGroupMember | null;
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
  successor,
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
  const [outcome, setOutcome] = useState<CarryOutcome | null>(null);
  const [pending, setPending] = useState<CarryTarget[] | null>(null);
  /** The rows written so far, kept across a retry so the regrouping at the end
   *  ties ALL of them together and not just the last pass's. */
  const [createdRows, setCreatedRows] = useState<NewGroupMember[]>([]);
  const closeRef = useRef<HTMLButtonElement>(null);
  const outcomeRef = useRef<HTMLParagraphElement>(null);
  /**
   * Whether this dialog is still the one on screen.
   *
   * The carry is a loop of provider round trips and the dialog stays closable
   * throughout (Escape, the backdrop, "Leave the others"). Without this it went
   * on writing copy after copy into a dialog the user had dismissed, and its
   * closing `onClose()` popped whatever they had opened since. The writes
   * already issued cannot be recalled; the remaining ones stop.
   */
  const open = useRef(isOpen);
  useEffect(() => {
    open.current = isOpen;
  }, [isOpen]);
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
    setCreatedRows([]);
    queueMicrotask(() => closeRef.current?.focus());
  }, [isOpen]);

  // The outcome note takes focus once React has actually rendered it. Focusing
  // it from inside `carry` could not work: the note does not exist yet at that
  // point, so the call found nothing and the report went unread.
  useEffect(() => {
    if (outcome) outcomeRef.current?.focus();
  }, [outcome]);

  // After a partial carry, only what is still outstanding.
  const targets = pending ?? plan.targets;
  // A regroup that failed is retried on its own — there are no copies left to
  // write, and the button must not look like it does nothing.
  const canRetry = targets.length > 0 || outcome?.kind === 'regroup';

  const carry = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    const failed: CarryTarget[] = [];
    // The rows created so far, to be joined into a group of their own — the
    // earlier passes' included, so a retry ties the whole set together.
    const created: NewGroupMember[] = [...createdRows];
    let done = outcome?.done ?? 0;
    for (const target of targets) {
      // The user closed it. Stop writing copies into a dialog that is gone.
      if (!open.current) return;
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
          const standalone = await createEvent({
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
          created.push({
            calendar_id: standalone.calendar_id,
            event_id: standalone.id,
            title: standalone.title,
            starts_at: standalone.start,
          });
        } else if (scope === 'future' && occurrence) {
          // What the edit did to the anchor: its series was cut in two at the
          // occurrence, and a new one took over from there. Each copy has a
          // series of its OWN, so the same cut is made in it — an update would
          // move every occurrence of the copy because one of them was edited.
          //
          // The copy's own next occurrence at or after the cutoff is the point
          // it is cut at. Usually that is the cutoff itself; a copy patterned
          // differently (fortnightly against weekly) is cut at ITS next one,
          // which is what "and all following" means over there.
          const anchorIso = firstOccurrenceFrom(
            current as CalendarEvent & CarryableFields,
            occurrence,
          );
          if (anchorIso == null) {
            // This copy has nothing left at or after the cutoff. Reported, not
            // counted as done: a copy that silently kept its old shape is the
            // contradiction the group exists to prevent.
            failed.push(target);
            continue;
          }
          // The MOVE, applied to this copy's own cut point — not the
          // anchor's instant. See `futureCarryRow`: writing the anchor's
          // instant onto a copy cut somewhere else left the two halves not
          // meeting, and could even put the end before the start.
          const row = futureCarryRow(
            current as CalendarEvent & CarryableFields,
            anchorIso,
            before,
            after,
            plan.changed,
          );
          const splitPlan = planSeriesSplit(
            current as CalendarEvent & CarryableFields,
            anchorIso,
          );
          const currentRecurrence = current.recurrence;
          if (splitPlan == null || currentRecurrence == null) {
            // The copy is a single event: it has one occurrence, and "this and
            // all following" is that one — so the whole copy IS the tail. It
            // leaves the group of heads and joins the new rows, or the split
            // would strand it with a group it no longer belongs to.
            await updateEvent(row, target.calendar_id);
            // Bookkeeping, not a refusal: this copy is on its way straight
            // back into the new group two steps down.
            await ungroupEvent(target.calendar_id, target.event_id, true).catch(
              () => undefined,
            );
            created.push({
              calendar_id: target.calendar_id,
              event_id: target.event_id,
              title: row.title,
              starts_at: row.start,
            });
          } else {
            const tail = await writeSeriesSplit(
              {
                truncate: (headRule) =>
                  updateEvent(
                    {
                      ...current,
                      recurrence: { ...currentRecurrence, rrule: headRule },
                      // The copies are the user's own bookkeeping — the invite
                      // went out from the anchor, and telling every copy's
                      // attendees again would mail them twice.
                      send_invitations: false,
                      truncate_tail_overrides: true,
                    },
                    target.calendar_id,
                  ),
                createTail: (recurrence) =>
                  createEvent(
                    {
                      calendar_id: target.calendar_id,
                      title: row.title,
                      description: row.description,
                      location: row.location,
                      start: row.start,
                      end: row.end,
                      all_day: row.all_day,
                      recurrence,
                      // The copy keeps its own: what travels is what the
                      // appointment IS.
                      color_label: current.color_label,
                      reminders: current.reminders,
                      sound: null,
                      attendees: current.attendees,
                      send_invitations: false,
                    },
                    // A continuation of the copy's own series — its zone stays
                    // verbatim so both halves expand alike.
                    { preserveRecurrenceZone: true },
                  ),
                restore: () =>
                  updateEvent(
                    { ...current, send_invitations: false },
                    target.calendar_id,
                  ),
              },
              splitPlan,
            );
            created.push({
              calendar_id: tail.calendar_id,
              event_id: tail.id,
              title: tail.title,
              starts_at: tail.start,
            });
          }
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
        // A split that failed AND could not be undone leaves this copy's series
        // ending at the cutoff. Reporting that as "not changed" would be the
        // opposite of true, so it gets said in its own words.
        if (seriesLeftTruncated(err)) {
          setError(
            t('dialogs.eventGroupCarry.truncatedNotRestored', {
              calendar: calendarName(target.calendar_id),
            }),
          );
        } else if (isCommandError(err)) {
          setError(err.message);
        }
      }
    }
    // The rows this carry created are copies of each other exactly as the ones
    // it came from were — so they are told so. Without this, an occurrence or
    // "and all following" edit would quietly UNDO the grouping from that point
    // on: the appointment the user had just made one row would be four again.
    //
    // A failure here leaves the writes done and correct, so it is not counted
    // against any copy; it is said in its own words, because the group is what
    // the user came for.
    // Tie the new rows together even if the user has already closed this: they
    // are written either way, and the grouping is a local step that needs no
    // dialog. The successor is left out when the anchor's own new row is an
    // override — its id carries `::rid::`, which no group lookup resolves, so
    // registering it would add a member nothing can ever match; the created
    // rows are still tied to each other.
    const members = [...(successor ? [successor] : []), ...created];
    let regroupFailed = false;
    if (members.length >= 2) {
      try {
        await groupEvents(members);
      } catch {
        regroupFailed = true;
      }
    }
    if (!open.current) return;
    setCreatedRows(created);
    setBusy(false);
    onChanged?.();
    // The whole point of the dialog: say what actually happened, including
    // what did not.
    if (failed.length > 0) {
      // Named by CALENDAR, not by title: every copy of an appointment carries
      // the same title — that is what made them a group — so a list of titles
      // told the user nothing about which copy is now out of step.
      const names = failed.map((target) => calendarName(target.calendar_id));
      setOutcome({ kind: 'partly', done, failed });
      setPending(failed);
      announce(
        t('dialogs.eventGroupCarry.partly', { done, failed: names.join(', ') }),
      );
      // Stays open. A half-carried group is exactly the state this feature
      // exists to prevent, so the user has to see it and can retry the rest.
      // (The focus happens in the effect below — at this point React has not
      // rendered the note yet, so there is nothing here to focus.)
      return;
    }
    if (regroupFailed) {
      // Every copy was written; only the new rows are not tied together yet.
      // Said, and left on screen, because "the appointment is one row again" is
      // what the user was promised.
      setOutcome({ kind: 'regroup', done });
      setPending([]);
      announce(t('dialogs.eventGroupCarry.regroupFailed', { count: done }));
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
          {outcome.kind === 'regroup'
            ? t('dialogs.eventGroupCarry.regroupFailed', { count: outcome.done })
            : t('dialogs.eventGroupCarry.partly', {
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

      {/* Which occurrences this is about. Without it the question reads the
          same whether it will touch one appointment or a hundred. */}
      {!outcome && scope !== 'series' && (
        <FocusableNote className="form__hint">
          {t(
            scope === 'future'
              ? 'dialogs.eventGroupCarry.scopeFuture'
              : 'dialogs.eventGroupCarry.scopeOccurrence',
          )}
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
          aria-disabled={busy || !canRetry || undefined}
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
