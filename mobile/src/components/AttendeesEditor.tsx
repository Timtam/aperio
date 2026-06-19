import { useState } from 'react';
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

import { useListFocusManager } from '../a11y/useListFocusManager';
import { parseAttendee } from '../api/calendar';
import { useTheme, useThemedStyles, type ThemeColors } from '../theme';

// Event attendees editor — the mobile analogue of the desktop AttendeePicker,
// minus the contacts typeahead (that needs the not-yet-bridged contact search).
// Attendees are free-form "Name <email>" / bare-email strings (the wire shape);
// the shared cal-core parser extracts the email (the CN in "Name <email>", else
// the whole entry — it does NOT validate), so each new entry is then checked for
// an email shape here and de-duplicated by that email. Screen-reader-first: a
// labelled add field + button, a list of removable attendees with SR focus moved
// on add/remove, and a "notify attendees" switch shown only when the calendar
// can actually invite (RFC-6638 scheduling), matching the desktop's gating.

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
  // Move SR focus to the new/sibling row after add/remove (RN won't on its own).
  const { registerRow, registerAdd, onAdd, onRemove } = useListFocusManager(value.length);

  const add = () => {
    const entry = input.trim();
    if (entry === '') return;
    const email = emailOf(entry);
    if (!EMAIL_RE.test(email)) {
      setError(t('dialogs.event.attendees.invalid'));
      AccessibilityInfo.announceForAccessibility(t('dialogs.event.attendees.invalid'));
      return;
    }
    const lower = email.toLowerCase();
    if (value.some((a) => emailOf(a).toLowerCase() === lower)) {
      setError(null);
      setInput('');
      AccessibilityInfo.announceForAccessibility(t('dialogs.event.attendees.alreadyOnList'));
      return;
    }
    setError(null);
    onAdd();
    onChange([...value, entry]);
    setInput('');
    AccessibilityInfo.announceForAccessibility(
      t('dialogs.event.attendees.added', { name: entry }),
    );
  };

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
