import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AccessibilityInfo, Platform, Pressable, StyleSheet, Text } from 'react-native';

import {
  carryOnto,
  occurrenceCarryRow,
  planCarry,
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

export default function EventGroupCarryModal({
  route,
  navigation,
}: RootStackScreenProps<'EventGroupCarry'>) {
  const { group, anchor, before, after, scope = 'series', occurrence } = route.params;
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  useCancelHeader(navigation);

  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void listCalendars()
      .then((cals) => {
        if (!cancelled) setCalendars(cals);
      })
      .catch(() => {
        // Without the calendar list nothing is known to be writable, so the
        // plan below comes out empty and the screen says so rather than
        // writing somewhere it should not.
        if (!cancelled) setCalendars([]);
      });
    return () => {
      cancelled = true;
    };
  }, []);

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

  const carry = useCallback(async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    const failed: string[] = [];
    let done = 0;
    for (const target of plan.targets) {
      try {
        const current = await getEventById(target.event_id, target.calendar_id);
        if (current == null) {
          // The copy is gone from under us. Reported, not silently counted.
          failed.push(target.title);
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
        failed.push(target.title);
        const message = errorMessage(err);
        setError(message);
        // `accessibilityLiveRegion` below is ANDROID ONLY, so on iOS this
        // announce is the only channel VoiceOver has.
        if (Platform.OS === 'ios') AccessibilityInfo.announceForAccessibility(message);
      }
    }
    setBusy(false);
    // The whole point of the screen: say what actually happened, including
    // what did not.
    AccessibilityInfo.announceForAccessibility(
      failed.length > 0
        ? t('dialogs.eventGroupCarry.partly', { done, failed: failed.join(', ') })
        : t('dialogs.eventGroupCarry.done', { count: done }),
    );
    navigation.goBack();
  }, [busy, plan, after, scope, occurrence, t, navigation]);

  return (
    <FormScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      accessibilityViewIsModal
    >
      <Text style={styles.heading} accessibilityRole="header">
        {t('dialogs.eventGroupCarry.title')}
      </Text>

      <Text style={styles.intro} accessibilityRole="text">
        {t('dialogs.eventGroupCarry.message', {
          count: plan.targets.length,
          fields: plan.changed
            .map((field) => t(`dialogs.eventGroupCarry.field.${field}`))
            .join(', '),
        })}
      </Text>

      {plan.targets.map((target) => (
        <Text
          key={`${target.calendar_id} ${target.event_id}`}
          style={styles.member}
          accessibilityRole="text"
        >
          {t('dialogs.eventGroupCarry.target', {
            title: target.title,
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
            title: target.title,
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
        accessibilityLabel={t('dialogs.eventGroupCarry.carry')}
        accessibilityState={{ disabled: busy || plan.targets.length === 0 }}
        onPress={() => void carry()}
        style={styles.action}
      >
        <Text
          style={[
            styles.actionText,
            (busy || plan.targets.length === 0) && styles.disabled,
          ]}
        >
          {t('dialogs.eventGroupCarry.carry')}
        </Text>
      </Pressable>

      <Pressable
        accessibilityRole="button"
        accessibilityLabel={t('dialogs.eventGroupCarry.keep')}
        onPress={() => navigation.goBack()}
        style={styles.action}
      >
        <Text style={styles.actionText}>{t('dialogs.eventGroupCarry.keep')}</Text>
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
