import { useState, type ReactNode } from 'react';
import {
  Platform,
  ScrollView,
  StyleSheet,
  View,
  type StyleProp,
  type ViewStyle,
} from 'react-native';

import { A11yFormContainer } from '../../modules/a11y-form';
import { getFormLab } from '../a11y/formLab';

// The editors' and forms' root scroll container, and how it lives with the
// keyboard.
//
// On iOS the scroll view keeps its full height and runs on under the
// keyboard; React Native sets the covered part as its bottom content inset,
// natively and in window coordinates (`automaticallyAdjustKeyboardInsets`),
// and scrolls the focused input into view. That is how UIKit's own forms do it
// (Contacts, "New Contact"), and it is what VoiceOver needs: it reads the
// inset to know which part of the scroll view the keyboard hides, and keeps
// scrolling to the fields below.
//
// The earlier approach shortened the form instead (a KeyboardAvoidingView,
// later its own bottom padding), so the scroll view ended at the keyboard's
// edge. The fields below it then lay, on screen, under the keyboard's window:
// swiping right past the last visible field, VoiceOver went into the keyboard
// rather than scrolling on (Toni, builds 72 to 75: the feedback form's Send, a
// new task's Priority and Effort; Apple's Contacts scrolled on). The shortened
// form had been chosen against a report of focus jumping when the keyboard
// opened (6dad7a5a, 2026-06-20); if that comes back, it is the trade to
// revisit, not the shortening.
//
// Android resizes the window for the keyboard itself.
//
// The diagnostic build's form lab (src/a11y/formLab.ts) can put the content
// into one ordered VoiceOver container (modules/a11y-form), the way a table
// is; it takes the content's style over, so the layout stays the same. Read
// once, when the form mounts: a switch changed later must not rebuild an open
// form (half-typed fields would be lost).

export function FormScrollView({
  style,
  contentContainerStyle,
  accessibilityViewIsModal,
  children,
}: {
  style?: StyleProp<ViewStyle>;
  contentContainerStyle?: StyleProp<ViewStyle>;
  accessibilityViewIsModal?: boolean;
  children: ReactNode;
}) {
  const [ordered] = useState(() => getFormLab().container);
  const Ordered = ordered ? A11yFormContainer : null;
  return (
    <View accessibilityViewIsModal={accessibilityViewIsModal} style={[styles.flex, style]}>
      <ScrollView
        style={styles.flex}
        contentContainerStyle={Ordered ? undefined : contentContainerStyle}
        keyboardShouldPersistTaps="handled"
        // Set once, at mount: React Native reads it when the keyboard moves,
        // and a change while the keyboard is up would be missed.
        automaticallyAdjustKeyboardInsets={Platform.OS === 'ios'}
        automaticallyAdjustsScrollIndicatorInsets={Platform.OS === 'ios'}
      >
        {Ordered ? (
          <Ordered ordered style={contentContainerStyle}>
            {children}
          </Ordered>
        ) : (
          children
        )}
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({ flex: { flex: 1 } });
