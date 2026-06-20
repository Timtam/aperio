import { useCallback, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  findNodeHandle,
  Linking,
  Modal,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { ALLOWED_LINK_SCHEMES, schemeOf } from '@aperio/shared';

import type { AdapterKind } from '../api/accounts';
import { useThemedStyles, type ThemeColors } from '../theme';

// One-shot privacy notice (DESIGN.md §10.6) — the RN twin of the desktop
// ContactsPrivacyNoticeModal. Shown the first time a contacts-capable account is
// connected; acknowledging writes the `contacts.privacyNoticeAcknowledged` pref
// so it never re-appears (the same body lives as a standing reference in
// ContactsSettingsScreen). There's no cancel path — the account already exists,
// so a cancel-shaped action would mislead; the only action is "Got it" (which
// the hardware-back also routes to). Screen-reader-first: the acknowledge
// button takes focus on open so it dismisses without hunting for it.

interface ProviderPolicy {
  name: string;
  url: string;
}

/** Map an adapter kind to the provider whose privacy policy to link. CardDAV is
 *  generic (the user-supplied server is the source of truth), so it shows the
 *  generic line. Mirrors the desktop `providerPolicyFor`. */
function providerPolicyFor(kind: AdapterKind | null): ProviderPolicy | null {
  switch (kind) {
    case 'google':
      return { name: 'Google', url: 'https://policies.google.com/privacy' };
    case 'microsoft_graph':
    case 'ews':
      return { name: 'Microsoft', url: 'https://privacy.microsoft.com/privacystatement' };
    default:
      return null;
  }
}

export interface ContactsPrivacyNoticeModalProps {
  /** Adapter kind of the just-connected account; `null` keeps the modal closed
   *  (the consumer derives `visible` from it). */
  adapterKind: AdapterKind | null;
  onAcknowledge: () => void;
}

export default function ContactsPrivacyNoticeModal({
  adapterKind,
  onAcknowledge,
}: ContactsPrivacyNoticeModalProps) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const insets = useSafeAreaInsets();
  const ackRef = useRef<View>(null);

  const provider = providerPolicyFor(adapterKind);

  const onShow = useCallback(() => {
    const tag = findNodeHandle(ackRef.current);
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, []);

  const openPolicy = useCallback(
    async (url: string) => {
      const scheme = schemeOf(url);
      if (scheme == null || !ALLOWED_LINK_SCHEMES.has(scheme)) return;
      try {
        await Linking.openURL(url);
      } catch (err) {
        AccessibilityInfo.announceForAccessibility(
          t('mobile.error', { message: err instanceof Error ? err.message : String(err) }),
        );
      }
    },
    [t],
  );

  return (
    <Modal
      visible={adapterKind !== null}
      animationType="slide"
      onRequestClose={onAcknowledge}
      onShow={onShow}
    >
      <ScrollView
        style={styles.screen}
        contentContainerStyle={[
          styles.content,
          { paddingTop: insets.top + 16, paddingBottom: insets.bottom + 24 },
        ]}
        keyboardShouldPersistTaps="handled"
      >
        <Text accessibilityRole="header" style={styles.title}>
          {t('dialogs.accounts.privacyNotice.title')}
        </Text>
        <Text style={styles.body} accessibilityRole="text">
          {t('dialogs.accounts.privacyNotice.body')}
        </Text>

        {provider ? (
          <>
            <Text style={styles.body} accessibilityRole="text">
              {t('dialogs.accounts.privacyNotice.providerLine', { provider: provider.name })}
            </Text>
            <Pressable
              accessibilityRole="link"
              accessibilityLabel={t('dialogs.accounts.privacyNotice.providerLink', {
                provider: provider.name,
              })}
              onPress={() => void openPolicy(provider.url)}
              style={({ pressed }) => [styles.link, pressed && styles.pressed]}
            >
              <Text style={styles.linkText} importantForAccessibility="no">
                {t('dialogs.accounts.privacyNotice.providerLink', { provider: provider.name })}
              </Text>
            </Pressable>
          </>
        ) : (
          <Text style={styles.body} accessibilityRole="text">
            {t('dialogs.accounts.privacyNotice.providerGeneric')}
          </Text>
        )}

        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.accounts.privacyNotice.cacheHint')}
        </Text>

        <Pressable
          ref={ackRef}
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.accounts.privacyNotice.acknowledge')}
          onPress={onAcknowledge}
          style={({ pressed }) => [styles.button, pressed && styles.pressed]}
        >
          <Text style={styles.buttonText} importantForAccessibility="no">
            {t('dialogs.accounts.privacyNotice.acknowledge')}
          </Text>
        </Pressable>
      </ScrollView>
    </Modal>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { paddingHorizontal: 16, gap: 16 },
    title: { fontSize: 24, fontWeight: '800', color: c.textPrimary },
    body: { fontSize: 16, color: c.textPrimary, lineHeight: 22 },
    hint: { fontSize: 14, color: c.textSecondary, lineHeight: 20 },
    link: {
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    linkText: { fontSize: 15, fontWeight: '600', color: c.link },
    button: {
      paddingVertical: 14,
      paddingHorizontal: 18,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
      marginTop: 8,
    },
    buttonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
    pressed: { opacity: 0.7 },
  });
