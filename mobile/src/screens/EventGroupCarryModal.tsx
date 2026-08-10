import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AccessibilityInfo, Platform, Pressable, StyleSheet, Text } from 'react-native';

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
} from '@aperio/shared';

import {
  addEventExdate,
  createEvent,
  getEventById,
  listCalendars,
  updateEvent,
  type Calendar,
  type CalendarEvent,
} from '../api/calendar';
import {
  groupEvents,
  ungroupEvent,
  type NewGroupMember,
} from '../api/eventGroups';
import { FormScrollView } from '../components/FormScrollView';
import { useCancelHeader } from '../components/useCancelHeader';
import type { RootStackScreenProps } from '../navigation/types';
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

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** One copy the carry may write, as `planCarry` describes it. */
type CarryTarget = ReturnType<typeof planCarry>['targets'][number];

/** How the last attempt ended, when it did not simply succeed. */
type CarryOutcome =
  /** Some copies could not be written. */
  | { kind: 'partly'; done: number; failed: CarryTarget[] }
  /** Every copy was written, but the new rows are not tied together yet. */
  | { kind: 'regroup'; done: number };

export default function EventGroupCarryModal({
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
  const { t } = useTranslation();
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

  const carry = useCallback(async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    const failed: CarryTarget[] = [];
    // The rows created so far, to be joined into a group of their own — the
    // earlier passes' included, so a retry ties the whole set together.
    const created: NewGroupMember[] = [...createdRows];
    let done = outcome?.done ?? 0;
    for (const target of targets) {
      // The user left. Stop writing copies into a screen that is gone.
      if (!alive.current) return;
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
          // differently is cut at ITS next one, which is what "and all
          // following" means over there.
          const anchorIso = firstOccurrenceFrom(
            current as CalendarEvent & CarryableFields,
            occurrence,
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
        const message = seriesLeftTruncated(err)
          ? t('dialogs.eventGroupCarry.truncatedNotRestored', {
              calendar: calendarName(target.calendar_id),
            })
          : errorMessage(err);
        if (!alive.current) return;
        setError(message);
        // `accessibilityLiveRegion` below is ANDROID ONLY, so on iOS this
        // announce is the only channel VoiceOver has.
        if (Platform.OS === 'ios') AccessibilityInfo.announceForAccessibility(message);
      }
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
        await groupEvents(members);
      } catch {
        regroupFailed = true;
      }
    }
    if (!alive.current) return;
    setCreatedRows(created);
    setBusy(false);
    // The whole point of the screen: say what actually happened, including
    // what did not.
    if (failed.length > 0) {
      // Named by CALENDAR, not by title: every copy of an appointment carries
      // the same title — that is what made them a group — so a list of titles
      // said nothing about which copy is now out of step.
      const names = failed.map((target) => calendarName(target.calendar_id));
      setOutcome({ kind: 'partly', done, failed });
      setPending(failed);
      AccessibilityInfo.announceForAccessibility(
        t('dialogs.eventGroupCarry.partly', { done, failed: names.join(', ') }),
      );
      // Stays open: a half-carried group is the state this feature exists to
      // prevent, so it has to be seen, and the rest can be retried.
      return;
    }
    if (regroupFailed) {
      // Every copy was written; only the new rows are not tied together yet.
      // Said, and left on screen, because "the appointment is one row again" is
      // what the user was promised.
      setOutcome({ kind: 'regroup', done });
      setPending([]);
      AccessibilityInfo.announceForAccessibility(
        t('dialogs.eventGroupCarry.regroupFailed', { count: done }),
      );
      return;
    }
    AccessibilityInfo.announceForAccessibility(
      t('dialogs.eventGroupCarry.done', { count: done }),
    );
    navigation.goBack();
  }, [
    busy,
    targets,
    outcome,
    createdRows,
    successor,
    plan.changed,
    before,
    after,
    scope,
    occurrence,
    calendarName,
    t,
    navigation,
  ]);

  return (
    <FormScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      accessibilityViewIsModal
    >
      <Text style={styles.heading} accessibilityRole="header">
        {t('dialogs.eventGroupCarry.title')}
      </Text>

      <Text
        style={outcome ? styles.warning : styles.intro}
        accessibilityRole="text"
        accessibilityLiveRegion={outcome ? 'assertive' : 'none'}
      >
        {outcome
          ? outcome.kind === 'regroup'
            ? t('dialogs.eventGroupCarry.regroupFailed', { count: outcome.done })
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

      {error != null && (
        <Text
          style={styles.error}
          accessibilityRole="text"
          accessibilityLiveRegion="assertive"
        >
          {error}
        </Text>
      )}

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
