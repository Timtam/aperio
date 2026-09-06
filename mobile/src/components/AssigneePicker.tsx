import { useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  findNodeHandle,
  Pressable,
  StyleSheet,
  Text,
  View,
} from 'react-native';

import type { TaskAssignment, TaskUser } from '@aperio/shared';

import { useThemedStyles, type ThemeColors } from '../theme';

// Picker for task assignees, scoped to a list's member pool (DESIGN §9.7) —
// the mobile twin of the desktop AssigneePicker, and it takes the same two
// shapes, chosen by what the SOURCE can hold.
//
// `multiple`: selected users render as removable rows, the rest as "add"
// buttons (RN has no native <select>). `single`: one radio group, because a
// source that keeps one assignee must not be asked for two — Todoist stores a
// single `assignee_id` and its adapter drops the rest on write, with the save
// still reporting success.
//
// Purely presentational: the caller gates rendering on the list's declared
// `task_assignment` mode and on there being any assignable members. Screen-
// reader-first — every row/button is an addressable element with an explicit
// label, and every change announces its result.

export function AssigneePicker({
  members,
  value,
  currentUserId,
  mode = 'multiple',
  onChange,
}: {
  members: TaskUser[];
  value: TaskUser[];
  currentUserId: string | null;
  /** What the source can hold. `none` is not passed — the caller does not
   *  render the picker at all then. */
  mode?: TaskAssignment;
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

  /** One per remove button, in render order — the repark targets. */
  const removeRefs = useRef<(View | null)[]>([]);

  const add = (user: TaskUser) => {
    if (selectedIds.has(user.id)) return;
    onChange([...value, user]);
    AccessibilityInfo.announceForAccessibility(
      t('dialogs.task.assignees.added', { name: user.name }),
    );
  };

  const remove = (user: TaskUser, index: number) => {
    onChange(value.filter((x) => x.id !== user.id));
    AccessibilityInfo.announceForAccessibility(
      t('dialogs.task.assignees.removed', { name: user.name }),
    );
    // RN does not move the screen reader's cursor when the element under it
    // unmounts — the strand `useListFocusManager` documents. Park it on the
    // remove button that slid into this slot, else the previous one, so
    // several people can be removed without re-navigating each time.
    setTimeout(() => {
      const target = removeRefs.current[index] ?? removeRefs.current[index - 1];
      const tag = target ? findNodeHandle(target) : null;
      if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
    }, 0);
  };

  // One assignee at most: a radio group, so the screen reader says which one
  // is chosen and there is no way to pick a second the source would drop.
  // "Nobody" is a real choice here, not the absence of one.
  if (mode === 'single') {
    const pick = (user: TaskUser | null) => {
      onChange(user ? [user] : []);
      AccessibilityInfo.announceForAccessibility(
        user
          ? t('dialogs.task.assignees.added', { name: user.name })
          : t('dialogs.task.assignees.nobody'),
      );
    };
    const chosenId = value[0]?.id ?? null;
    return (
      <View style={styles.field}>
        <Text style={styles.label}>{t('dialogs.task.fields.assignees')}</Text>
        <View
          accessibilityRole="radiogroup"
          accessibilityLabel={t('dialogs.task.fields.assignees')}
          style={styles.addList}
        >
          <Pressable
            accessibilityRole="radio"
            accessibilityState={{ checked: chosenId == null }}
            accessibilityLabel={t('dialogs.task.assignees.nobody')}
            onPress={() => pick(null)}
            style={({ pressed }) => [styles.addButton, pressed && styles.pressed]}
          >
            <Text style={styles.addButtonText} importantForAccessibility="no">
              {t('dialogs.task.assignees.nobody')}
            </Text>
          </Pressable>
          {members.map((m) => (
            <Pressable
              key={m.id}
              accessibilityRole="radio"
              accessibilityState={{ checked: chosenId === m.id }}
              accessibilityLabel={label(m)}
              onPress={() => pick(m)}
              style={({ pressed }) => [styles.addButton, pressed && styles.pressed]}
            >
              <Text style={styles.addButtonText} importantForAccessibility="no">
                {label(m)}
              </Text>
            </Pressable>
          ))}
        </View>
      </View>
    );
  }

  return (
    <View style={styles.field}>
      <Text style={styles.label}>{t('dialogs.task.fields.assignees')}</Text>

      {value.length > 0 && (
        <View
          accessibilityRole="list"
          accessibilityLabel={t('dialogs.task.fields.assignees')}
          style={styles.list}
        >
          {value.map((u, i) => (
            <View key={u.id} style={styles.row}>
              <Text style={styles.name} accessibilityRole="text" accessibilityLabel={label(u)}>
                {label(u)}
              </Text>
              <Pressable
                ref={(node) => {
                  removeRefs.current[i] = node as View | null;
                }}
                accessibilityRole="button"
                accessibilityLabel={t('dialogs.task.assignees.remove', { name: u.name })}
                onPress={() => remove(u, i)}
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
