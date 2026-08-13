import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Pressable,
  StyleSheet,
  Switch,
  Text,
  TextInput,
  View,
} from 'react-native';

import { formatAttendee, primaryChannelValue } from '@aperio/shared';

import { useListFocusManager } from '../a11y/useListFocusManager';
import { parseAttendee } from '../api/calendar';
import { searchContacts, type Contact } from '../api/contacts';
import { useTheme, useThemedStyles, type ThemeColors } from '../theme';

// Event attendees editor — the mobile analogue of the desktop AttendeePicker.
// Attendees are free-form "Name <email>" / bare-email strings (the wire shape);
// the shared cal-core parser extracts the email (the CN in "Name <email>", else
// the whole entry — it does NOT validate), so each new entry is then checked for
// an email shape here and de-duplicated by that email. Screen-reader-first: a
// labelled add field + button, a contacts typeahead below it (the SR-natural
// analogue of the desktop combobox — a list of result BUTTONS, not an
// aria-activedescendant popup, since TalkBack/VoiceOver have no combobox idiom),
// a list of removable attendees with SR focus moved on add/remove, and a "notify
// attendees" switch shown only when the calendar can actually invite (RFC-6638
// scheduling), matching the desktop's gating.

const SEARCH_DEBOUNCE_MS = 180;
const MAX_SUGGESTIONS = 8;

/** A pragmatic "looks like an email" check (cal-core's parser does no
 *  validation). Stricter than the desktop picker, which accepts any non-empty
 *  string — but the UI promises a valid address, so we hold to that. */
const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

function emailOf(entry: string): string {
  try {
    return parseAttendee(entry).email;
  } catch {
    return '';
  }
}

export function AttendeesEditor({
  value,
  onChange,
  notify,
  onNotifyChange,
  showNotify,
}: {
  value: string[];
  onChange: (next: string[]) => void;
  notify: boolean;
  onNotifyChange: (next: boolean) => void;
  /** Whether the "notify attendees" switch is meaningful (external calendar with
   *  ≥1 attendee). Local calendars never send invitations. */
  showNotify: boolean;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { colors } = useTheme();
  const [input, setInput] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [suggestions, setSuggestions] = useState<Contact[]>([]);
  // Move SR focus to the new/sibling row after add/remove (RN won't on its own).
  const { registerRow, registerAdd, onAdd, onRemove } = useListFocusManager(value.length);

  /** Append an entry: dedupe by email (or the raw string when no email), move
   *  SR focus to the new row, announce. `validate` gates the email-shape check
   *  — on for the free-form field, off for a picked contact (trusted). Returns
   *  whether the field should clear. */
  const commitEntry = useCallback(
    (entry: string, validate: boolean): boolean => {
      const trimmed = entry.trim();
      if (trimmed === '') return false;
      const email = emailOf(trimmed);
      if (validate && !EMAIL_RE.test(email)) {
        setError(t('dialogs.event.attendees.invalid'));
        AccessibilityInfo.announceForAccessibility(t('dialogs.event.attendees.invalid'));
        return false;
      }
      const key = (email || trimmed).toLowerCase();
      if (value.some((a) => (emailOf(a) || a).toLowerCase() === key)) {
        setError(null);
        AccessibilityInfo.announceForAccessibility(t('dialogs.event.attendees.alreadyOnList'));
        return true;
      }
      setError(null);
      onAdd();
      onChange([...value, trimmed]);
      AccessibilityInfo.announceForAccessibility(
        t('dialogs.event.attendees.added', { name: trimmed }),
      );
      return true;
    },
    [value, onChange, onAdd, t],
  );

  const add = () => {
    if (commitEntry(input, true)) {
      setInput('');
      setSuggestions([]);
    }
  };

  // Pick a contact from the typeahead — formats it to the "Name <email>" wire
  // shape and commits (no email-shape validation; the contact is trusted).
  const pickContact = (contact: Contact) => {
    if (commitEntry(formatAttendee(contact), false)) {
      setInput('');
      setSuggestions([]);
    }
  };

  // Contacts typeahead — debounced search over the existing bridge, mirroring
  // the desktop AttendeePicker: filter out already-listed contacts, cap the
  // list, and announce the result count politely so the user knows suggestions
  // appeared without it interrupting their typing.
  const lastAnnounced = useRef<number>(-1);
  useEffect(() => {
    const trimmed = input.trim();
    if (trimmed.length < 1) {
      setSuggestions([]);
      lastAnnounced.current = -1;
      return;
    }
    let cancelled = false;
    const handle = setTimeout(() => {
      void searchContacts(trimmed)
        .then((rows) => {
          if (cancelled) return;
          const taken = new Set(value.map((a) => emailOf(a).toLowerCase()));
          const filtered = rows
            .filter((c) => {
              const email = primaryChannelValue(c.emails)?.toLowerCase();
              return !email || !taken.has(email);
            })
            .slice(0, MAX_SUGGESTIONS);
          setSuggestions(filtered);
          // Announce once per settled result set (the debounce already gates
          // this to typing pauses), not on every keystroke.
          if (filtered.length !== lastAnnounced.current) {
            lastAnnounced.current = filtered.length;
            if (filtered.length > 0) {
              AccessibilityInfo.announceForAccessibility(
                t('dialogs.event.attendees.suggestionsCount', { count: filtered.length }),
              );
            }
          }
        })
        .catch(() => {
          if (!cancelled) setSuggestions([]);
        });
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      clearTimeout(handle);
    };
  }, [input, value, t]);

  const remove = (i: number) => {
    const removed = value[i];
    onRemove(i);
    const out = value.slice();
    out.splice(i, 1);
    onChange(out);
    AccessibilityInfo.announceForAccessibility(
      t('dialogs.event.attendees.removed', { name: removed }),
    );
  };

  return (
    <View style={styles.field}>
      <Text style={styles.label}>{t('dialogs.event.fields.attendees')}</Text>

      {value.length > 0 && (
        <View
          accessibilityRole="list"
          accessibilityLabel={t('dialogs.event.attendees.chipsLabel')}
          style={styles.list}
        >
          {value.map((attendee, i) => (
            // Index-keyed: controlled add/remove list, no reordering.
            <View key={`${attendee}-${i}`} style={styles.row}>
              <Text
                ref={registerRow(i)}
                style={styles.attendee}
                accessibilityRole="text"
                accessibilityLabel={attendee}
              >
                {attendee}
              </Text>
              <Pressable
                accessibilityRole="button"
                accessibilityLabel={t('dialogs.event.attendees.removeLabel', { name: attendee })}
                onPress={() => remove(i)}
                style={({ pressed }) => [styles.removeButton, pressed && styles.pressed]}
              >
                <Text style={styles.removeButtonText}>{t('mobile.delete')}</Text>
              </Pressable>
            </View>
          ))}
        </View>
      )}

      <View style={styles.addRow}>
        <TextInput
          style={styles.input}
          value={input}
          onChangeText={setInput}
          placeholder={t('dialogs.event.attendees.placeholder')}
          accessibilityLabel={t('dialogs.event.fields.attendees')}
          autoCapitalize="none"
          autoCorrect={false}
          keyboardType="email-address"
          returnKeyType="done"
          onSubmitEditing={add}
        />
        <Pressable
          ref={registerAdd}
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.event.attendees.add')}
          onPress={add}
          style={({ pressed }) => [styles.addButton, pressed && styles.pressed]}
        >
          <Text style={styles.addButtonText}>{t('mobile.add')}</Text>
        </Pressable>
      </View>

      {suggestions.length > 0 && (
        <View
          accessibilityRole="list"
          accessibilityLabel={t('dialogs.event.attendees.popupLabel')}
          style={styles.suggestions}
        >
          {suggestions.map((c) => {
            const email = primaryChannelValue(c.emails);
            return (
              <Pressable
                key={c.id}
                accessibilityRole="button"
                accessibilityLabel={
                  email
                    ? t('dialogs.event.attendees.suggestionLabel', {
                        name: c.display_name,
                        email,
                      })
                    : c.display_name
                }
                onPress={() => pickContact(c)}
                style={({ pressed }) => [styles.suggestion, pressed && styles.pressed]}
              >
                <Text style={styles.suggestionName} importantForAccessibility="no">
                  {c.display_name}
                </Text>
                {email != null && (
                  <Text style={styles.suggestionEmail} importantForAccessibility="no">
                    {email}
                  </Text>
                )}
              </Pressable>
            );
          })}
        </View>
      )}

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {showNotify && (
        <Pressable
          accessibilityRole="switch"
          accessibilityState={{ checked: notify }}
          accessibilityLabel={t('dialogs.event.fields.notifyAttendees')}
          onPress={() => onNotifyChange(!notify)}
          style={({ pressed }) => [styles.switchRow, pressed && styles.pressed]}
        >
          <Text style={styles.switchLabel} importantForAccessibility="no">
            {t('dialogs.event.fields.notifyAttendees')}
          </Text>
          <View pointerEvents="none">
            <Switch
              value={notify}
              trackColor={{ false: colors.border, true: colors.accent }}
              importantForAccessibility="no"
              accessibilityElementsHidden
            />
          </View>
        </Pressable>
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
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      paddingVertical: 8,
      paddingHorizontal: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    attendee: { flex: 1, fontSize: 16, color: c.textPrimary },
    addRow: { flexDirection: 'row', gap: 10, alignItems: 'center' },
    input: {
      flex: 1,
      fontSize: 17,
      color: c.textPrimary,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.background,
    },
    addButton: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    addButtonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
    suggestions: { gap: 6 },
    suggestion: {
      gap: 2,
      paddingVertical: 10,
      paddingHorizontal: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    suggestionName: { fontSize: 16, fontWeight: '600', color: c.textPrimary },
    suggestionEmail: { fontSize: 14, color: c.textSecondary },
    removeButton: {
      paddingVertical: 8,
      paddingHorizontal: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.dangerBorder,
      backgroundColor: c.dangerBg,
    },
    removeButtonText: { fontSize: 14, fontWeight: '600', color: c.danger },
    switchRow: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
      gap: 12,
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    switchLabel: { flex: 1, fontSize: 16, color: c.textPrimary },
    error: { fontSize: 14, fontWeight: '600', color: c.danger },
    pressed: { opacity: 0.7 },
  });
