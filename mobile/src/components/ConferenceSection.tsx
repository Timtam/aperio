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

import { ALLOWED_LINK_SCHEMES, detectConference, schemeOf } from '@aperio/shared';

import { useThemedStyles, type ThemeColors } from '../theme';

/**
 * The "join this meeting" affordance on an event — the mobile twin of the
 * desktop ConferenceSection.
 *
 * Works for any event with a conference, whoever created it. Detection is the
 * shared implementation and reads URLs rather than prose labels, so it does not
 * care which language the invitation arrived in.
 *
 * Renders nothing when there is no meeting, so callers drop it in
 * unconditionally.
 *
 * Each detail is its own accessible element with its own label. A meeting
 * number and a password concatenated are read as one run-on sentence, and a
 * password is precisely what someone needs to hear on its own. The labels come
 * from the invitation itself where Aperio did not derive them — it does not
 * translate what it cannot interpret.
 */
export function ConferenceSection({
  location,
  description,
}: {
  location?: string | null;
  description?: string | null;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const conference = useMemo(
    () => detectConference({ location, description }),
    [location, description],
  );

  if (!conference) {
    return null;
  }

  const providerName = t(`conferencing.provider.${conference.provider}`);
  const derived = [
    conference.meetingNumber && {
      label: t('conferencing.meetingNumber'),
      value: conference.meetingNumber,
    },
    conference.password && {
      label: t('conferencing.password'),
      value: conference.password,
    },
  ].filter((d): d is { label: string; value: string } => !!d);
  const details = derived.length > 0 ? derived : conference.labelledDetails;

  const open = async () => {
    // Re-validate the scheme: a description can come from an untrusted external
    // invitation, and the detector's own filter is not the place to rely on.
    const scheme = schemeOf(conference.joinUrl);
    if (scheme == null || !ALLOWED_LINK_SCHEMES.has(scheme)) return;
    try {
      await Linking.openURL(conference.joinUrl);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      AccessibilityInfo.announceForAccessibility(
        t('conferencing.openFailed', { message }),
      );
    }
  };

  return (
    <View style={styles.section} accessibilityLabel={t('conferencing.label')}>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={t('conferencing.joinNamed', {
          provider: providerName,
        })}
        onPress={() => void open()}
        style={({ pressed }) => [styles.join, pressed && styles.joinPressed]}
      >
        <Text style={styles.joinText}>
          {t('conferencing.joinNamed', { provider: providerName })}
        </Text>
      </Pressable>
      {details.map((detail) => (
        <View
          key={`${detail.label}:${detail.value}`}
          style={styles.detail}
          accessible
          accessibilityLabel={t('conferencing.detail', {
            label: detail.label,
            value: detail.value,
          })}
        >
          <Text style={styles.detailLabel} importantForAccessibility="no">
            {detail.label}
          </Text>
          <Text
            style={styles.detailValue}
            importantForAccessibility="no"
            selectable
          >
            {detail.value}
          </Text>
        </View>
      ))}
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    section: {
      gap: 8,
      padding: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    join: {
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    joinPressed: { backgroundColor: c.accentPressed },
    joinText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
    detail: { flexDirection: 'row', flexWrap: 'wrap', gap: 8 },
    detailLabel: { fontSize: 15, fontWeight: '600', color: c.textSecondary },
    detailValue: { fontSize: 15, color: c.textPrimary },
  });
