import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AccessibilityInfo, Pressable, StyleSheet, Text, View } from 'react-native';

import {
  findGroupSuggestions,
  memberFromEvent,
  seriesIdOf,
  type EventGroup,
  type SuggestionDecline,
} from '@aperio/shared';

import type { Calendar, CalendarEvent } from '../api/calendar';
import {
  declineGroupSuggestion,
  groupEvents,
  groupSuggestionDeclines,
} from '../api/eventGroups';
import { useThemedStyles, type ThemeColors } from '../theme';

// "These two look like the same appointment — are they?"
// (DESIGN-event-groups.md, Stufe 3) — the RN twin of the desktop notice.
//
// ONE row, above the day, and only when there is something to ask. Its size is
// the point: an offer that cannot be dismissed for good is a daily
// interruption, and with a screen reader it is one more thing to swipe past
// every morning before reaching the actual day.
//
// Both answers are final. Group makes the group; "not the same" is remembered
// (migration 0037) and the pair is never offered again on any device.

export function GroupSuggestionNotice({
  events,
  groups,
  calendars,
  onChanged,
}: {
  /** The day's events as the list renders them. Folded or not makes no
   *  difference: folding only removes copies already in a group, and those are
   *  exactly the ones no suggestion is made about. */
  events: readonly CalendarEvent[];
  groups: readonly EventGroup[];
  calendars: readonly Calendar[];
  onChanged: () => void;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  // `null` until the declines are known — NOT an empty list. Offering a pair
  // the user already refused is the one failure this must not have, so it
  // stays quiet until it knows, and stays quiet if the read fails.
  const [declines, setDeclines] = useState<SuggestionDecline[] | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void groupSuggestionDeclines()
      .then((rows) => {
        if (!cancelled) setDeclines(rows);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [events]);

  const suggestion = useMemo(
    () =>
      declines == null
        ? null
        : (findGroupSuggestions(events, groups, declines, seriesIdOf)[0] ?? null),
    [events, groups, declines],
  );

  const calendarName = useCallback(
    (id: string) => calendars.find((c) => c.id === id)?.name ?? id,
    [calendars],
  );

  const say = useCallback((message: string) => {
    // No live region here, so this is the only channel on BOTH platforms.
    AccessibilityInfo.announceForAccessibility(message);
  }, []);

  const accept = useCallback(async () => {
    if (busy || !suggestion) return;
    setBusy(true);
    try {
      await groupEvents([
        memberFromEvent({ ...suggestion.first, id: seriesIdOf(suggestion.first) }),
        memberFromEvent({ ...suggestion.second, id: seriesIdOf(suggestion.second) }),
      ]);
      say(t('views.groupSuggestion.grouped', { title: suggestion.first.title }));
      onChanged();
    } catch {
      say(t('views.groupSuggestion.failed'));
    } finally {
      setBusy(false);
    }
  }, [busy, suggestion, say, t, onChanged]);

  const refuse = useCallback(async () => {
    if (busy || !suggestion) return;
    setBusy(true);
    try {
      await declineGroupSuggestion(
        {
          calendar_id: suggestion.first.calendar_id,
          event_id: seriesIdOf(suggestion.first),
        },
        {
          calendar_id: suggestion.second.calendar_id,
          event_id: seriesIdOf(suggestion.second),
        },
      );
      say(t('views.groupSuggestion.declined'));
      onChanged();
    } catch {
      say(t('views.groupSuggestion.failed'));
    } finally {
      setBusy(false);
    }
  }, [busy, suggestion, say, t, onChanged]);

  if (!suggestion) return null;
  const { first, second } = suggestion;

  return (
    <View style={styles.notice} accessibilityRole="summary">
      <Text style={styles.message} accessibilityRole="text">
        {t('views.groupSuggestion.message', {
          title: first.title,
          first: calendarName(first.calendar_id),
          second: calendarName(second.calendar_id),
        })}
      </Text>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={t('views.groupSuggestion.accept')}
        accessibilityState={{ disabled: busy }}
        onPress={() => void accept()}
        style={styles.action}
      >
        <Text style={[styles.actionText, busy && styles.disabled]}>
          {t('views.groupSuggestion.accept')}
        </Text>
      </Pressable>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={t('views.groupSuggestion.refuse')}
        accessibilityState={{ disabled: busy }}
        onPress={() => void refuse()}
        style={styles.action}
      >
        <Text style={[styles.actionText, busy && styles.disabled]}>
          {t('views.groupSuggestion.refuse')}
        </Text>
      </Pressable>
    </View>
  );
}

function makeStyles(c: ThemeColors) {
  return StyleSheet.create({
    notice: {
      gap: 8,
      padding: 12,
      borderRadius: 8,
      backgroundColor: c.surfaceAlt,
      borderWidth: 1,
      borderColor: c.border,
    },
    message: { fontSize: 15, color: c.textPrimary },
    action: {
      minHeight: 44,
      justifyContent: 'center',
      paddingHorizontal: 12,
      borderRadius: 8,
      backgroundColor: c.surface,
    },
    actionText: { fontSize: 16, color: c.textPrimary },
    disabled: { color: c.textSecondary },
  });
}
