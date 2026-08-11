import { forwardRef, memo } from 'react';
import { StyleSheet, TextInput, type TextStyle } from 'react-native';

import { useThemedStyles, type ThemeColors } from '../theme';

/**
 * The title input of the editors and quick-adds — a plain `TextInput`, kept
 * apart from everything that renders around it.
 *
 * ## Why it is its own component
 *
 * Under the title field sits the suggestion list, fed by a search that runs
 * while you type. Every result that arrives sets state on the SCREEN, so the
 * screen re-renders — and until this component existed, that re-rendered the
 * text input too, handing React Native a `value` for a field the user is in
 * the middle of writing.
 *
 * Typing survives that: each keystroke is reported, applied and re-rendered in
 * one short round trip. DICTATION does not. iOS extends the field from its own
 * recognition session, asynchronously, and tells JS afterwards — so a render
 * that arrives from the side, carrying the text as JS last knew it, lands in
 * the middle of an utterance. What came back was the finished sentence with
 * the first recognised fragment stuck on the end: "Henk antworten Henk".
 *
 * `memo` is the whole fix: when the suggestion list changes and the text has
 * not, this subtree does not render at all, and nothing reaches the native
 * field. It only works while the props stay stable, so every caller passes a
 * `useCallback`'d handler — a fresh closure per render would defeat it
 * silently, which is why the handler type is spelled out rather than inlined.
 *
 * The ref is forwarded to the `TextInput` itself, unchanged: the screens use it
 * for `findNodeHandle` (moving screen-reader focus into the field) and
 * `focus()`, and both must keep meaning exactly what they meant.
 */
export interface TitleFieldProps {
  /** The current title. */
  value: string;
  /** Stable — see the note on `memo` above. */
  onChangeText: (text: string) => void;
  placeholder?: string;
  accessibilityLabel: string;
  editable?: boolean;
  returnKeyType?: 'next' | 'done';
  /** The quick-adds create the entry from the keyboard's Done key. Stable
   *  too — it is a prop like any other, and an unstable one costs the memo. */
  onSubmitEditing?: () => void;
  /** The screen's own input styling, so this stays a drop-in replacement. */
  style?: TextStyle;
}

export const TitleField = memo(
  forwardRef<TextInput, TitleFieldProps>(function TitleField(
    {
      value,
      onChangeText,
      placeholder,
      accessibilityLabel,
      editable = true,
      returnKeyType,
      onSubmitEditing,
      style,
    },
    ref,
  ) {
    const styles = useThemedStyles(makeStyles);
    return (
      <TextInput
        ref={ref}
        style={[styles.input, style]}
        value={value}
        onChangeText={onChangeText}
        placeholder={placeholder}
        accessibilityLabel={accessibilityLabel}
        editable={editable}
        returnKeyType={returnKeyType}
        onSubmitEditing={onSubmitEditing}
        // A title is a name, not a form field the OS should complete for the
        // user; the suggestions under it are the app's own, and better.
        autoComplete="off"
      />
    );
  }),
);

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    input: {
      borderWidth: 1,
      borderColor: c.border,
      borderRadius: 8,
      paddingHorizontal: 12,
      paddingVertical: 10,
      fontSize: 16,
      color: c.textPrimary,
      backgroundColor: c.surface,
    },
  });
