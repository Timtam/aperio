import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { writeErrorMessage,
  eventWriteFailureReason,
  writeNeverLanded,
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
  endsByDateOf,
  TailShiftRefusedError,
  cutoffDay,
  readSeriesRows,
  seriesMaybeShownTwice,
  seriesTailMayStand,
  createsInSteps,
  writeSeriesSplit,
  textFields,
  zoneCityName,
  zoneOffer,
  zoneRefusalText,
  zoneVerdictFor,
  type CarryableFields,
  type CarryField,
  type ZoneSeriesRefused,
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
    zoneMove = null,
    zoneVerdicts = null,
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
  /** What a pass said about copies that did not get the zone (146a), kept
   *  across a retry: such a copy is not offered again, so this is where it is
   *  still named. Mirrors the desktop. */
  const [zoneNotes, setZoneNotes] = useState<string[]>([]);
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

  // A series' zone carried to its copies (stage 13): the offer from each
  // copy's verdict as read before the screen opened. Mirrors the desktop.
  const zoneTo = zoneMove?.anchorAfter.tzid ?? null;
  const offer = useMemo(
    () =>
      calendars != null &&
      scope === 'series' &&
      zoneMove &&
      zoneVerdicts &&
      plan.changed.includes('zone')
        ? zoneOffer(plan, new Map(Object.entries(zoneVerdicts)))
        : null,
    [calendars, scope, zoneMove, zoneVerdicts, plan],
  );
  /** A changed field as the message names it: the zone by the city of the
   *  series' new zone, first (143a). */
  const fieldName = useCallback(
    (field: CarryField) =>
      field === 'zone'
        ? t('dialogs.eventGroupCarry.field.zone', { zone: zoneCityName(zoneTo) })
        : t(`dialogs.eventGroupCarry.field.${field}`),
    [t, zoneTo],
  );
  /** Why each copy that does not get the zone does not, as the offer says it
   *  before the carry: the series refused with their reason, then the copies
   *  that are no series there. */
  const zoneReasons = useMemo(
    () =>
      offer
        ? [
            ...offer.refused.map(({ target, verdict }) =>
              t('dialogs.eventGroupCarry.zoneRefused', {
                calendar: calendarName(target.calendar_id),
                reason: zoneRefusalText(verdict, zoneTo, t),
              }),
            ),
            // Where the save also changed the series' times, the sentence says
            // the copy does not get them: a single event elsewhere keeps its
            // own (146a).
            ...offer.notASeries.map((target) =>
              t(
                plan.changed.some((field) => TIME_FIELDS.includes(field))
                  ? 'dialogs.eventGroupCarry.zoneNotASeriesTimes'
                  : 'dialogs.eventGroupCarry.zoneNotASeries',
                { calendar: calendarName(target.calendar_id) },
              ),
            ),
          ]
        : [],
    [offer, plan.changed, t, calendarName, zoneTo],
  );
  /** The offer's message: the question, then how far the copies move and how
   *  many do not get the zone. */
  const offerMessage = useMemo(() => {
    const question = t('dialogs.eventGroupCarry.message', {
      count: offer ? offer.targets.length : plan.targets.length,
      fields: plan.changed.map(fieldName).join(', '),
    });
    if (!offer) return question;
    const days = offer.days.map(({ days: moved, count }) =>
      t(
        moved < 0
          ? 'dialogs.eventGroupCarry.zoneDaysEarlier'
          : 'dialogs.eventGroupCarry.zoneDaysLater',
        {
          count,
          days: t('dialogs.eventGroupCarry.dayCount', { count: Math.abs(moved) }),
        },
      ),
    );
    const refusedCount = offer.refused.length + plan.skipped.length;
    const refused =
      refusedCount > 0
        ? [t('dialogs.eventGroupCarry.zoneRefusedCount', { count: refusedCount })]
        : [];
    return [question, ...days, ...refused].join(' ');
  }, [offer, plan, fieldName, t]);

  // After a partial carry, only what is still outstanding.
  const targets = pending ?? offer?.targets ?? plan.targets;
  /** No copy can take the change (stage 13): the screen can only close. */
  const nothingToCarry = !outcome && offer !== null && offer.nothingToCarry;
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
    // Copies that did not get the zone, with their reason (146a): written
    // without it, or not changed. Named in the report, never retried.
    const zoneLines: string[] = [];
    // The day this pass asks each copy on, for the rule groups (120a).
    const today = new Date().toISOString();
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
        // Whether its standalone went out: one whose answer was lost may
        // stand. Mirrors the desktop.
        let standaloneSent = false;
        // Whether its new series went out, and whether its create answered:
        // one whose answer was lost may stand (review round 18 of #128).
        let tailSent = false;
        let tailMade = false;
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
            standaloneSent = true;
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
                    endsByDate: endsByDateOf(calendars?.find((c) => c.id === current.calendar_id)),
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
                endsByDate: endsByDateOf(calendars?.find((c) => c.id === current.calendar_id)),
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
                    createTail: async () => {
                      tailSent = true;
                      const made = await createEvent(
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
                      );
                      tailMade = true;
                      return made;
                    },
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
          } else if (offer && zoneMove) {
            // The series' zone, carried (stage 13): the copy is asked again as
            // it is now, and gets one write — or none, where it already stands
            // as the series does and nothing else changed. Mirrors the desktop.
            const verdict = zoneVerdictFor(
              zoneMove,
              current,
              calendars?.find((c) => c.id === target.calendar_id),
              today,
            );
            const texts = textFields(plan.changed);
            const copyRow = current as CalendarEvent & CarryableFields;
            if (verdict.outcome === 'changed') {
              // Its rule, start, end, deletions and zone as the core wrote them
              // on the new clock; its changed occurrences move with it where
              // its calendar keeps them by slot (95a).
              const write: CalendarEvent = {
                ...carryOnto(copyRow, after, texts),
                start: verdict.start,
                end: verdict.end,
                recurrence: {
                  ...(current.recurrence ?? { exceptions: [] }),
                  rrule: verdict.rrule,
                  exceptions: verdict.exceptions,
                  tzid: verdict.tzid,
                },
                ...(verdict.shiftMinutes !== 0
                  ? { occurrence_shift_minutes: verdict.shiftMinutes }
                  : {}),
                send_invitations: false,
              };
              askable = true;
              const savedSeries = await updateEvent(
                agreed ? { ...write, accepts_exception_loss: true } : write,
                target.calendar_id,
              );
              askable = false;
              noteBack(savedSeries, target.calendar_id);
            } else if (verdict.outcome === 'unchanged') {
              // Already as the series stands: only its text, if that changed.
              // On a retry the copy was outstanding: its earlier write landed
              // after all, so it counts as written.
              if (texts.length === 0) {
                if (pending !== null) done += 1;
                continue;
              }
              noteBack(
                await updateEvent(carryOnto(copyRow, after, texts), target.calendar_id),
                target.calendar_id,
              );
            } else if (verdict.reason === 'not_a_series') {
              // No series there: its text only (146a). The series' start and
              // end are instants of the series; a single event elsewhere
              // would land on the series' first day, or end before it starts.
              if (texts.length === 0) continue;
              noteBack(
                await updateEvent(carryOnto(copyRow, after, texts), target.calendar_id),
                target.calendar_id,
              );
            } else {
              // A series that cannot take the zone keeps its times and zone,
              // and still gets its text (146a); either way it is named.
              const reason = zoneRefusalText(verdict as ZoneSeriesRefused, zoneTo, t);
              const calendar = calendarName(target.calendar_id);
              if (texts.length === 0) {
                zoneLines.push(
                  t('dialogs.eventGroupCarry.notChangedReason', { calendar, reason }),
                );
                continue;
              }
              noteBack(
                await updateEvent(carryOnto(copyRow, after, texts), target.calendar_id),
                target.calendar_id,
              );
              zoneLines.push(
                t('dialogs.eventGroupCarry.changedWithoutZone', { calendar, reason }),
              );
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
                      endsByDate: endsByDateOf(calendars?.find((c) => c.id === current.calendar_id)),
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
          if (standaloneSent && occurrence && !writeNeverLanded(err)) {
            // Its standalone may stand, its answer lost, say: said, and not
            // offered again, which would make a second one. Mirrors the
            // desktop.
            unsure.push(
              t('dialogs.eventGroupCarry.standaloneMayStand', {
                calendar: calendarName(target.calendar_id),
                date: cutoffDay(occurrence, i18n.language),
                reason: eventWriteFailureReason(err, t),
              }),
            );
            continue;
          }
          if (tailSent && !tailMade && splitAt != null && !writeNeverLanded(err)) {
            // Its new series may stand, its answer lost, say: said, and not
            // offered again, which would write it a second time.
            unsure.push(
              t('dialogs.eventGroupCarry.tailMayStand', {
                calendar: calendarName(target.calendar_id),
                date: cutoffDay(splitAt, i18n.language),
                reason: eventWriteFailureReason(err, t),
              }),
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
      // So is a copy that did not get the zone (146a).
      const spoken = [...zoneLines, ...unsure];
      if (spoken.length > 0) AccessibilityInfo.announceForAccessibility(spoken.join(' '));
      return;
    }
    setCreatedRows(created);
    const allDoubts = [...doubts, ...unsure];
    setDoubts(allDoubts);
    const allZoneNotes = [...zoneNotes, ...zoneLines];
    setZoneNotes(allZoneNotes);
    // Said once, with whatever else this pass reports, by the outcome line that
    // takes the cursor: a second utterance in the same frame would cut the
    // first off, and a queued announcement is queued on iOS only. The copies
    // that did not get the zone come first, as they do on the desktop.
    const doubtful = `${zoneLines.length === 0 ? '' : ` ${zoneLines.join(' ')}`}${
      unsure.length === 0 ? '' : ` ${unsure.join(' ')}`
    }`;
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
    if (allDoubts.length > 0 || allZoneNotes.length > 0) {
      // No copy is left to offer again, but some may show twice, or did not
      // get the zone: leaving would take the only words that say which ones.
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
    zoneNotes,
    offer,
    zoneMove,
    zoneTo,
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
            : nothingToCarry
              ? // No copy can take the change: the screen can only close, and
                // says why in one line, each copy's reason included.
                [
                  t('dialogs.eventGroupCarry.nothingToCarry'),
                  ...zoneReasons,
                  ...plan.skipped.map((target) =>
                    t('dialogs.eventGroupCarry.skipped', {
                      calendar: calendarName(target.calendar_id),
                    }),
                  ),
                ].join(' ')
              : offerMessage}
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
        !nothingToCarry &&
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
      {/* The copies that will not get the zone, each with its reason (146a),
          before the user decides. Mirrors the desktop. */}
      {!outcome &&
        !nothingToCarry &&
        zoneReasons.map((reason) => (
          <Text key={reason} style={styles.warning} accessibilityRole="text">
            {reason}
          </Text>
        ))}

      {!nothingToCarry &&
        plan.skipped.map((target) => (
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

      {/* The copies that did not get the zone, as the passes said them: never
          offered again, so named here. Mirrors the desktop. */}
      {zoneNotes.map((note) => (
        <Text key={note} style={styles.warning} accessibilityRole="text">
          {note}
        </Text>
      ))}

      {/* Not a live region: the outcome line reads it with the outcome. */}
      {error != null && (
        <Text style={styles.error} accessibilityRole="text">
          {error}
        </Text>
      )}

      {/* Nothing is left to try once no copy is outstanding, and nothing to
          carry where no copy can take the change. */}
      {outcome?.kind !== 'written' && !nothingToCarry && (
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
          outcome || nothingToCarry
            ? 'dialogs.eventGroupCarry.dismiss'
            : 'dialogs.eventGroupCarry.keep',
        )}
        onPress={() => navigation.goBack()}
        style={styles.action}
      >
        <Text style={styles.actionText}>
          {t(
            outcome || nothingToCarry
              ? 'dialogs.eventGroupCarry.dismiss'
              : 'dialogs.eventGroupCarry.keep',
          )}
        </Text>
      </Pressable>
    </FormScrollView>
  );
}

/** The changed fields that are the series' times: a copy that is no series
 *  there does not get them. Mirrors the desktop. */
const TIME_FIELDS: readonly CarryField[] = ['start', 'end', 'all_day'];

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
