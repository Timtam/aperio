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

// Event attendees editor — the mobile analogue of the desktop AttendeePicker,
// minus the contacts typeahead (that needs the not-yet-bridged contact search).
// Attendees are free-form "Name <email>" / bare-email strings (the wire shape);
// each new entry is validated + de-duplicated by its parsed email via the shared
// cal-core parser. Screen-reader-first: a labelled add field + button, a list of
// removable attendees with SR focus moved on add/remove, and a "notify
// attendees" switch shown only when invitations are meaningful (external
// calendar + at least one attendee), matching the desktop's gating.

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
  const [input, setInput] = useState('');
  const [error, setError] = useState<string | null>(null);
  // Move SR focus to the new/sibling row after add/remove (RN won't on its own).
  const { registerRow, registerAdd, onAdd, onRemove } = useListFocusManager(value.length);

  const add = () => {
    const entry = input.trim();
    if (entry === '') return;
    const email = emailOf(entry);
    if (email === '') {
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
              trackColor={{ false: '#c9d2e0', true: '#1d4ed8' }}
              importantForAccessibility="no"
              accessibilityElementsHidden
            />
          </View>
        </Pressable>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  field: { gap: 8 },
  label: { fontSize: 15, fontWeight: '600', color: '#2b3240' },
  list: { gap: 8 },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingVertical: 8,
    paddingHorizontal: 12,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f8fafc',
  },
  attendee: { flex: 1, fontSize: 16, color: '#10131a' },
  addRow: { flexDirection: 'row', gap: 10, alignItems: 'center' },
  input: {
    flex: 1,
    fontSize: 17,
    color: '#10131a',
    paddingVertical: 12,
    paddingHorizontal: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#ffffff',
  },
  addButton: {
    paddingVertical: 12,
    paddingHorizontal: 18,
    borderRadius: 10,
    backgroundColor: '#1d4ed8',
    alignItems: 'center',
  },
  addButtonText: { fontSize: 16, fontWeight: '700', color: '#ffffff' },
  removeButton: {
    paddingVertical: 8,
    paddingHorizontal: 12,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#d9b3b0',
    backgroundColor: '#fbeceb',
  },
  removeButtonText: { fontSize: 14, fontWeight: '600', color: '#b42318' },
  switchRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 12,
    paddingVertical: 10,
    paddingHorizontal: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f8fafc',
  },
  switchLabel: { flex: 1, fontSize: 16, color: '#10131a' },
  error: { fontSize: 14, fontWeight: '600', color: '#b42318' },
  pressed: { opacity: 0.7 },
});
