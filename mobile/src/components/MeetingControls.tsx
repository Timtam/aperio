import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AccessibilityInfo, Pressable, StyleSheet, Text, View } from 'react-native';

import { listAccounts, type Account } from '../api/accounts';
import type { CalendarEvent } from '../api/calendar';
import {
  adoptMeeting,
  attachMeeting,
  detachMeeting,
  inspectEventMeeting,
  type EventMeetingInspection,
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
  const [found, setFound] = useState<EventMeetingInspection | null>(null);
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
  const calendarId = event?.calendar_id ?? null;
  useEffect(() => {
    if (!eventId || !calendarId) {
      setFound(null);
      return;
    }
    let cancelled = false;
    inspectEventMeeting({ event_id: eventId, calendar_id: calendarId })
      .then((result) => {
        if (!cancelled) setFound(result);
      })
      .catch(() => {
        // Treated as "nothing known", which offers Create — the same thing the
        // editor did before any of this existed.
      });
    return () => {
      cancelled = true;
    };
  }, [eventId, calendarId]);

  const announce = (message: string) =>
    AccessibilityInfo.announceForAccessibility(message);

  const create = useCallback(
    async (usePersonalRoom: boolean) => {
    const account = accounts[0];
    if (!event || !account) return;
    setBusy(true);
    setError(null);
    try {
      const attached = await attachMeeting({
        event_id: event.id,
        calendar_id: event.calendar_id,
        account_id: account.id,
        use_personal_room: usePersonalRoom,
      });
      setFound({
        binding: {
          event_id: event.id,
          account_id: account.id,
          meeting_id: attached.meeting.id,
          join_url: attached.meeting.join_url,
          created_at: new Date().toISOString(),
        },
        meeting: attached.meeting,
        account_id: account.id,
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
    },
    [accounts, event, onEventChanged, t],
  );

  const remove = useCallback(async () => {
    if (!event) return;
    setBusy(true);
    setError(null);
    try {
      const saved = await detachMeeting({
        event_id: event.id,
        calendar_id: event.calendar_id,
      });
      setFound(null);
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

  /** Take over a meeting that is on the event but not yet ours. */
  const adopt = useCallback(async () => {
    if (!event || !found?.meeting || !found.account_id) return;
    setBusy(true);
    setError(null);
    try {
      const binding = await adoptMeeting({
        event_id: event.id,
        account_id: found.account_id,
        meeting_id: found.meeting.id,
        join_url: found.meeting.join_url,
      });
      setFound({ ...found, binding });
      announce(t('conferencing.meetingAdopted'));
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      announce(t('conferencing.meetingFailed', { message }));
    } finally {
      setBusy(false);
    }
  }, [event, found, t]);

  // No videoconference account, nothing to offer — and a disabled button
  // explaining an absence teaches nothing that Settings does not.
  if (accounts.length === 0) return null;

  if (!event) {
    return <Text style={styles.hint}>{t('conferencing.saveEventFirst')}</Text>;
  }

  // Three states, and the middle one is the point: an event that ALREADY has a
  // meeting must not be offered "create", which would mint a second one and
  // write its link in alongside the first.
  const owned = found?.binding != null;
  const adoptable = !owned && found?.meeting != null;
  const label = owned
    ? t('conferencing.removeMeeting')
    : adoptable
      ? t('conferencing.adoptMeeting')
      : t('conferencing.createMeeting');
  const note = owned
    ? t('conferencing.meetingOwned')
    : adoptable
      ? t('conferencing.meetingNotOwned')
      : null;
  const invitees = found?.meeting?.invitees ?? [];

  return (
    <View style={styles.group}>
      {/* Who the PROVIDER says is invited, kept apart from the event's own
          attendee list: an event auto-created from an invitation mail often
          lists only the recipient and the provider's sending address. */}
      {invitees.length > 0 && (
        <>
          <Text style={styles.label}>{t('conferencing.meetingInvitees')}</Text>
          {invitees.map((invitee) => {
            const line = invitee.co_host
              ? t('conferencing.inviteeCoHost', {
                  name: invitee.display_name ?? invitee.email,
                  email: invitee.email,
                })
              : t('conferencing.invitee', {
                  name: invitee.display_name ?? invitee.email,
                  email: invitee.email,
                });
            return (
              <Text key={invitee.email} style={styles.hint} accessibilityLabel={line}>
                {line}
              </Text>
            );
          })}
        </>
      )}
      {note != null && <Text style={styles.hint}>{note}</Text>}
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={label}
        accessibilityState={{ disabled: busy }}
        onPress={() =>
          void (owned ? remove() : adoptable ? adopt() : create(false))
        }
        disabled={busy}
        style={({ pressed }) => [styles.button, pressed && styles.pressed]}
      >
        <Text style={styles.buttonText}>{label}</Text>
      </Pressable>
      {/* The second kind of meeting, named rather than hidden behind a dialog:
          one more stop instead of four, and each says what it does. */}
      {!owned && !adoptable && (
        <>
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t('conferencing.usePersonalRoom')}
            accessibilityState={{ disabled: busy }}
            onPress={() => void create(true)}
            disabled={busy}
            style={({ pressed }) => [styles.button, pressed && styles.pressed]}
          >
            <Text style={styles.buttonText}>
              {t('conferencing.usePersonalRoom')}
            </Text>
          </Pressable>
          <Text style={styles.hint}>{t('conferencing.personalRoomHint')}</Text>
        </>
      )}
      {error != null && <Text style={styles.error}>{error}</Text>}
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    group: { gap: 8 },
    label: { fontSize: 15, fontWeight: '600', color: c.textPrimary },
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
