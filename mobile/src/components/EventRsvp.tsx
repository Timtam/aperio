import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AccessibilityInfo, Pressable, StyleSheet, Text, View } from 'react-native';

import {
  AttendeeStatus,
  CalendarEvent,
  respondToEvent,
} from '../api/calendar';
import { resolveCalendarUserEmail } from '../state/currentUserEmail';
import { useThemedStyles, type ThemeColors } from '../theme';

// RSVP affordance for an existing meeting (DESIGN §7.3) — the mobile twin of the
// desktop EventRsvp. Shown only when the event carries per-attendee response
// data (external, scheduling-capable providers):
//   - a non-organizer attendee sees Accept / Tentative / Decline buttons with
//     the current status marked selected;
//   - the organizer sees a read-only list of each attendee's status.
// "Who am I" comes from `calendarCurrentUserEmail`; when it's unknown (local /
// iCal, or a provider that can't report it) the component renders nothing.
// Screen-reader-first: every control is an addressable element with an explicit
// label, the current selection rides accessibilityState.selected, and a response
// announces its result.

/** The three respondable statuses, in render order. */
const RESPONSE_ACTIONS: AttendeeStatus[] = ['accepted', 'tentative', 'declined'];

/** Lower-case, `mailto:`-stripped form for comparing addresses. */
function normalizeEmail(value: string | null | undefined): string {
  if (!value) return '';
  return value.trim().replace(/^mailto:/i, '').toLowerCase();
}

export function EventRsvp({
  event,
  onResponded,
}: {
  event: CalendarEvent;
  /** Called after a successful response so the host can refresh + close. */
  onResponded: () => void;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const responses = event.attendee_responses ?? [];

  const [myEmail, setMyEmail] = useState<string | null>(null);
  const [pending, setPending] = useState<AttendeeStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    if (responses.length === 0) {
      setMyEmail(null);
      return;
    }
    resolveCalendarUserEmail(event.calendar_id)
      .then((email) => {
        if (!cancelled) setMyEmail(email);
      })
      .catch(() => {
        if (!cancelled) setMyEmail(null);
      });
    return () => {
      cancelled = true;
    };
  }, [event.calendar_id, responses.length]);

  if (responses.length === 0) return null;

  const me = normalizeEmail(myEmail);
  if (!me) return null;
  const isOrganizer = normalizeEmail(event.organizer) === me;
  const myResponse = responses.find((r) => normalizeEmail(r.email) === me);

  // Organizer view: read-only per-attendee status list.
  if (isOrganizer) {
    return (
      <View style={styles.field}>
        <Text style={styles.label}>{t('dialogs.event.rsvp.attendeeStatusLabel')}</Text>
        <View
          accessibilityRole="list"
          accessibilityLabel={t('dialogs.event.rsvp.attendeeStatusLabel')}
          style={styles.list}
        >
          {responses.map((r) => {
            const statusLabel = t(`dialogs.event.rsvp.status.${r.status}`);
            return (
              <View key={r.email} style={styles.row}>
                <Text
                  style={styles.attendee}
                  accessibilityRole="text"
                  accessibilityLabel={`${r.name ?? r.email}: ${statusLabel}`}
                >
                  {r.name ?? r.email} · {statusLabel}
                </Text>
              </View>
            );
          })}
        </View>
      </View>
    );
  }

  // Only a (non-organizer) attendee can respond.
  if (!myResponse) return null;

  const respond = async (status: AttendeeStatus) => {
    setPending(status);
    setError(null);
    try {
      // Respond against the loaded event's id — `getEventById` loads the series
      // master, so this is already the series id (no synthetic `@ISO` suffix).
      await respondToEvent(event.calendar_id, event.id, status, true);
      AccessibilityInfo.announceForAccessibility(
        t('dialogs.event.rsvp.responded', {
          status: t(`dialogs.event.rsvp.status.${status}`),
        }),
      );
      onResponded();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setPending(null);
    }
  };

  return (
    <View style={styles.field}>
      <Text style={styles.label}>{t('dialogs.event.rsvp.yourResponseLabel')}</Text>
      <View
        accessibilityRole="radiogroup"
        accessibilityLabel={t('dialogs.event.rsvp.yourResponseLabel')}
        style={styles.buttons}
      >
        {RESPONSE_ACTIONS.map((status) => {
          const current = myResponse.status === status;
          return (
            <Pressable
              key={status}
              accessibilityRole="button"
              accessibilityState={{ selected: current, disabled: pending !== null }}
              accessibilityLabel={t(`dialogs.event.rsvp.action.${status}`)}
              disabled={pending !== null}
              onPress={() => void respond(status)}
              style={({ pressed }) => [
                styles.button,
                current && styles.buttonCurrent,
                pressed && styles.pressed,
              ]}
            >
              <Text
                style={[styles.buttonText, current && styles.buttonTextCurrent]}
                importantForAccessibility="no"
              >
                {t(`dialogs.event.rsvp.action.${status}`)}
              </Text>
            </Pressable>
          );
        })}
      </View>
      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    field: { gap: 8 },
    label: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    list: { gap: 8 },
    row: {
      paddingVertical: 8,
      paddingHorizontal: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    attendee: { fontSize: 16, color: c.textPrimary },
    buttons: { flexDirection: 'row', flexWrap: 'wrap', gap: 10 },
    button: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    buttonCurrent: { backgroundColor: c.accent, borderColor: c.accent },
    buttonText: { fontSize: 16, fontWeight: '600', color: c.textPrimary },
    buttonTextCurrent: { color: c.textOnAccent, fontWeight: '700' },
    error: { fontSize: 14, fontWeight: '600', color: c.danger },
    pressed: { opacity: 0.7 },
  });
