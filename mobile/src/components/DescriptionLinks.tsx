import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Linking,
  Pressable,
  StyleSheet,
  Text,
  View,
} from 'react-native';

import { detectLinks, schemeOf, ALLOWED_LINK_SCHEMES } from '@aperio/shared';

import { useThemedStyles, type ThemeColors } from '../theme';

/**
 * Activatable link bar shown under an editable (plain-text) description field —
 * the mobile port of the desktop DescriptionLinks. Detects the URLs / emails in
 * the current text and offers each as a real accessible link button that opens
 * in the OS browser / mail client. The field above stays the editable source of
 * truth; the links update live as the text changes.
 *
 * Opening re-validates the scheme (http/https/mailto only) before
 * `Linking.openURL` — descriptions can come from untrusted external invitations,
 * so this is the mobile analogue of the desktop `open_external_url` gate (the
 * shared `detectLinks` already filtered, but we re-check defensively).
 *
 * Renders nothing when the text has no openable links, so callers drop it in
 * unconditionally.
 */
export function DescriptionLinks({ text }: { text: string | null | undefined }) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const links = useMemo(() => detectLinks(text), [text]);

  if (links.length === 0) {
    return null;
  }

  const open = async (url: string) => {
    const scheme = schemeOf(url);
    if (scheme == null || !ALLOWED_LINK_SCHEMES.has(scheme)) return;
    try {
      await Linking.openURL(url);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      AccessibilityInfo.announceForAccessibility(
        t('descriptionLinks.openFailed', { message }),
      );
    }
  };

  return (
    <View style={styles.group} accessibilityRole="list">
      <Text style={styles.label} accessibilityRole="header">
        {t('descriptionLinks.label')}
      </Text>
      {links.map((link) => (
        <Pressable
          key={link.url}
          accessibilityRole="link"
          accessibilityLabel={t('descriptionLinks.open', { url: link.url })}
          onPress={() => void open(link.url)}
          style={({ pressed }) => [styles.item, pressed && styles.pressed]}
        >
          <Text style={styles.itemText} numberOfLines={1}>
            {link.text}
          </Text>
        </Pressable>
      ))}
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    group: { gap: 6 },
    label: { fontSize: 14, fontWeight: '600', color: c.textLabel },
    item: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    itemText: { fontSize: 15, fontWeight: '600', color: c.link },
    pressed: { opacity: 0.7 },
  });
