import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  findNodeHandle,
  Platform,
  Pressable,
  StyleSheet,
  Text,
} from 'react-native';

import {
  eventGroupMemberKey,
  expandAll,
  memberFromEvent,
  seriesIdOf,
  suggestGroupMate,
  withoutDuplicateMeetings,
  type EventGroup,
} from '@aperio/shared';

import { getEvents, listCalendars, type Calendar, type CalendarEvent } from '../api/calendar';
import {
  dissolveEventGroup,
  eventGroupsForEvents,
  groupEvents,
  ungroupEvent,
} from '../api/eventGroups';
import { FormScrollView } from '../components/FormScrollView';
import { RadioGroup } from '../components/RadioGroup';
import { useCancelHeader } from '../components/useCancelHeader';
import type { RootStackScreenProps } from '../navigation/types';
import { useCalendarVisibility } from '../state/calendarVisibility';
import { useThemedStyles, type ThemeColors } from '../theme';

// "These events mean the same appointment" (DESIGN-event-groups.md) — the RN
// twin of the desktop EventGroupDialog.
//
// ONE event at a time, deliberately: the anchor rode in from the row that was
// long-pressed, and the second one is NAMED from the other events of that day,
// which is where a duplicate of an appointment lives by definition. A
// multi-select across calendars would be a pointing gesture with no rotor
// equivalent worth the name.
//
// Nothing here reaches a provider: grouping two events changes neither of
// them, and ungrouping leaves both exactly as they were.

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** The day the anchor event starts, as a local midnight-to-midnight range. */
function dayRangeOf(event: CalendarEvent): { start: string; end: string } {
  const at = new Date(event.start);
  const from = new Date(at.getFullYear(), at.getMonth(), at.getDate(), 0, 0, 0, 0);
  const to = new Date(from);
  to.setDate(to.getDate() + 1);
  return { start: from.toISOString(), end: to.toISOString() };
}

export default function EventGroupModal({
  route,
  navigation,
}: RootStackScreenProps<'EventGroup'>) {
  const { event } = route.params;
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { hidden: hiddenCalendars } = useCalendarVisibility();
  useCancelHeader(navigation);

  const anchorId = seriesIdOf(event);
  const anchorKey = eventGroupMemberKey(event.calendar_id, anchorId);

  // `undefined` while the lookup is in flight — distinct from `null`, the
  // answer "not grouped". Without the distinction the screen would claim the
  // event is ungrouped for the first frame of every open.
  const [group, setGroup] = useState<EventGroup | null | undefined>(undefined);
  const [dayEvents, setDayEvents] = useState<CalendarEvent[]>([]);
  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [picked, setPicked] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  /**
   * Land focus back on the heading after an action removes the control the
   * user is standing on.
   *
   * Taking the last-but-one member out dissolves the group, so "Take this
   * event out" and "Dissolve group" both UNMOUNT the moment they succeed —
   * with VoiceOver's cursor on them. Without a repark the cursor lands
   * wherever the platform decides, which on a screen the user is still in is
   * indistinguishable from having been thrown out of it. The heading is the
   * one node that is always there, and hearing it again says plainly which
   * screen this still is.
   */
  const headingRef = useRef<Text>(null);
  const reparkFocus = useCallback(() => {
    const tag = findNodeHandle(headingRef.current);
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, []);

  const calendarName = useCallback(
    (id: string) => calendars.find((c) => c.id === id)?.name ?? id,
    [calendars],
  );

  /**
   * What to call a member.
   *
   * The stored `title` is the SIGNATURE — what the event was called when it
   * joined, kept so a member whose provider id changed can be found again. It
   * is explicitly not for display: after a rename it is simply wrong. So the
   * day's loaded events answer first, and the signature is the fallback for a
   * member outside that range, where a stale name still beats no name.
   */
  const memberTitle = useCallback(
    (m: { calendar_id: string; event_id: string; title: string }) => {
      const key = eventGroupMemberKey(m.calendar_id, m.event_id);
      const live = dayEvents.find(
        (ev) => eventGroupMemberKey(ev.calendar_id, seriesIdOf(ev)) === key,
      );
      return live?.title ?? m.title;
    },
    [dayEvents],
  );

  const loadGroup = useCallback(async () => {
    try {
      const groups = await eventGroupsForEvents([
        { calendar_id: event.calendar_id, event_id: anchorId },
      ]);
      setGroup(groups[0] ?? null);
    } catch {
      // A failed lookup reads as "not grouped" rather than as a blank screen:
      // the picker below still works, and grouping is idempotent.
      setGroup(null);
    }
  }, [event.calendar_id, anchorId]);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      await loadGroup();
      try {
        const range = dayRangeOf(event);
        const cals = await listCalendars();
        if (cancelled) return;
        setCalendars(cals);
        // Only the calendars the user has switched ON, matching the desktop
        // dialog (which reads the selected set) and the move/copy pickers on
        // both platforms: a picker offers what the user has chosen to see.
        // The anchor's own calendar is always asked, even when hidden — it is
        // where the user just came from.
        const perCalendar = await Promise.all(
          cals
            .filter((c) => c.id === event.calendar_id || !hiddenCalendars.has(c.id))
            .map((c) =>
              getEvents({ calendar_id: c.id, ...range }).catch(() => [] as CalendarEvent[]),
            ),
        );
        // Expanded, like every other calendar surface: the adapters hand back
        // a recurring SERIES as its master row, so an unexpanded list offers
        // series that have no occurrence on this day and hides the ones whose
        // master lies outside it. `expandAll` is the same helper the day list
        // itself uses, so the picker offers exactly what the view showed.
        if (!cancelled) {
          setDayEvents(
            // `withoutDuplicateMeetings` for the same reason every view runs
            // it: a videoconference account contributes a read-only calendar
            // of its own meetings, and the ones that already have a calendar
            // entry are dropped there. Offering them here would invite the
            // user to group an event with a row that is not shown anywhere —
            // and the group would then name something they cannot see.
            withoutDuplicateMeetings(
              expandAll(perCalendar.flat(), {
                start: new Date(range.start),
                end: new Date(range.end),
              }),
            ),
          );
        }
      } catch (err) {
        if (!cancelled) setError(errorMessage(err));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [event, loadGroup, hiddenCalendars]);

  /**
   * The events that can be named as "the same appointment".
   *
   * By SERIES, not by occurrence: a recurring event shows up once per day in
   * the range, and offering the same series three times would be three ways to
   * say one thing. Members of the anchor's own group drop out — they already
   * say it.
   */
  const candidates = useMemo(() => {
    const seen = new Set<string>([anchorKey]);
    for (const m of group?.members ?? []) {
      seen.add(eventGroupMemberKey(m.calendar_id, m.event_id));
    }
    const out: CalendarEvent[] = [];
    for (const ev of dayEvents) {
      const key = eventGroupMemberKey(ev.calendar_id, seriesIdOf(ev));
      if (seen.has(key)) continue;
      seen.add(key);
      out.push(ev);
    }
    return out;
  }, [dayEvents, group, anchorKey]);

  /**
   * The candidate that looks like a copy of this event — same name, same
   * start, another calendar (`suggestGroupMate`).
   *
   * Offered, never applied: it arrives preselected with a line saying why, so
   * confirming is one tap and disagreeing is just choosing something else.
   */
  const suggested = useMemo(
    () => suggestGroupMate({ ...event, id: anchorId }, candidates),
    [event, anchorId, candidates],
  );
  // Applied ONCE. A ref rather than reading `picked` in the effect: the point
  // is "did we already offer this", which is not the same question as "is
  // something chosen right now" — a user who deliberately clears the choice
  // must not have the suggestion pushed back at them.
  const suggestionOffered = useRef(false);
  useEffect(() => {
    if (suggestionOffered.current || suggested == null) return;
    suggestionOffered.current = true;
    setPicked(eventGroupMemberKey(suggested.calendar_id, seriesIdOf(suggested)));
  }, [suggested]);

  const options = useMemo(
    () =>
      candidates.map((ev) => ({
        value: eventGroupMemberKey(ev.calendar_id, seriesIdOf(ev)),
        label: t('dialogs.eventGroup.candidate', {
          title: ev.title,
          // `i18n.language`, not the device locale: Aperio's language is a
          // setting of its own, and a German app that reads out 8:00 AM in an
          // otherwise German sentence is jarring — the rest of the app already
          // formats this way.
          time: ev.all_day
            ? t('dialogs.eventGroup.allDay')
            : new Date(ev.start).toLocaleTimeString(i18n.language, {
                hour: '2-digit',
                minute: '2-digit',
              }),
          calendar: calendarName(ev.calendar_id),
        }),
      })),
    [candidates, calendarName, t, i18n.language],
  );

  const fail = useCallback(
    (err: unknown) => {
      // The one refusal a user can actually meet: both events are already
      // grouped, with different partners. Only they can decide what that
      // should become, so it is said plainly.
      //
      // Recognised by the CODE the native module attaches, not by words in the
      // message — the message is the Rust `Display` text, in English, and a
      // string match on it would silently stop working the day that sentence
      // is reworded (it never matched to begin with).
      const message =
        (err as { code?: string })?.code === 'event_group_conflict'
          ? t('dialogs.eventGroup.conflict')
          : errorMessage(err);
      setError(message);
      // The live region below is ANDROID ONLY — `accessibilityLiveRegion` maps
      // to the Android platform attribute and does nothing on iOS. Removing
      // this announce to avoid a double utterance therefore left VoiceOver
      // with no channel at all: the one refusal a user has to act on ("take
      // one of them out first") was set into a Text near the top of the screen
      // and never spoken, while the cursor sat on a button that sounded
      // exactly as it had before the tap. So: announce where nothing else
      // will, stay quiet where the live region already does.
      if (Platform.OS === 'ios') AccessibilityInfo.announceForAccessibility(message);
    },
    [t],
  );

  const addPicked = useCallback(async () => {
    if (busy || picked === '') return;
    const other = candidates.find(
      (ev) => eventGroupMemberKey(ev.calendar_id, seriesIdOf(ev)) === picked,
    );
    if (!other) return;
    setBusy(true);
    setError(null);
    try {
      const next = await groupEvents([
        memberFromEvent({ ...event, id: anchorId }),
        memberFromEvent({ ...other, id: seriesIdOf(other) }),
      ]);
      setGroup(next);
      setPicked('');
      AccessibilityInfo.announceForAccessibility(
        t('dialogs.eventGroup.grouped', { title: event.title, other: other.title }),
      );
    } catch (err) {
      fail(err);
    } finally {
      setBusy(false);
    }
  }, [busy, picked, candidates, event, anchorId, t, fail]);

  const removeSelf = useCallback(async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await ungroupEvent(event.calendar_id, anchorId);
      // `null`, not the group that came back: the call returns what is LEFT of
      // the group, which — with three or more members — still exists without
      // this event in it. Storing that made the dialog go on claiming this
      // event was grouped, and go on offering to dissolve a group it had just
      // left. What this screen states is always about THIS event.
      setGroup(null);
      AccessibilityInfo.announceForAccessibility(
        t('dialogs.eventGroup.ungrouped', { title: event.title }),
      );
      // Both buttons unmount with the membership the user was standing on.
      reparkFocus();
    } catch (err) {
      fail(err);
    } finally {
      setBusy(false);
    }
  }, [busy, event, anchorId, t, fail, reparkFocus]);

  const dissolve = useCallback(async () => {
    if (busy || !group) return;
    setBusy(true);
    setError(null);
    try {
      await dissolveEventGroup(group.id);
      setGroup(null);
      AccessibilityInfo.announceForAccessibility(t('dialogs.eventGroup.dissolved'));
      reparkFocus();
    } catch (err) {
      fail(err);
    } finally {
      setBusy(false);
    }
  }, [busy, group, t, fail, reparkFocus]);

  const members = group?.members ?? [];
  const addBlocked = busy || picked === '';

  return (
    <FormScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      accessibilityViewIsModal
    >
      <Text ref={headingRef} style={styles.heading} accessibilityRole="header">
        {t('dialogs.eventGroup.title')}
      </Text>

      <Text style={styles.intro} accessibilityRole="text">
        {group === undefined
          ? t('dialogs.eventGroup.loading')
          : members.length > 0
            ? t('dialogs.eventGroup.inGroup', {
                title: event.title,
                count: members.length - 1,
              })
            : t('dialogs.eventGroup.notGrouped', { title: event.title })}
      </Text>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {members.map((m) => (
        <Text
          key={eventGroupMemberKey(m.calendar_id, m.event_id)}
          style={styles.member}
          accessibilityRole="text"
        >
          {t('dialogs.eventGroup.member', {
            title: memberTitle(m),
            calendar: calendarName(m.calendar_id),
          })}
        </Text>
      ))}

      {options.length > 0 ? (
        <>
          {suggested != null && (
            <Text style={styles.hint} accessibilityRole="text">
              {t('dialogs.eventGroup.suggestHint', { title: suggested.title })}
            </Text>
          )}
          <RadioGroup<string>
            label={t('dialogs.eventGroup.pickLabel')}
            value={picked}
            options={options}
            onChange={setPicked}
          />
        </>
      ) : (
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.eventGroup.noCandidates')}
        </Text>
      )}

      <Pressable
        accessibilityRole="button"
        accessibilityLabel={t('dialogs.eventGroup.add')}
        // `accessibilityState.disabled` rather than skipping the control: a
        // button that vanishes when unusable never tells anyone it exists.
        // The dimmed text says the same thing to everyone else — without it a
        // sighted user just taps a button that does nothing.
        accessibilityState={{ disabled: addBlocked }}
        onPress={() => void addPicked()}
        style={styles.action}
      >
        <Text style={[styles.actionText, addBlocked && styles.disabled]}>
          {t('dialogs.eventGroup.add')}
        </Text>
      </Pressable>

      {members.length > 0 && (
        <>
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t('dialogs.eventGroup.removeSelf')}
            accessibilityState={{ disabled: busy }}
            onPress={() => void removeSelf()}
            style={styles.action}
          >
            <Text style={[styles.actionText, busy && styles.disabled]}>
              {t('dialogs.eventGroup.removeSelf')}
            </Text>
          </Pressable>
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t('dialogs.eventGroup.dissolve')}
            accessibilityState={{ disabled: busy }}
            onPress={() => void dissolve()}
            style={styles.action}
          >
            <Text style={[styles.actionText, busy ? styles.disabled : styles.destructive]}>
              {t('dialogs.eventGroup.dissolve')}
            </Text>
          </Pressable>
        </>
      )}
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
    hint: { fontSize: 14, color: c.textSecondary },
    error: { fontSize: 15, color: c.danger },
    action: {
      minHeight: 44,
      justifyContent: 'center',
      paddingHorizontal: 12,
      borderRadius: 8,
      backgroundColor: c.surfaceAlt,
    },
    actionText: { fontSize: 16, color: c.textPrimary },
    destructive: { color: c.danger },
    disabled: { color: c.textSecondary },
  });
}
