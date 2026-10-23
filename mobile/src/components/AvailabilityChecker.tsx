import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AccessibilityInfo, Pressable, StyleSheet, Text, View } from 'react-native';

import { readFailureSentence } from '@aperio/shared';

import { FreeBusy, FreeBusySlot, queryFreeBusy } from '../api/calendar';
import { useThemedStyles, type ThemeColors } from '../theme';

// Attendee free/busy check for the event editor — the mobile twin of the
// desktop EventDialog's "Check availability". On demand it queries the
// attendees' busy slots over the currently-entered window (through the account
// that owns the calendar) and shows a per-attendee Busy/Free verdict + a summary.
// Best-effort: a local calendar or a provider that can't answer returns no
// slots, which reads as "free/unknown". Screen-reader-first: a labelled button,
// a polite result summary announcement, and each attendee row an addressable
// element labelled "{email}: {Busy|Free}".

/** Bare email out of an attendee entry ("Name <email>" or a raw email) — the
 *  free/busy lookup keys on the address. Mirrors the desktop `attendeeEmail`. */
function attendeeEmail(entry: string): string {
  const angle = entry.match(/<([^>]+)>/);
  return (angle ? angle[1] : entry).trim();
}

/** True when any slot overlaps `[startIso, endIso)`. Compared via epoch millis
 *  so it's robust to TZ/format differences. Mirrors the desktop `isBusyInWindow`. */
function isBusyInWindow(slots: FreeBusySlot[], startIso: string, endIso: string): boolean {
  const ws = new Date(startIso).getTime();
  const we = new Date(endIso).getTime();
  return slots.some(
    (s) => new Date(s.start).getTime() < we && new Date(s.end).getTime() > ws,
  );
}

export function AvailabilityChecker({
  calendarId,
  attendees,
  start,
  end,
}: {
  calendarId: string;
  /** Free-form attendee entries ("Name <email>" / bare email). */
  attendees: string[];
  /** The event window as RFC-3339 instants, or null when unparseable. */
  start: string | null;
  end: string | null;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);

  // The last check's result + the exact window it was run for (so each verdict
  // is computed against what the user asked about), cleared whenever the
  // attendees or window change so a stale verdict never shows.
  const [availability, setAvailability] = useState<FreeBusy[] | null>(null);
  const [checkedWindow, setCheckedWindow] = useState<{ start: string; end: string } | null>(
    null,
  );
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const attendeesKey = JSON.stringify(attendees);
  useEffect(() => {
    setAvailability(null);
    setCheckedWindow(null);
    setError(null);
  }, [attendeesKey, start, end, calendarId]);

  const check = useCallback(async () => {
    // An unparseable/empty window: tell the user why nothing happened (the
    // assertive error region announces it), matching the desktop — a silent
    // no-op button is a dead end for a screen-reader user.
    if (start == null || end == null) {
      setError(t('dialogs.event.dateInvalid'));
      return;
    }
    const emails = attendees.map(attendeeEmail).filter((e) => e.length > 0);
    if (emails.length === 0) return;
    setChecking(true);
    setError(null);
    try {
      const result = await queryFreeBusy(calendarId, emails, start, end);
      setAvailability(result);
      setCheckedWindow({ start, end });
      const busyCount = result.filter((fb) => isBusyInWindow(fb.slots, start, end)).length;
      AccessibilityInfo.announceForAccessibility(
        busyCount === 0
          ? t('dialogs.event.availability.allFree')
          : t('dialogs.event.availability.someBusy', { count: busyCount }),
      );
    } catch (err) {
      // A check that failed says why, and the previous answer goes: "All
      // attendees are free" must never stand beside it (decision 273).
      setAvailability(null);
      setCheckedWindow(null);
      setError(readFailureSentence('dialogs.event.availability.failed', err, t));
    } finally {
      setChecking(false);
    }
  }, [attendees, calendarId, end, start, t]);

  const busyCount =
    availability && checkedWindow
      ? availability.filter((fb) => isBusyInWindow(fb.slots, checkedWindow.start, checkedWindow.end))
          .length
      : 0;

  return (
    <View style={styles.field}>
      <Pressable
        accessibilityRole="button"
        accessibilityState={{ disabled: checking }}
        accessibilityLabel={t('dialogs.event.availability.check')}
        disabled={checking}
        onPress={() => void check()}
        style={({ pressed }) => [styles.button, pressed && styles.pressed]}
      >
        <Text style={styles.buttonText}>
          {checking
            ? t('dialogs.event.availability.checking')
            : t('dialogs.event.availability.check')}
        </Text>
      </Pressable>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {availability != null && checkedWindow != null && (
        <View style={styles.results}>
          {/* The result is announced once via announceForAccessibility in check()
              (the desktop announce-once pattern); this Text is the navigable
              read-back, so it must NOT also be a live region or TalkBack speaks
              the same summary twice. */}
          <Text style={styles.summary} accessibilityRole="text">
            {busyCount === 0
              ? t('dialogs.event.availability.allFree')
              : t('dialogs.event.availability.someBusy', { count: busyCount })}
          </Text>
          {availability.length > 0 && (
            <View accessibilityRole="list" style={styles.list}>
              {availability.map((fb) => {
                const busy = isBusyInWindow(fb.slots, checkedWindow.start, checkedWindow.end);
                const status = busy
                  ? t('dialogs.event.availability.busy')
                  : t('dialogs.event.availability.free');
                return (
                  <View key={fb.email} style={styles.row}>
                    <Text
                      style={styles.email}
                      accessibilityRole="text"
                      accessibilityLabel={`${fb.email}: ${status}`}
                    >
                      {fb.email}
                    </Text>
                    <Text
                      style={[styles.status, busy ? styles.statusBusy : styles.statusFree]}
                      importantForAccessibility="no"
                    >
                      {status}
                    </Text>
                  </View>
                );
              })}
            </View>
          )}
        </View>
      )}
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    field: { gap: 8 },
    button: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignItems: 'center',
    },
    buttonText: { fontSize: 16, fontWeight: '600', color: c.accent },
    results: { gap: 8 },
    summary: { fontSize: 15, fontWeight: '600', color: c.textPrimary },
    list: { gap: 8 },
    row: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
      gap: 10,
      paddingVertical: 8,
      paddingHorizontal: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    email: { flex: 1, fontSize: 15, color: c.textPrimary },
    status: { fontSize: 14, fontWeight: '700' },
    statusBusy: { color: c.danger },
    statusFree: { color: c.accent },
    error: { fontSize: 14, fontWeight: '600', color: c.danger },
    pressed: { opacity: 0.7 },
  });
