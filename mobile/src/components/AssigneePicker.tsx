import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { AccessibilityInfo, Pressable, StyleSheet, Text, View } from 'react-native';

import type { TaskUser } from '@aperio/shared';

import { useThemedStyles, type ThemeColors } from '../theme';

// Multi-select picker for task assignees, scoped to a list's member pool
// (DESIGN §9.7) — the mobile twin of the desktop AssigneePicker. Selected users
// render as a list of removable rows; the members not yet picked render as
// "add" buttons (RN has no native <select>). Purely presentational: the caller
// gates rendering on whether the list has any assignable members. Screen-
// reader-first — every row/button is an addressable element with an explicit
// "Add {name}" / "Remove {name}" label, and add/remove announce the result.

export function AssigneePicker({
  members,
  value,
  currentUserId,
  onChange,
}: {
  members: TaskUser[];
  value: TaskUser[];
  currentUserId: string | null;
  onChange: (next: TaskUser[]) => void;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const selectedIds = useMemo(() => new Set(value.map((u) => u.id)), [value]);
  const available = useMemo(
    () => members.filter((m) => !selectedIds.has(m.id)),
    [members, selectedIds],
  );

  // Mark the connected account itself so "assign to me" is obvious.
  const label = (u: TaskUser) =>
    u.id === currentUserId ? t('dialogs.task.assignees.you', { name: u.name }) : u.name;

  const add = (user: TaskUser) => {
    if (selectedIds.has(user.id)) return;
    onChange([...value, user]);
    AccessibilityInfo.announceForAccessibility(
      t('dialogs.task.assignees.added', { name: user.name }),
    );
  };

  const remove = (user: TaskUser) => {
    onChange(value.filter((x) => x.id !== user.id));
    AccessibilityInfo.announceForAccessibility(
      t('dialogs.task.assignees.removed', { name: user.name }),
    );
  };

  return (
    <View style={styles.field}>
      <Text style={styles.label}>{t('dialogs.task.fields.assignees')}</Text>

      {value.length > 0 && (
        <View
          accessibilityRole="list"
          accessibilityLabel={t('dialogs.task.fields.assignees')}
          style={styles.list}
        >
          {value.map((u) => (
            <View key={u.id} style={styles.row}>
              <Text style={styles.name} accessibilityRole="text" accessibilityLabel={label(u)}>
                {label(u)}
              </Text>
              <Pressable
                accessibilityRole="button"
                accessibilityLabel={t('dialogs.task.assignees.remove', { name: u.name })}
                onPress={() => remove(u)}
                style={({ pressed }) => [styles.removeButton, pressed && styles.pressed]}
              >
                <Text style={styles.removeButtonText} importantForAccessibility="no">
                  {t('mobile.delete')}
                </Text>
              </Pressable>
            </View>
          ))}
        </View>
      )}

      {available.length > 0 && (
        <View
          accessibilityRole="list"
          accessibilityLabel={t('dialogs.task.assignees.add')}
          style={styles.addList}
        >
          {available.map((m) => (
            <Pressable
              key={m.id}
              accessibilityRole="button"
              accessibilityLabel={t('dialogs.task.assignees.addNamed', { name: m.name })}
              onPress={() => add(m)}
              style={({ pressed }) => [styles.addButton, pressed && styles.pressed]}
            >
              <Text style={styles.addButtonText} importantForAccessibility="no">
                {label(m)}
              </Text>
            </Pressable>
          ))}
        </View>
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
    name: { flex: 1, fontSize: 16, color: c.textPrimary },
    addList: { gap: 8 },
    addButton: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    addButtonText: { fontSize: 16, color: c.link, fontWeight: '600' },
    removeButton: {
      paddingVertical: 8,
      paddingHorizontal: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.dangerBorder,
      backgroundColor: c.dangerBg,
    },
    removeButtonText: { fontSize: 14, fontWeight: '600', color: c.danger },
    pressed: { opacity: 0.7 },
  });
