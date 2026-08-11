import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { AccessibilityInfo, Platform, Pressable, StyleSheet, Text, View } from 'react-native';

import { useThemedStyles, type ThemeColors } from '../theme';

/**
 * Earlier entries with the name being typed, offered under the title field.
 *
 * The desktop half of this is a combobox — a popup over the input, arrowed
 * through with `aria-activedescendant`. React Native has no such thing, and
 * faking one is how you get a control that VoiceOver and TalkBack each
 * misread differently. So the offers are what they actually are here: buttons
 * in a list under the field, reachable by the same swipe that reaches
 * everything else, each saying what it is and where it came from.
 *
 * Accepting one fills the rest of the editor from that earlier entry. It never
 * fills the day — that is what makes this a new entry, and it came from
 * wherever the editor was opened.
 */
/** How long the offers must hold still before VoiceOver is told about them. */
const ANNOUNCE_IDLE_MS = 800;

export interface TitleSuggestionOption {
  id: string;
  title: string;
  /** Where it comes from — the calendar or the list. */
  hint?: string;
}

export function TitleSuggestions({
  options,
  onAccept,
  editable = true,
}: {
  options: readonly TitleSuggestionOption[];
  onAccept: (id: string) => void;
  editable?: boolean;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  // How many there are, once they arrive. `accessibilityLiveRegion` is ANDROID
  // ONLY, so on iOS this announce is the only channel VoiceOver has — and a
  // list that appears in silence is a list a screen-reader user never learns
  // is there.
  const spoken = useRef(0);
  useEffect(() => {
    if (options.length === 0) {
      spoken.current = 0;
      return;
    }
    if (options.length === spoken.current) return;
    // Not immediately. While someone DICTATES a title the list keeps changing,
    // and an announcement then talks over the recognition the user is still
    // producing — on iOS, where this announce is VoiceOver's only channel for
    // the list, that is the worst possible moment for it. Waiting for the
    // offers to hold still means it lands in the pause after the phrase, which
    // is when anyone would act on it anyway.
    const timer = setTimeout(() => {
      spoken.current = options.length;
      if (Platform.OS === 'ios') {
        AccessibilityInfo.announceForAccessibility(
          t('suggestions.count', { count: options.length }),
        );
      }
    }, ANNOUNCE_IDLE_MS);
    return () => clearTimeout(timer);
  }, [options.length, t]);

  if (options.length === 0) return null;
  return (
    <View style={styles.wrap}>
      <Text
        style={styles.heading}
        accessibilityRole="text"
        accessibilityLiveRegion="polite"
      >
        {t('suggestions.count', { count: options.length })}
      </Text>
      {options.map((option) => (
        <Pressable
          key={option.id}
          accessibilityRole="button"
          accessibilityLabel={
            option.hint ? `${option.title}, ${option.hint}` : option.title
          }
          accessibilityHint={t('suggestions.acceptHint')}
          accessibilityState={{ disabled: !editable }}
          disabled={!editable}
          onPress={() => onAccept(option.id)}
          style={({ pressed }) => [styles.option, pressed && styles.pressed]}
        >
          <Text style={styles.optionTitle}>{option.title}</Text>
          {option.hint != null && option.hint !== '' && (
            <Text style={styles.optionHint}>{option.hint}</Text>
          )}
        </Pressable>
      ))}
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    wrap: { gap: 6 },
    heading: { fontSize: 13, color: c.textSecondary },
    option: {
      minHeight: 44,
      justifyContent: 'center',
      paddingHorizontal: 12,
      paddingVertical: 8,
      borderRadius: 8,
      backgroundColor: c.surfaceAlt,
    },
    pressed: { opacity: 0.7 },
    optionTitle: { fontSize: 15, color: c.textPrimary },
    optionHint: { fontSize: 13, color: c.textSecondary },
  });
