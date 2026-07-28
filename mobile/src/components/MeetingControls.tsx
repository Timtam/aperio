import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AccessibilityInfo, Pressable, StyleSheet, Text, View } from 'react-native';

import { listAccounts, type Account } from '../api/accounts';
import type { CalendarEvent } from '../api/calendar';
import {
  attachMeeting,
  detachMeeting,
  eventMeeting,
  type EventMeetingBinding,
} from '../api/meetings';
import { useThemedStyles, type ThemeColors } from '../theme';

/**
 * Creating and removing the meeting for an event — the mobile twin of the
 * desktop `MeetingControls`.
 *
 * Distinct from `ConferenceSection`, which shows the meeting an event *has*,
 * from any tool and in any language. This is about the meeting Aperio *owns*:
 * one it created and recorded, and can therefore take back down. An event
 * carrying a colleague's link gets Join and no Remove, which is correct — it is
 * not ours to delete.
 */
export function MeetingControls({
  event,
  onEventChanged,
}: {
  /** The saved event, or `null` while it is still being composed. */
  event: CalendarEvent | null;
  onEventChanged: (event: CalendarEvent) => void;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [binding, setBinding] = useState<EventMeetingBinding | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Which accounts can mint a meeting comes from the account list itself, so a
  // videoconference adapter added later appears here without a change.
  useEffect(() => {
    let cancelled = false;
    listAccounts()
      .then((all) => {
        if (!cancelled) setAccounts(all.filter((acc) => acc.is_videoconference));
      })
      .catch(() => {
        // Nothing offered is the honest outcome of not knowing.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const eventId = event?.id ?? null;
  useEffect(() => {
    if (!eventId) {
      setBinding(null);
      return;
    }
    let cancelled = false;
    eventMeeting(eventId)
      .then((found) => {
        if (!cancelled) setBinding(found);
      })
      .catch(() => {
        // Treated as "no meeting of ours", which only hides the Remove button.
      });
    return () => {
      cancelled = true;
    };
  }, [eventId]);

  const announce = (message: string) =>
    AccessibilityInfo.announceForAccessibility(message);

  const create = useCallback(async () => {
    const account = accounts[0];
    if (!event || !account) return;
    setBusy(true);
    setError(null);
    try {
      const attached = await attachMeeting({
        event_id: event.id,
        calendar_id: event.calendar_id,
        account_id: account.id,
      });
      setBinding({
        event_id: event.id,
        account_id: account.id,
        meeting_id: attached.meeting.id,
        join_url: attached.meeting.join_url,
        created_at: new Date().toISOString(),
      });
      onEventChanged(attached.event);
      announce(t('conferencing.meetingCreated'));
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      announce(t('conferencing.meetingFailed', { message }));
    } finally {
      setBusy(false);
    }
  }, [accounts, event, onEventChanged, t]);

  const remove = useCallback(async () => {
    if (!event) return;
    setBusy(true);
    setError(null);
    try {
      const saved = await detachMeeting({
        event_id: event.id,
        calendar_id: event.calendar_id,
      });
      setBinding(null);
      if (saved) onEventChanged(saved);
      announce(t('conferencing.meetingRemoved'));
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      announce(t('conferencing.meetingFailed', { message }));
    } finally {
      setBusy(false);
    }
  }, [event, onEventChanged, t]);

  // No videoconference account, nothing to offer — and a disabled button
  // explaining an absence teaches nothing that Settings does not.
  if (accounts.length === 0) return null;

  if (!event) {
    return <Text style={styles.hint}>{t('conferencing.saveEventFirst')}</Text>;
  }

  const label = binding
    ? t('conferencing.removeMeeting')
    : t('conferencing.createMeeting');

  return (
    <View style={styles.group}>
      {binding && <Text style={styles.hint}>{t('conferencing.meetingOwned')}</Text>}
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={label}
        accessibilityState={{ disabled: busy }}
        onPress={() => void (binding ? remove() : create())}
        disabled={busy}
        style={({ pressed }) => [styles.button, pressed && styles.pressed]}
      >
        <Text style={styles.buttonText}>{label}</Text>
      </Pressable>
      {error != null && <Text style={styles.error}>{error}</Text>}
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    group: { gap: 8 },
    hint: { fontSize: 13, color: c.textSecondary },
    button: {
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
      alignItems: 'center',
    },
    pressed: { opacity: 0.7 },
    buttonText: { fontSize: 16, fontWeight: '600', color: c.textPrimary },
    error: { fontSize: 13, color: c.danger },
  });
