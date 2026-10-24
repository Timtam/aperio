import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { writeErrorMessage,
  deletionsNotRestoredSentence,
  exceptionsLossOf,
  exceptionsLossSentences,
  type ExceptionsLoss,
  carvedOccurrenceKeptSentence,
  removeOriginalAfterCopy,
} from '@aperio/shared';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  findNodeHandle,
  Pressable,
  StyleSheet,
  Text,
} from 'react-native';

import {
  carryOnto,
  firstOccurrenceFrom,
  futureCarryRow,
  occurrenceCarryRow,
  organizerOf,
  planCarry,
  planSeriesSplit,
  tailRecurrenceFor,
  TailShiftRefusedError,
  cutoffDay,
  readSeriesRows,
  seriesMaybeShownTwice,
  seriesTailMayStand,
  createsInSteps,
  writeSeriesSplit,
  type CarryableFields,
} from '@aperio/shared';

import {
  addEventExdate,
  createEvent,
  deleteEvent,
  getEventById,
  getSeriesRows,
  listCalendars,
  updateEvent,
  type Calendar,
  type CalendarEvent,
} from '../api/calendar';
import {
  eventGroupsForEvents,
  groupEvents,
  ungroupEvent,
  type NewGroupMember,
} from '../api/eventGroups';
import { FormScrollView } from '../components/FormScrollView';
import { StepWriteWaitHost } from '../components/StepWriteWaitHost';
import { useCancelHeader } from '../components/useCancelHeader';
import type { RootStackScreenProps } from '../navigation/types';
import { showEventScopeDialog } from '../state/eventScopeDialog';
import { holdStepWriteWait, stepWriteWait } from '../state/stepWriteWait';
import { useThemedStyles, type ThemeColors } from '../theme';

// "Carry this change to the other copies?" (DESIGN-event-groups.md, Stufe 2) —
// the RN twin of the desktop EventGroupCarryDialog.
//
// Asked AFTER the edit is saved, never before: the user's change is safe
// whatever they answer, so this can only ever add work, and cancelling costs
// nothing. It says what it will do before doing it, and what it did afterwards
// — including the copies it could not touch, because a colleague's calendar is
// read-only and skipping it quietly is how a group ends up meaning two
// different times.


/** One copy the carry may write, as `planCarry` describes it. */
type CarryTarget = ReturnType<typeof planCarry>['targets'][number];

/** How the last attempt ended, when it did not simply succeed. */
type CarryOutcome =
  /** Some copies could not be written. */
  | { kind: 'partly'; done: number; failed: CarryTarget[] }
  /** Every copy was written, but the new rows are not tied together yet. */
  | { kind: 'regroup'; done: number }
  /** No copy is left to offer again, and some series may show twice: a copy
   *  written while its old series may still run through the cut (decision
   *  145), or one whose new part could not be deleted again after its cut
   *  was refused. Said, and left on screen, with nothing to try again. */
  | { kind: 'written'; done: number };

/**
 * The carry to the copies, and beside it the wait of a write in steps
 * (decisions 227, 265): on iOS a dialog presents from the screen it is
 * rendered in, and one at the app root could not present over this sheet.
 * Beside the screen's content, not in it, so it stays mounted whichever view
 * the content shows.
 */
export default function EventGroupCarryModal(props: RootStackScreenProps<'EventGroupCarry'>) {
  return (
    <>
      <EventGroupCarryScreen {...props} />
      <StepWriteWaitHost />
    </>
  );
}

function EventGroupCarryScreen({
  route,
  navigation,
}: RootStackScreenProps<'EventGroupCarry'>) {
  const {
    group,
    anchor,
    before,
    after,
    scope = 'series',
    occurrence,
    successor,
  } = route.params;
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  useCancelHeader(navigation);

  // `null` until they arrive: an empty list makes every copy look read-only,
  // and the screen would state that as fact while it simply did not know yet.
  const [calendars, setCalendars] = useState<Calendar[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /**
   * What the last attempt achieved, once some of it failed.
   *
   * A partial carry is the one outcome the user must act on — the group now
   * means two different things — and it used to be spoken once into a screen
   * that was already dismissing itself. It stays instead, and the button
   * retries exactly what is left.
   */
  const [outcome, setOutcome] = useState<CarryOutcome | null>(null);
  const [pending, setPending] = useState<CarryTarget[] | null>(null);
  /** The rows written so far, kept across a retry so the regrouping at the end
   *  ties ALL of them together and not just the last pass's. */
  const [createdRows, setCreatedRows] = useState<NewGroupMember[]>([]);
  /** What a pass said about copies whose series may now show twice, kept
   *  across a retry: they are written and not offered again, so this is the
   *  only place they are still named. Mirrors the desktop. */
  const [doubts, setDoubts] = useState<string[]>([]);
  /** Whether the question about copies whose series would lose occurrences
   *  (decision 251) is open: it has the cursor until it is answered. */
  const [asking, setAsking] = useState(false);
  /** The outcome line, which takes the cursor whenever a pass ends with an
   *  outcome, as the desktop's outcome note does, and what it then reads: the
   *  outcome, the last failure and this pass's doubts, in one utterance. */
  const outcomeRef = useRef<Text>(null);
  const [outcomeSaid, setOutcomeSaid] = useState('');
  const carryRef = useRef<
    (only?: CarryTarget[], consent?: ReadonlySet<string>, earlier?: string) => Promise<void>
  >(async () => undefined);
  /**
   * Whether this screen is still here.
   *
   * The carry is a loop of provider round trips, and the header's Cancel (or
   * a back swipe) does not interrupt it: the loop went on writing copy after
   * copy into a screen the user had left, then called `goBack()` from an
   * unmounted screen and popped whatever they had opened since. The writes
   * already issued cannot be recalled, but the remaining ones stop.
   */
  const alive = useRef(true);
  useEffect(
    () => () => {
      alive.current = false;
    },
    [],
  );

  useEffect(() => {
    let cancelled = false;
    void listCalendars()
      .then((cals) => {
        if (!cancelled) setCalendars(cals);
      })
      .catch(() => {
        // Still `null`, so the screen keeps saying "loading" rather than
        // announcing every copy as read-only — which is a claim, not a
        // fallback.
        if (!cancelled) setCalendars([]);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const calendarName = useCallback(
    (id: string) => calendars?.find((c) => c.id === id)?.name ?? id,
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
          const cal = calendars?.find((c) => c.id === id);
          // Unknown means a calendar this device no longer holds — treated as
          // unwritable rather than tried and failed halfway through.
          return cal != null && !cal.read_only;
        },
        (calendarId, eventId) =>
          group.members.find(
            (m) => m.calendar_id === calendarId && m.event_id === eventId,
          )?.title ?? eventId,
      ),
    [group, anchor, before, after, calendars],
  );

  // After a partial carry, only what is still outstanding.
  const targets = pending ?? plan.targets;
  // A regroup that failed is retried on its own — there are no copies left to
  // write, and the button must not announce itself disabled when it is the only
  // way to put the appointment back together.
  const canRetry = targets.length > 0 || outcome?.kind === 'regroup';

  /**
   * `only`: the copies to write this pass, the ones the question was about;
   * `consent`: the copies whose series' write carries the user's yes;
   * `earlier`: what the pass that asked did not get to say, said with this
   * pass's outcome. Every other outstanding copy stays outstanding. Mirrors
   * the desktop.
   */
  const carry = useCallback(async (
    only?: CarryTarget[],
    consent?: ReadonlySet<string>,
    earlier = '',
  ) => {
    if (busy) return;
    setBusy(true);
    // The failure on screen belongs to a copy a pass of the yes does not
    // write again: it stays.
    if (!only) setError(null);
    const list = only ?? targets;
    const kept = only
      ? (pending ?? []).filter((target) => !only.some((o) => sameTarget(o, target)))
      : [];
    const failed: CarryTarget[] = [];
    // Copies whose series would lose occurrences: asked about after the loop.
    const asked: { target: CarryTarget; loss: ExceptionsLoss }[] = [];
    // Copies whose series may now show twice from the cut, said in words.
    const unsure: string[] = [];
    // A copy whose series got deleted occurrences back that could not be
    // deleted again (decision 253): saved, and said with this pass's doubts,
    // by its calendar.
    const noteBack = (saved: CalendarEvent, calendarId: string) => {
      const sentence = deletionsNotRestoredSentence(
        saved.deletions_not_restored,
        saved.title,
        i18n.language,
        t,
      );
      if (sentence) unsure.push(`${calendarName(calendarId)}: ${sentence}`);
    };
    // The last failure said on screen, which the outcome line reads too.
    let lastError = '';
    // The rows created so far, to be joined into a group of their own — the
    // earlier passes' included, so a retry ties the whole set together.
    const created: NewGroupMember[] = [...createdRows];
    let done = outcome?.done ?? 0;
    // The "please wait" dialog while a copy's new series is created in steps
    // (decisions 227, 265): opened at the first copy that needs it, and held
    // until the pass is done. Mirrors the desktop.
    const releaseWait = holdStepWriteWait();
    try {
      for (const target of list) {
        // The user left. Stop writing copies into a screen that is gone; what
        // the ones written left is still tied together and said below.
        if (!alive.current) break;
        // The question's yes, for this copy's series (decision 251).
        const agreed = consent?.has(targetKey(target)) ?? false;
        // Only a write of the copy's whole series asks; a split's cut says the
        // sentence, as in the editor (a yes would write its new series again).
        let askable = false;
        // Where this copy's series is split, once it is: a failure after that
        // names the day. Mirrors the desktop.
        let splitAt: string | null = null;
        // Whether part of this copy's carry stands already (its standalone):
        // a failure after that is never "not changed".
        let landed = false;
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
            // Updating the row would move EVERY occurrence of the copy because
            // one of them was edited — the outcome the scope prompt prevents.
            const row = occurrenceCarryRow(
              current as CalendarEvent & CarryableFields,
              occurrence,
              after,
              plan.changed,
            );
            if (row == null) {
              // The instant this copy is cut at cannot be read, so there is no
              // row to write — and inventing one would put an appointment where
              // nobody put it. Reported, not counted as done, exactly like a copy
              // with nothing left after the cutoff.
              failed.push(target);
              continue;
            }
            // The standalone first, then the skip, as the move and the
            // editors do: a create that fails leaves the copy whole, and a skip
            // that fails after it is a later step (decision 277). Mirrors the desktop.
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
              ...organizerOf(current),
            });
            created.push({
              calendar_id: standalone.calendar_id,
              event_id: standalone.id,
              title: standalone.title,
              starts_at: standalone.start,
            });
            landed = true;
            const removed = await removeOriginalAfterCopy('skip', () =>
              addEventExdate(target.event_id, occurrence, target.calendar_id),
            );
            if (!removed) {
              // A busy provider kept the copy's occurrence in its series: said,
              // and counted as written — its standalone stands and joins the
              // new rows — so it is not offered again, which would make a
              // second standalone.
              unsure.push(
                carvedOccurrenceKeptSentence(
                  standalone.title,
                  target.calendar_id,
                  target.calendar_id,
                  calendarName,
                  t,
                ),
              );
            }
          } else if (scope === 'future' && occurrence) {
            // What the edit did to the anchor: its series was cut in two at the
            // occurrence, and a new one took over from there. Each copy has a
            // series of its OWN, so the same cut is made in it — an update would
            // move every occurrence of the copy because one of them was edited.
            //
            // The copy's own next occurrence at or after the cutoff is the point
            // it is cut at. Usually that is the cutoff itself; a copy patterned
            // differently is cut at ITS next one, which is what "and all
            // following" means over there.
            // The copy's own provider-kept occurrences, read first and once: they
            // decide where it is cut as much as how. An occurrence changed
            // elsewhere is there, one deleted as a cancelled row is not
            // (decisions 125, 141). A copy without a rule has none and is not
            // read. Mirrors the desktop.
            const rows = (await readSeriesRows(current, occurrence, getSeriesRows)).rows;
            const anchorIso = firstOccurrenceFrom(
              current as CalendarEvent & CarryableFields,
              occurrence,
              rows,
            );
            if (anchorIso == null) {
              // Nothing left in this copy at or after the cutoff. Reported, not
              // counted as done.
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
            if (row == null) {
              // The instant this copy is cut at cannot be read, so there is no
              // row to write — and inventing one would put an appointment where
              // nobody put it. Reported, not counted as done, exactly like a copy
              // with nothing left after the cutoff.
              failed.push(target);
              continue;
            }
            // With the same rows: one changed elsewhere is still there before
            // the cut, though its master lists it among the exceptions
            // (decision 125), and the new part leaves out what they delete.
            const splitPlan = planSeriesSplit(
              current as CalendarEvent & CarryableFields,
              anchorIso,
              rows,
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
            } else if (splitPlan.kind === 'whole') {
              // Nothing of this copy comes before its cut point, so "this and
              // all following" is the whole copy (decision 118): cutting it
              // would leave a head that ends before it starts, hidden from the
              // views and still ringing. It is rewritten in place instead — its
              // id, and everything kept under it, stay — and like a single it
              // leaves the group of heads for the new one. Mirrors the desktop.
              askable = true;
              const savedWhole = await updateEvent(
                {
                  ...row,
                  // What the copy repeats by from its cut on — its rule moved
                  // with a new date (decision 189) — and its deleted
                  // occurrences, carried along with a new date, time or kind
                  // as the anchor's are (decision 152).
                  recurrence: tailRecurrenceFor({
                    master: current,
                    cutoffIso: anchorIso,
                    plan: splitPlan,
                    tail: { start: row.start, all_day: row.all_day },
                    opened: anchorIso,
                  }),
                  send_invitations: false,
                  ...(agreed ? { accepts_exception_loss: true } : {}),
                },
                target.calendar_id,
              );
              askable = false;
              noteBack(savedWhole, target.calendar_id);
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
              splitAt = anchorIso;
              // Decided before anything is written: the copy's rule, moved
              // with a new date (decision 189), and its deleted occurrences,
              // carried along with a new date, time or kind as the anchor's
              // are (decision 152).
              const tailRecurrence = tailRecurrenceFor({
                master: current,
                cutoffIso: anchorIso,
                plan: splitPlan,
                tail: { start: row.start, all_day: row.all_day },
                opened: anchorIso,
              });
              const written = await stepWriteWait(
                createsInSteps(
                  calendars?.find((c) => c.id === target.calendar_id),
                  tailRecurrence,
                ),
                () => writeSeriesSplit(
                  {
                    createTail: () =>
                      createEvent(
                        {
                          calendar_id: target.calendar_id,
                          title: row.title,
                          description: row.description,
                          location: row.location,
                          start: row.start,
                          end: row.end,
                          all_day: row.all_day,
                          recurrence: tailRecurrence,
                          // The copy keeps its own: what travels is what the
                          // appointment IS.
                          color_label: current.color_label,
                          reminders: current.reminders,
                          sound: null,
                          attendees: current.attendees,
                          send_invitations: false,
                          ...organizerOf(current),
                        },
                        // A continuation of the copy's own series — its zone stays
                        // verbatim so both halves expand alike.
                        { preserveRecurrenceZone: true },
                      ),
                    // The cut can make the provider bring deleted occurrences
                    // back as well (a zone it writes again, 247); any it could not
                    // delete again are said with this pass's doubts (253).
                    truncate: async (headRule) => {
                      const cut = await updateEvent(
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
                      );
                      noteBack(cut, target.calendar_id);
                    },
                    removeTail: (tail) =>
                      deleteEvent(tail.id, tail.calendar_id, false),
                  },
                  splitPlan,
                ),
              );
              const tail = written.tail;
              // Written, but the copy's old series may still run through the
              // cut (145): the new row is real, so it counts and is grouped, and
              // the doubt is said instead of offering to write it again.
              if (written.headCut === 'unsure') {
                unsure.push(
                  t('dialogs.eventGroupCarry.unsureCut', {
                    calendar: calendarName(target.calendar_id),
                    date: cutoffDay(anchorIso, i18n.language),
                  }),
                );
              }
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
            // A copy that is a series moves as the anchor did when its start
            // moved or it switched between all-day and a time of day: its rule,
            // its deleted occurrences and its zone go along, as from its own
            // start (decision 198). Moved only in its start, it repeated on its
            // old days next to the anchor's new ones. A rule that cannot move
            // reports the copy, with the reason (193).
            const wholePlan =
              current.recurrence?.rrule &&
              (Date.parse(next.start) !== Date.parse(current.start) ||
                next.all_day !== current.all_day)
                ? planSeriesSplit(
                    current,
                    current.start,
                    (await readSeriesRows(current, current.start, getSeriesRows)).rows,
                  )
                : null;
            const seriesWrite =
              wholePlan?.kind === 'whole'
                ? {
                    ...next,
                    recurrence: tailRecurrenceFor({
                      master: current,
                      cutoffIso: current.start,
                      plan: wholePlan,
                      tail: { start: next.start, all_day: next.all_day },
                      opened: current.start,
                      whole: true,
                    }),
                  }
                : next;
            askable = true;
            const savedSeries = await updateEvent(
              agreed ? { ...seriesWrite, accepts_exception_loss: true } : seriesWrite,
              target.calendar_id,
            );
            askable = false;
            noteBack(savedSeries, target.calendar_id);
          }
          done += 1;
        } catch (err) {
          // A split whose new series could not be taken back may show this copy
          // twice from the cut. Reporting that as "not changed" would be the
          // opposite of true, and trying again would write the new series a
          // second time: it is said in its own words, and not offered again.
          // Mirrors the desktop. So is a new part the provider created and
          // could neither complete nor delete again (decisions 203, 210).
          if (seriesTailMayStand(err) && splitAt != null) {
            unsure.push(
              t('dialogs.eventGroupCarry.newPartMayStand', {
                calendar: calendarName(target.calendar_id),
                date: cutoffDay(splitAt, i18n.language),
              }),
            );
            continue;
          }
          if (seriesMaybeShownTwice(err) && splitAt != null) {
            unsure.push(
              t('dialogs.eventGroupCarry.maybeShownTwice', {
                calendar: calendarName(target.calendar_id),
                date: cutoffDay(splitAt, i18n.language),
              }),
            );
            continue;
          }
          if (landed) {
            // Its standalone stands and joins the new rows: written, with the
            // failure after it said, and not offered again.
            done += 1;
            unsure.push(
              `${calendarName(target.calendar_id)}: ${writeErrorMessage(err, t)}`,
            );
            continue;
          }
          failed.push(target);
          // A copy whose series would lose occurrences is asked about once,
          // after every other copy is written (decision 251): the question says
          // what and how many.
          const loss = exceptionsLossOf(err);
          if (loss && askable && !agreed) {
            asked.push({ target, loss });
            continue;
          }
          // A copy whose rule cannot move with the new date wrote nothing, and
          // the user learns why (decision 193).
          const message =
            err instanceof TailShiftRefusedError
              ? t('dialogs.eventGroupCarry.shiftRefused', {
                  calendar: calendarName(target.calendar_id),
                  reason: t(`dialogs.moveScope.refusal.${err.reason}`),
                })
              : writeErrorMessage(err, t);
          if (!alive.current) break;
          setError(message);
          // Said with the outcome, by the outcome line: announced here as well,
          // or through a live region, it cut the outcome off (Android) or was
          // cut off by it (iOS).
          lastError = message;
        }
      }
    } finally {
      // Closed before the report: what it says comes after the dialog is gone.
      await releaseWait();
    }
    // The rows this carry created are copies of each other exactly as the ones
    // it came from were — so they are told so. Without this, an occurrence or
    // "and all following" edit would quietly UNDO the grouping from that point
    // on: the appointment the user had just made one row would be four again.
    //
    // Done even if the user has already left: the rows are written either way,
    // and tying them together is a local step that needs no screen. Leaving it
    // undone would be the one lasting consequence of walking away.
    // The successor is left out when the anchor's own new row is an override —
    // its id carries `::rid::`, which no group lookup resolves, so registering
    // it would add a member nothing can ever match; the created rows are still
    // tied to each other.
    const members = [...(successor ? [successor] : []), ...created];
    let regroupFailed = false;
    if (members.length >= 2) {
      try {
        // A "this and all following" that rewrote the anchor's whole series in
        // place (decision 118) left it where it was: in THIS group, among the
        // heads of the copies. Grouping the new rows with it would pull them
        // in there too. So it leaves first, as bookkeeping — asked, not
        // assumed, because on a retry it may already be in the new group.
        if (successor) {
          const [current] = await eventGroupsForEvents([successor]);
          if (current?.id === group.id) {
            await ungroupEvent(successor.calendar_id, successor.event_id, true);
          }
        }
        await groupEvents(members);
      } catch {
        regroupFailed = true;
      }
    }
    if (!alive.current) {
      // Gone before the pass ended: a copy that may now show twice is still
      // said (210), as nothing else will. Mirrors the desktop.
      if (unsure.length > 0) AccessibilityInfo.announceForAccessibility(unsure.join(' '));
      return;
    }
    setCreatedRows(created);
    const allDoubts = [...doubts, ...unsure];
    setDoubts(allDoubts);
    // Said once, with whatever else this pass reports, by the outcome line that
    // takes the cursor: a second utterance in the same frame would cut the
    // first off, and a queued announcement is queued on iOS only.
    const doubtful = unsure.length === 0 ? '' : ` ${unsure.join(' ')}`;
    const failure = lastError === '' ? '' : ` ${lastError}`;
    setBusy(false);
    // The whole point of the screen: say what actually happened, including
    // what did not.
    // The copies a pass before this one left outstanding stay so.
    const outstanding = [...kept, ...failed];
    if (outstanding.length > 0) {
      // Named by CALENDAR, not by title: every copy of an appointment carries
      // the same title — that is what made them a group — so a list of titles
      // said nothing about which copy is now out of step.
      const names = outstanding.map((target) => calendarName(target.calendar_id));
      // The earlier pass's rest first, as its copies come first among the
      // ones not changed.
      setOutcomeSaid(
        `${t('dialogs.eventGroupCarry.partly', { done, failed: names.join(', ') })}${earlier}${failure}${doubtful}`,
      );
      setOutcome({ kind: 'partly', done, failed: outstanding });
      setPending(outstanding);
      // The question comes now, once, for every copy that would lose
      // occurrences, and the outcome line waits for its answer: after Cancel
      // it takes the cursor and says the pass's report; a yes says its rest
      // — why the copies it does not write again were not changed, and this
      // pass's doubts — with the next pass's outcome.
      if (asked.length > 0) {
        const rest = `${earlier}${failure}${doubtful}`;
        setAsking(true);
        showEventScopeDialog({
          title: t('dialogs.event.exceptionsLoss.title'),
          message: t('dialogs.eventGroupCarry.lossMessage', {
            count: asked.length,
            copies: asked
              .map(({ target, loss }) => {
                const { what, lost } = exceptionsLossSentences(loss, t);
                return `${calendarName(target.calendar_id)}: ${what} ${lost}`;
              })
              .join(' '),
          }),
          cancelLabel: t('dialogs.cancel'),
          onCancel: () => setAsking(false),
          options: [
            {
              key: 'carryAnyway',
              label: t('dialogs.eventGroupCarry.lossConfirm'),
              destructive: true,
              run: () => {
                // Only these copies, now with the consent for each — through
                // the carry of the latest render, which knows what is pending.
                const these = asked.map(({ target }) => target);
                setAsking(false);
                void carryRef.current(these, new Set(these.map(targetKey)), rest);
              },
            },
          ],
        });
        return;
      }
      // Stays open: a half-carried group is the state this feature exists to
      // prevent, so it has to be seen, and the rest can be retried.
      return;
    }
    if (regroupFailed) {
      // Every copy was written; only the new rows are not tied together yet.
      // Said, and left on screen, because "the appointment is one row again" is
      // what the user was promised.
      setOutcomeSaid(
        `${t('dialogs.eventGroupCarry.regroupFailed', { count: done })}${earlier}${doubtful}`,
      );
      setOutcome({ kind: 'regroup', done });
      setPending([]);
      return;
    }
    if (allDoubts.length > 0) {
      // No copy is left to offer again, but some may show twice: leaving would
      // take the only words that say which ones.
      setOutcomeSaid(`${t('dialogs.eventGroupCarry.done', { count: done })}${earlier}${doubtful}`);
      setOutcome({ kind: 'written', done });
      setPending([]);
      return;
    }
    AccessibilityInfo.announceForAccessibility(
      `${t('dialogs.eventGroupCarry.done', { count: done })}${earlier}`,
    );
    navigation.goBack();
  }, [
    busy,
    calendars,
    targets,
    pending,
    outcome,
    createdRows,
    doubts,
    successor,
    group.id,
    plan.changed,
    before,
    after,
    scope,
    occurrence,
    calendarName,
    t,
    i18n.language,
    navigation,
  ]);
  // The question's yes carries again through the carry of the latest render.
  carryRef.current = carry;

  // Every outcome puts the cursor on the outcome line, which reads what the
  // pass has to say, as the desktop focuses its outcome note. The outcome
  // 'written' also removes the button that held the cursor: without a new
  // place it was stranded, and the screen reader jumped to the top. While
  // the question about lost occurrences is open, it has the cursor; the line
  // takes it once the question is answered with Cancel. A yes starts a pass,
  // whose own outcome takes it when that pass ends.
  useEffect(() => {
    if (!outcome || asking || busy) return;
    requestAnimationFrame(() => {
      const tag = findNodeHandle(outcomeRef.current);
      if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
    });
  }, [outcome, asking, busy]);

  return (
    <FormScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      accessibilityViewIsModal
    >
      <Text style={styles.heading} accessibilityRole="header">
        {t('dialogs.eventGroupCarry.title')}
      </Text>

      {/* Not a live region: every outcome is announced as it is set, doubts
          included, and on Android a live region's own reading of just this
          line cut that announcement off. */}
      <Text
        ref={outcomeRef}
        style={outcome ? styles.warning : styles.intro}
        accessibilityRole="text"
        accessibilityLabel={outcome && outcomeSaid !== '' ? outcomeSaid : undefined}
      >
        {outcome
          ? outcome.kind === 'regroup'
            ? t('dialogs.eventGroupCarry.regroupFailed', { count: outcome.done })
            : outcome.kind === 'written'
              ? t('dialogs.eventGroupCarry.done', { count: outcome.done })
              : t('dialogs.eventGroupCarry.partly', {
                  done: outcome.done,
                  failed: outcome.failed
                    .map((target) => calendarName(target.calendar_id))
                    .join(', '),
                })
          : calendars == null
            ? t('dialogs.eventGroup.loading')
            : t('dialogs.eventGroupCarry.message', {
                count: plan.targets.length,
                fields: plan.changed
                  .map((field) => t(`dialogs.eventGroupCarry.field.${field}`))
                  .join(', '),
              })}
      </Text>

      {/* Which occurrences this is about. Without it the question reads the
          same whether it will touch one appointment or a hundred. */}
      {!outcome && scope !== 'series' && (
        <Text style={styles.member} accessibilityRole="text">
          {t(
            scope === 'future'
              ? 'dialogs.eventGroupCarry.scopeFuture'
              : 'dialogs.eventGroupCarry.scopeOccurrence',
          )}
        </Text>
      )}

      {/* Named by their calendar. The title is the same on every copy — that
          is what made them a group — and the stored one is the title the copy
          had when it JOINED, which an edit since then has already outdated. */}
      {calendars != null &&
        targets.map((target) => (
          <Text
            key={`${target.calendar_id} ${target.event_id}`}
            style={styles.member}
            accessibilityRole="text"
          >
            {t('dialogs.eventGroupCarry.target', {
              calendar: calendarName(target.calendar_id),
            })}
          </Text>
        ))}

      {/* The copies it may not write — said BEFORE the user decides, because
          "carry to all" that silently means "to some" is the contradiction
          this feature exists to prevent. */}
      {plan.skipped.map((target) => (
        <Text
          key={`skip ${target.calendar_id} ${target.event_id}`}
          style={styles.warning}
          accessibilityRole="text"
        >
          {t('dialogs.eventGroupCarry.skipped', {
            calendar: calendarName(target.calendar_id),
          })}
        </Text>
      ))}

      {/* The copies written whose series may show twice, one line each.
          Mirrors the desktop. */}
      {doubts.map((doubt) => (
        <Text key={doubt} style={styles.warning} accessibilityRole="text">
          {doubt}
        </Text>
      ))}

      {/* Not a live region: the outcome line reads it with the outcome. */}
      {error != null && (
        <Text style={styles.error} accessibilityRole="text">
          {error}
        </Text>
      )}

      {/* Nothing is left to try once no copy is outstanding. */}
      {outcome?.kind !== 'written' && (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t(
            outcome ? 'dialogs.eventGroupCarry.retry' : 'dialogs.eventGroupCarry.carry',
          )}
          accessibilityState={{ disabled: busy || !canRetry }}
          onPress={() => void carry()}
          style={styles.action}
        >
          <Text
            style={[styles.actionText, (busy || !canRetry) && styles.disabled]}
          >
            {t(
              outcome ? 'dialogs.eventGroupCarry.retry' : 'dialogs.eventGroupCarry.carry',
            )}
          </Text>
        </Pressable>
      )}

      <Pressable
        accessibilityRole="button"
        accessibilityLabel={t(
          outcome ? 'dialogs.eventGroupCarry.dismiss' : 'dialogs.eventGroupCarry.keep',
        )}
        onPress={() => navigation.goBack()}
        style={styles.action}
      >
        <Text style={styles.actionText}>
          {t(
            outcome ? 'dialogs.eventGroupCarry.dismiss' : 'dialogs.eventGroupCarry.keep',
          )}
        </Text>
      </Pressable>
    </FormScrollView>
  );
}

/** One copy, as a key: its calendar and its id. */
function targetKey(target: CarryTarget): string {
  return `${target.calendar_id}\u0000${target.event_id}`;
}

function sameTarget(a: CarryTarget, b: CarryTarget): boolean {
  return a.calendar_id === b.calendar_id && a.event_id === b.event_id;
}

function makeStyles(c: ThemeColors) {
  return StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16, gap: 12 },
    heading: { fontSize: 20, fontWeight: '600', color: c.textPrimary },
    intro: { fontSize: 15, color: c.textPrimary },
    member: { fontSize: 15, color: c.textSecondary },
    warning: { fontSize: 15, color: c.warning },
    error: { fontSize: 15, color: c.danger },
    action: {
      minHeight: 44,
      justifyContent: 'center',
      paddingHorizontal: 12,
      borderRadius: 8,
      backgroundColor: c.surfaceAlt,
    },
    actionText: { fontSize: 16, color: c.textPrimary },
    disabled: { color: c.textSecondary },
  });
}
