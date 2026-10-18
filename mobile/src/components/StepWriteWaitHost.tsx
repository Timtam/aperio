import { useEffect, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  ActivityIndicator,
  findNodeHandle,
  Modal,
  Platform,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import { useTranslation } from 'react-i18next';

import { whenAppLockCoverGone, whenAppLockReleased } from '../state/appLock';
import { registerStepWriteWaitHost } from '../state/stepWriteWait';
import { useThemedStyles, type ThemeColors } from '../theme';

// The "please wait" dialog while a series is created in steps (decisions 227,
// 265). It has no button, and neither the back button nor a tap outside closes
// it: the write it waits for cannot be left halfway. Screen-reader focus lands
// on its sentence when it shows. The imperative opener lives in
// ../state/stepWriteWait; this is the render host, mounted once at the app root.
//
// Its close settles only once it is gone: on iOS after the Modal's onDismiss —
// iOS refuses to present or dismiss while a dismissal is still running, and
// what follows a save often navigates or opens a notice —, on Android right
// after the close commits. A close asked for before the dialog has shown waits
// for that first, briefly, so a fast save never leaves it on screen. Neither
// opening nor closing happens under the app lock's cover: a modal presented or
// dismissed there acts above or behind it, so both wait for the cover to go,
// and what follows a close waits for an unlock. And what follows speaks only a
// moment after the dialog is gone: the system moves the screen reader's focus
// back to the screen then, and a sentence said at the same moment was cut off.

/** How long a close waits for a dialog that has not shown yet. */
const SHOW_TIMEOUT_MS = 1000;
/** How long a close waits for the dismissal it asked for, at most: one the
 *  system never reports must not hold the save forever. */
const DISMISS_TIMEOUT_MS = 2000;
/** The pause before what follows may speak, as after the lock's cover
 *  (AppLockGate): spoken at once, it loses the race against the focus move. */
const ANNOUNCE_DELAY_MS = 400;

const delay = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

export function StepWriteWaitHost() {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const [visible, setVisible] = useState(false);
  const messageRef = useRef<Text | null>(null);
  const shown = useRef<{ promise: Promise<void>; resolve: () => void } | null>(null);
  const dismissed = useRef<(() => void) | null>(null);
  // Whether a write wants the dialog, and whether it was put on screen: a
  // close can come before the cover let it open.
  const wanted = useRef(false);
  const presented = useRef(false);

  useEffect(
    () =>
      registerStepWriteWaitHost({
        open: () => {
          let resolve = () => {};
          const promise = new Promise<void>((r) => {
            resolve = r;
          });
          shown.current = { promise, resolve };
          wanted.current = true;
          void whenAppLockCoverGone().then(() => {
            if (!wanted.current) return;
            presented.current = true;
            setVisible(true);
          });
        },
        close: async () => {
          wanted.current = false;
          await whenAppLockCoverGone();
          if (presented.current) {
            presented.current = false;
            const showing = shown.current?.promise ?? Promise.resolve();
            await Promise.race([showing, delay(SHOW_TIMEOUT_MS)]);
            const gone = new Promise<void>((r) => {
              dismissed.current = r;
            });
            setVisible(false);
            if (Platform.OS !== 'ios') setTimeout(() => dismissed.current?.(), 0);
            await Promise.race([gone, delay(DISMISS_TIMEOUT_MS)]);
            dismissed.current = null;
            await delay(ANNOUNCE_DELAY_MS);
          }
          if (await whenAppLockReleased()) await delay(ANNOUNCE_DELAY_MS);
        },
      }),
    [],
  );

  const onShow = () => {
    shown.current?.resolve();
    const tag = findNodeHandle(messageRef.current);
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  };

  return (
    <Modal
      transparent
      visible={visible}
      animationType="fade"
      // Neither the back button nor anything else closes it.
      onRequestClose={() => {}}
      onShow={onShow}
      onDismiss={Platform.OS === 'ios' ? () => dismissed.current?.() : undefined}
    >
      <View style={styles.root}>
        <View
          accessible={false}
          importantForAccessibility="no-hide-descendants"
          style={[StyleSheet.absoluteFill, styles.backdrop]}
        />
        <View accessibilityViewIsModal style={styles.card}>
          <Text style={styles.title} accessibilityRole="header">
            {t('dialogs.stepWrite.title')}
          </Text>
          <ActivityIndicator
            accessibilityElementsHidden
            importantForAccessibility="no-hide-descendants"
          />
          <Text ref={messageRef} style={styles.message} accessibilityRole="text">
            {t('dialogs.stepWrite.body')}
          </Text>
        </View>
      </View>
    </Modal>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    root: { flex: 1, justifyContent: 'center', alignItems: 'center', padding: 24 },
    // The one justified literal, as in AppDialog: there is no scrim token.
    backdrop: { backgroundColor: 'rgba(0,0,0,0.5)' },
    card: {
      width: '100%',
      maxWidth: 480,
      gap: 14,
      padding: 20,
      borderRadius: 14,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    title: { fontSize: 19, fontWeight: '700', color: c.textPrimary },
    message: { fontSize: 15, color: c.textSecondary, lineHeight: 21 },
  });
