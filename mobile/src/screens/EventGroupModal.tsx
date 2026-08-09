import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AccessibilityInfo, Pressable, StyleSheet, Text } from 'react-native';

import {
  eventGroupMemberKey,
  memberFromEvent,
  seriesIdOf,
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
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
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

  const calendarName = useCallback(
    (id: string) => calendars.find((c) => c.id === id)?.name ?? id,
    [calendars],
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
        const perCalendar = await Promise.all(
          cals.map((c) =>
            getEvents({ calendar_id: c.id, ...range }).catch(() => [] as CalendarEvent[]),
          ),
        );
        if (!cancelled) setDayEvents(perCalendar.flat());
      } catch (err) {
        if (!cancelled) setError(errorMessage(err));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [event, loadGroup]);

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

  const options = useMemo(
    () =>
      candidates.map((ev) => ({
        value: eventGroupMemberKey(ev.calendar_id, seriesIdOf(ev)),
        label: t('dialogs.eventGroup.candidate', {
          title: ev.title,
          time: ev.all_day
            ? t('dialogs.eventGroup.allDay')
            : new Date(ev.start).toLocaleTimeString(undefined, {
                hour: '2-digit',
                minute: '2-digit',
              }),
          calendar: calendarName(ev.calendar_id),
        }),
      })),
    [candidates, calendarName, t],
  );

  const fail = useCallback(
    (err: unknown) => {
      // The one refusal a user can actually meet: both events are already
      // grouped, with different partners. Only they can decide what that
      // should become, so it is said plainly. A conflict is the ONLY conflict
      // this call raises, which is what lets the message be this specific.
      const message = /conflict|Gruppe/i.test(errorMessage(err))
        ? t('dialogs.eventGroup.conflict')
        : errorMessage(err);
      setError(message);
      AccessibilityInfo.announceForAccessibility(message);
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
      const next = await ungroupEvent(event.calendar_id, anchorId);
      setGroup(next);
      AccessibilityInfo.announceForAccessibility(
        t('dialogs.eventGroup.ungrouped', { title: event.title }),
      );
    } catch (err) {
      fail(err);
    } finally {
      setBusy(false);
    }
  }, [busy, event, anchorId, t, fail]);

  const dissolve = useCallback(async () => {
    if (busy || !group) return;
    setBusy(true);
    setError(null);
    try {
      await dissolveEventGroup(group.id);
      setGroup(null);
      AccessibilityInfo.announceForAccessibility(t('dialogs.eventGroup.dissolved'));
    } catch (err) {
      fail(err);
    } finally {
      setBusy(false);
    }
  }, [busy, group, t, fail]);

  const members = group?.members ?? [];

  return (
    <FormScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      accessibilityViewIsModal
    >
      <Text style={styles.heading} accessibilityRole="header">
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
            title: m.title,
            calendar: calendarName(m.calendar_id),
          })}
        </Text>
      ))}

      {options.length > 0 ? (
        <RadioGroup<string>
          label={t('dialogs.eventGroup.pickLabel')}
          value={picked}
          options={options}
          onChange={setPicked}
        />
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
        accessibilityState={{ disabled: busy || picked === '' }}
        onPress={() => void addPicked()}
        style={styles.action}
      >
        <Text style={styles.actionText}>{t('dialogs.eventGroup.add')}</Text>
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
            <Text style={styles.actionText}>{t('dialogs.eventGroup.removeSelf')}</Text>
          </Pressable>
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t('dialogs.eventGroup.dissolve')}
            accessibilityState={{ disabled: busy }}
            onPress={() => void dissolve()}
            style={styles.action}
          >
            <Text style={[styles.actionText, styles.destructive]}>
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
  });
}
