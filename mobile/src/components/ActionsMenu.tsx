import { useTranslation } from 'react-i18next';
import { Pressable, StyleSheet, Text } from 'react-native';

import { useThemedStyles, type ThemeColors } from '../theme';
import { AppDialog } from './AppDialog';

// Long-press action menu — the SIGHTED twin of the screen-reader custom
// actions ("wie Rotor", per Toni's tester): a row's long-press opens the SAME
// action list VoiceOver exposes via the rotor, so both audiences get the full
// per-item verb set (edit / delete / duplicate / plan / …) without a visible
// button per verb cluttering every row. Hosted in the focus-trapping AppDialog
// as a choice-only dialog (no Confirm — each action IS a choice; Cancel/
// tap-outside closes).

/** One menu entry — the same (name, label) pair the row already feeds to
 *  `accessibilityActions`, so screens reuse a single action list for both. */
export interface MenuAction {
  name: string;
  label: string;
  /** Styled as destructive (delete-class actions). */
  destructive?: boolean;
}

export function ActionsMenu({
  visible,
  title,
  actions,
  onAction,
  onClose,
}: {
  visible: boolean;
  /** The item's title — names the menu for both audiences. */
  title: string;
  actions: readonly MenuAction[];
  /** Called with the chosen action's `name` (the menu closes first). */
  onAction: (name: string) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  if (!visible) return null;
  return (
    <AppDialog
      visible
      title={title}
      cancelLabel={t('mobile.cancel')}
      onCancel={onClose}
    >
      {actions.map((a) => (
        <Pressable
          key={a.name}
          accessibilityRole="button"
          accessibilityLabel={a.label}
          onPress={() => {
            // Close BEFORE dispatching so a navigation the action triggers
            // isn't raced by the dialog's unmount focus-return.
            onClose();
            onAction(a.name);
          }}
          style={({ pressed }) => [
            styles.action,
            a.destructive && styles.actionDestructive,
            pressed && styles.pressed,
          ]}
        >
          <Text style={[styles.actionText, a.destructive && styles.actionTextDestructive]}>
            {a.label}
          </Text>
        </Pressable>
      ))}
    </AppDialog>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    action: {
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      marginBottom: 8,
    },
    actionDestructive: {
      borderColor: c.dangerBorder,
      backgroundColor: c.dangerBg,
    },
    pressed: { opacity: 0.7 },
    actionText: { fontSize: 16, fontWeight: '600', color: c.link, textAlign: 'center' },
    actionTextDestructive: { color: c.danger },
  });
